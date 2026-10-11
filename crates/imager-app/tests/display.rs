//! Looking at images, through the application state alone: display
//! commands, dragging a display range, histograms and what is remembered
//! about each image's view.

use std::path::PathBuf;
use std::sync::Arc;

use imager_app::demo;
use imager_app::state::histogram::{self, HistogramKey, SAMPLE_ABOVE};
use imager_app::state::settings::Settings;
use imager_app::state::{AppState, Dialog, Draft, HistogramState, Invocation, Status};
use imager_command::{Args, DatasetId, Value};
use imager_commands::view::channel_mode;
use imager_core::{Buffer, DType, Image, ImageMeta, PixelSource, Rect};
use imager_render::{ChannelMode, HistogramOptions};

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/imagej/out")
        .join(name)
}

fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("imager-display-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(name)
}

/// The application with one fixture open.
fn open(name: &str) -> (AppState, DatasetId) {
    let mut registry = imager_commands::standard_registry();
    demo::register(&mut registry).unwrap();
    let mut state = AppState::build(registry, Settings::default(), None);
    state.open(fixture(name));
    let id = state.active().expect("the fixture opens").id;
    (state, id)
}

fn image(state: &AppState, id: DatasetId) -> Image {
    state.session().workspace.image(id).unwrap().clone()
}

fn range(state: &AppState, id: DatasetId, channel: usize) -> Option<(f64, f64)> {
    image(state, id).meta().channels[channel].display_range
}

fn steps(state: &AppState) -> Vec<String> {
    let entries = state.session().history().applied();
    entries.iter().map(|e| e.command.clone()).collect()
}

/// The arguments the last command ran with, as numbers.
fn last_args(state: &AppState) -> Vec<(String, i64)> {
    let entry = state.session().history().applied().last().unwrap();
    entry
        .args
        .iter()
        .filter_map(|(name, value)| match value {
            Value::Int(i) => Some((name.to_string(), *i)),
            _ => None,
        })
        .collect()
}

/// Runs a display command as the Display panel does, and waits for it.
fn display(state: &mut AppState, command: &str, id: DatasetId, more: Args) {
    state.start_display(command, id, &more);
    state.wait();
    assert!(
        !matches!(state.status, Status::Failed(_)),
        "{command}: {:?}",
        state.status
    );
}

// ---- Display commands ----

#[test]
fn display_commands_change_metadata_are_recorded_and_undone() {
    let (mut state, id) = open("hyperstack_c3z4t5.tif");
    let original = image(&state, id);
    let version = state.samples_version(id);

    display(
        &mut state,
        "view.set_lut",
        id,
        Args::new().with("lut", "fire").with("channel", 2),
    );
    let fire = imager_render::builtin_lut("fire").unwrap();
    assert_eq!(image(&state, id).meta().channels[1].lut, Some(fire));
    assert_eq!(steps(&state), ["file.open", "view.set_lut"]);
    // Looking is not changing: the samples are the ones opened.
    assert_eq!(state.samples_version(id), version);

    state.undo();
    assert!(image(&state, id).ptr_eq(&original));
    assert_eq!(state.status, Status::Done("Undid Set Lookup Table".into()));
    assert_eq!(state.samples_version(id), version);
    state.redo();
    assert_eq!(steps(&state), ["file.open", "view.set_lut"]);
    assert_eq!(state.samples_version(id), version);

    // A command that does change samples is told apart, forwards and back.
    state.run("image.invert", &Args::new());
    let inverted = state.samples_version(id);
    assert_ne!(inverted, version);
    state.undo();
    assert_ne!(state.samples_version(id), inverted);
    // Undoing past a display command and a sample change together counts
    // as a sample change only for the step that was one.
    let after_undo = state.samples_version(id);
    state.undo();
    assert_eq!(state.samples_version(id), after_undo);
}

#[test]
fn a_command_from_the_menu_acts_on_the_plane_in_view() {
    let (mut state, id) = open("hyperstack_c3z4t5.tif");
    // A composite shows its channels together: all of them are adjusted,
    // each from its own plane at the slice and frame in view.
    state.set_position(id, &[0, 2, 3]);
    assert_eq!(
        state.invoke("view.auto_contrast", &Args::new()),
        Invocation::Started
    );
    state.wait();
    assert_eq!(
        last_args(&state),
        [
            ("channel".to_string(), 0),
            ("frame".to_string(), 4),
            ("press".to_string(), 1),
            ("slice".to_string(), 3)
        ]
    );
    let source = image(&state, id);
    for channel in 0..3 {
        let expected = imager_render::auto_contrast(&source, &[channel, 2, 3], 1).unwrap();
        assert_eq!(range(&state, id, channel), Some(expected.range));
    }

    // Shown one at a time, the channel in view is the one adjusted.
    state.run(
        "view.set_channel_mode",
        &Args::new().with("mode", "grayscale"),
    );
    assert_eq!(
        channel_mode(image(&state, id).meta()),
        ChannelMode::Grayscale
    );
    state.set_position(id, &[1, 0, 0]);
    let before = range(&state, id, 0);
    state.invoke("view.reset_display_range", &Args::new());
    state.wait();
    assert_eq!(
        last_args(&state),
        [
            ("channel".to_string(), 2),
            ("frame".to_string(), 1),
            ("slice".to_string(), 1)
        ]
    );
    let source = image(&state, id);
    assert_eq!(
        range(&state, id, 1),
        Some(imager_render::reset_range(&source, &[1, 0, 0]).unwrap())
    );
    assert_eq!(range(&state, id, 0), before);
}

#[test]
fn pressing_auto_again_stretches_further_until_something_else_happens() {
    // A histogram that thins out towards both ends, so that each press
    // of Auto cuts further in: value v occurs 2 * (min(v, 255 - v) + 1)
    // times.
    let mut pixels = Vec::new();
    for value in 0..=255u8 {
        let times = 2 * (usize::from(value.min(255 - value)) + 1);
        pixels.extend(std::iter::repeat_n(value, times));
    }
    assert_eq!(pixels.len(), 172 * 192);
    let path = temp("tails.png");
    imager_io::save(&Image::from_vec(172, 192, pixels).unwrap(), &path).unwrap();
    let mut state = AppState::new();
    state.open(path);
    let id = state.active().unwrap().id;
    let source = image(&state, id);
    let auto = |press| {
        imager_render::auto_contrast(&source, &[], press)
            .unwrap()
            .range
    };
    for press in 1..=3 {
        state.invoke("view.auto_contrast", &Args::new());
        state.wait();
        assert_eq!(range(&state, id, 0), Some(auto(press)), "press {press}");
    }
    assert_ne!(auto(1), auto(3), "the fixture tells the presses apart");

    // Reset starts the count again, as in ImageJ.
    state.invoke("view.reset_display_range", &Args::new());
    state.wait();
    state.invoke("view.auto_contrast", &Args::new());
    state.wait();
    assert_eq!(range(&state, id, 0), Some(auto(1)));
    // So does undoing back to before the first press.
    state.undo();
    state.undo();
    state.invoke("view.auto_contrast", &Args::new());
    state.wait();
    assert_eq!(range(&state, id, 0), Some(auto(1)));
}

fn field<'a>(dialog: &'a Dialog, name: &str) -> &'a Draft {
    &dialog
        .fields
        .iter()
        .find(|f| f.param.name == name)
        .unwrap()
        .draft
}

#[test]
fn dialogs_of_display_commands_ask_only_what_the_view_cannot_answer() {
    let (mut state, id) = open("gray16.tif");
    display(&mut state, "view.auto_contrast", id, Args::new());
    let shown = range(&state, id, 0).unwrap();

    // Set Display Range starts from the range in use.
    let Invocation::Dialog(dialog) = state.invoke("view.set_display_range", &Args::new()) else {
        panic!("Set Display Range asks for a range");
    };
    assert_eq!(field(&dialog, "min"), &Draft::Float(shown.0));
    assert_eq!(field(&dialog, "max"), &Draft::Float(shown.1));
    assert_eq!(field(&dialog, "channel"), &Draft::Int(1));

    // Set Lookup Table asks which; nothing else needs asking.
    assert!(matches!(
        state.invoke("view.set_lut", &Args::new()),
        Invocation::Dialog(_)
    ));
    for immediate in ["view.invert_lut", "view.reset_display_range"] {
        assert_eq!(state.invoke(immediate, &Args::new()), Invocation::Started);
        state.wait();
    }
    // And the menu says so: an ellipsis only where a dialog follows.
    let label = |id: &str| imager_app::state::menu::label(state.spec(id).unwrap());
    assert_eq!(label("view.auto_contrast"), "Auto Contrast");
    assert_eq!(label("view.invert_lut"), "Invert Lookup Table");
    assert_eq!(label("view.set_lut"), "Set Lookup Table…");
    assert_eq!(label("view.enhance_contrast"), "Enhance Contrast…");
}

// ---- Dragging a display range ----

#[test]
fn a_dragged_range_is_shown_at_once_and_recorded_once() {
    let (mut state, id) = open("hyperstack_c3z4t5.tif");
    let before = image(&state, id);
    let untouched = [range(&state, id, 0), range(&state, id, 2)];
    assert_eq!(steps(&state).len(), 1);

    // The drag: many positions, each on screen, none in the history.
    for step in 0..60 {
        let max = 30000.0 - 100.0 * step as f64;
        state.preview_display_range(id, Some(1), (20100.0, max));
        let shown = state.displayed(id).unwrap();
        assert_eq!(shown.meta().channels[1].display_range, Some((20100.0, max)));
        assert!(
            image(&state, id).ptr_eq(&before),
            "the image is not touched"
        );
        assert_eq!(steps(&state).len(), 1);
        assert!(state.running().is_none(), "no command runs during a drag");
    }
    assert_eq!(
        state.previewed_range(),
        Some((id, Some(1), (20100.0, 24100.0)))
    );
    // What is shown shares the samples of the image: only metadata differs.
    assert_eq!(
        state.displayed(id).unwrap().meta().shape(),
        before.meta().shape()
    );

    // Letting go: one command, with where the drag ended.
    assert!(state.commit_display_range());
    // Until it lands, the viewer keeps showing the dragged range.
    assert_eq!(
        state.displayed(id).unwrap().meta().channels[1].display_range,
        Some((20100.0, 24100.0))
    );
    state.wait();
    assert_eq!(steps(&state), ["file.open", "view.set_display_range"]);
    let entry = state.session().history().applied().last().unwrap();
    assert_eq!(entry.args.get("min"), Some(&Value::Float(20100.0)));
    assert_eq!(entry.args.get("max"), Some(&Value::Float(24100.0)));
    assert_eq!(entry.args.get("channel"), Some(&Value::Int(2)));
    assert_eq!(range(&state, id, 1), Some((20100.0, 24100.0)));
    assert_eq!([range(&state, id, 0), range(&state, id, 2)], untouched);
    assert_eq!(state.previewed_range(), None);
    assert!(state.displayed(id).unwrap().ptr_eq(&image(&state, id)));

    // One undo takes the whole gesture back.
    state.undo();
    assert!(image(&state, id).ptr_eq(&before));
    assert!(!state.can_undo() || steps(&state) == ["file.open"]);
}

#[test]
fn a_drag_that_changes_nothing_or_is_abandoned_leaves_no_trace() {
    let (mut state, id) = open("hyperstack_c3z4t5.tif");
    let stored = range(&state, id, 0).unwrap();

    // Out and back again.
    state.preview_display_range(id, Some(0), (0.0, 1.0));
    state.preview_display_range(id, Some(0), stored);
    assert!(!state.commit_display_range());
    assert_eq!(steps(&state).len(), 1);
    assert_eq!(state.previewed_range(), None);

    // Abandoned.
    state.preview_display_range(id, Some(0), (0.0, 1.0));
    state.cancel_display_range();
    assert!(state.displayed(id).unwrap().ptr_eq(&image(&state, id)));
    assert!(!state.commit_display_range());

    // Not a range at all.
    state.preview_display_range(id, Some(0), (f64::NAN, 1.0));
    assert_eq!(state.previewed_range(), None);
    assert_eq!(steps(&state).len(), 1);
}

#[test]
fn a_drag_ending_while_a_command_runs_is_dropped_with_a_reason() {
    let (mut state, id) = open("hyperstack_c3z4t5.tif");
    let stored = range(&state, id, 0);
    state.start("demo.slow", &Args::new().with("seconds", 0.4));
    assert!(state.running().is_some());
    state.preview_display_range(id, Some(0), (1.0, 2.0));
    assert!(!state.commit_display_range());
    assert_eq!(
        state.status,
        Status::Done("Set Display Range: Wait for Slow Command to finish".into())
    );
    assert_eq!(state.previewed_range(), None);
    state.wait();
    assert_eq!(range(&state, id, 0), stored);
    assert_eq!(steps(&state), ["file.open", "demo.slow"]);
}

#[test]
fn an_rgb_image_is_dragged_as_one() {
    let (mut state, id) = open("rgb.tif");
    state.preview_display_range(id, None, (10.0, 200.0));
    assert!(state.commit_display_range());
    state.wait();
    for channel in 0..3 {
        assert_eq!(range(&state, id, channel), Some((10.0, 200.0)));
    }
    let entry = state.session().history().applied().last().unwrap();
    assert_eq!(entry.args.get("channel"), Some(&Value::Int(0)));
    // The samples were not rewritten, as ImageJ would have.
    let opened = imager_io::open(fixture("rgb.tif")).unwrap();
    for plane in 0..3 {
        assert_eq!(
            image(&state, id).read_plane(plane).unwrap(),
            opened.read_plane(plane).unwrap()
        );
    }
}

// ---- What is remembered about a view ----

#[test]
fn the_view_of_an_image_lasts_as_long_as_the_image_is_open() {
    let (mut state, id) = open("hyperstack_c3z4t5.tif");
    assert_eq!(state.position(id), [0, 0, 0]);
    state.set_position(id, &[1, 3, 4]);
    assert_eq!(state.position(id), [1, 3, 4]);
    // Positions are kept inside the image, whatever is asked.
    state.set_position(id, &[9, 9, 9]);
    assert_eq!(state.position(id), [2, 3, 4]);
    state.set_position(id, &[1]);
    assert_eq!(state.position(id), [1, 0, 0]);

    // Hiding a channel is not a change to the image: nothing is recorded,
    // and nothing can be undone.
    assert!(state.channel_visible(id, 2));
    state.set_channel_visible(id, 2, false);
    assert!(!state.channel_visible(id, 2));
    assert!(state.channel_visible(id, 0));
    assert_eq!(steps(&state), ["file.open"]);
    assert!(!state.can_undo() || steps(&state).len() == 1);

    // It survives changes to the image it is a view of.
    state.set_position(id, &[1, 2, 3]);
    display(&mut state, "view.invert_lut", id, Args::new());
    state.run("image.invert", &Args::new());
    assert_eq!(state.position(id), [1, 2, 3]);
    assert!(!state.channel_visible(id, 2));

    // Another image has a view of its own.
    state.open(fixture("gray8.tif"));
    let other = state.active().unwrap().id;
    assert_eq!(state.position(other), Vec::<usize>::new());
    assert!(state.channel_visible(other, 2));
    state.set_active(id);

    // Closing the image forgets its view; an image that comes back by
    // undo starts afresh.
    let version = state.samples_version(id);
    state.run("file.close", &Args::new().with("input", id));
    assert!(state.session().workspace.get(id).is_err());
    assert_eq!(state.position(id), Vec::<usize>::new());
    assert!(state.channel_visible(id, 2));
    assert_eq!(state.samples_version(id), 0);
    state.set_position(id, &[1, 1, 1]);
    state.set_channel_visible(id, 0, false);
    assert!(
        state.view(id).position.is_empty(),
        "nothing is kept for a closed image"
    );
    assert!(state.channel_visible(id, 0));

    state.undo();
    assert!(state.session().workspace.get(id).is_ok());
    assert_eq!(state.position(id), [0, 0, 0]);
    assert!(state.channel_visible(id, 2));
    assert_ne!(state.samples_version(id), version);
    assert_ne!(state.samples_version(id), 0);
}

#[test]
fn stepping_needs_planes_to_step_through() {
    let (mut state, _) = open("gray8.tif");
    assert_eq!(
        state.unavailable_reason("view.next_plane").as_deref(),
        Some("The image has a single plane")
    );
    state.open(fixture("hyperstack_c3z4t5.tif"));
    assert_eq!(state.unavailable_reason("view.next_plane"), None);
    assert_eq!(
        state.unavailable_reason("window.tile_images"),
        None,
        "two images are open"
    );
}

// ---- Histograms ----

/// The histogram of a plane, once it has been counted.
fn histogram(state: &mut AppState, id: DatasetId, plane: usize) -> imager_render::Histogram {
    state.histogram(id, plane, false);
    state.wait_for_histograms();
    match state.histogram(id, plane, false) {
        HistogramState::Ready(found) => {
            assert_eq!(found.sampled, None);
            found.histogram.clone()
        }
        other => panic!("no histogram: {other:?}"),
    }
}

#[test]
fn histograms_are_counted_once_per_plane_of_samples() {
    let (mut state, id) = open("hyperstack_c3z4t5.tif");
    let source = image(&state, id);
    let options = HistogramOptions::default();
    let reference = |plane: usize| {
        let position = source.meta().plane_position(plane).unwrap();
        imager_render::plane_histogram(&source, &position, &options).unwrap()
    };

    // An image read from a file is counted off the calling thread.
    assert_eq!(state.histogram(id, 4, false), HistogramState::Pending);
    assert_eq!(histogram(&mut state, id, 4), reference(4));
    assert_eq!(state.histograms_computed(), 1);

    // Asking again, as every frame does, counts nothing.
    for _ in 0..50 {
        assert!(state.histogram(id, 4, false).ready().is_some());
    }
    assert_eq!(state.histograms_computed(), 1);

    // Nor does changing how the image is displayed, or undoing that.
    display(&mut state, "view.auto_contrast", id, Args::new());
    display(
        &mut state,
        "view.set_lut",
        id,
        Args::new().with("lut", "ice"),
    );
    state.preview_display_range(id, Some(1), (1.0, 2.0));
    assert!(state.histogram(id, 4, false).ready().is_some());
    state.cancel_display_range();
    state.undo();
    assert!(state.histogram(id, 4, false).ready().is_some());
    // (Auto Contrast looked at the plane it adjusted, which is another.)
    state.wait_for_histograms();
    let counted = state.histograms_computed();
    assert_eq!(counted, 2);
    assert!(state.histogram(id, 4, false).ready().is_some());
    assert_eq!(state.histograms_computed(), counted);

    // Another plane is another histogram; the first is kept.
    assert_eq!(histogram(&mut state, id, 17), reference(17));
    assert_eq!(state.histograms_computed(), counted + 1);
    assert!(state.histogram(id, 4, false).ready().is_some());
    assert_eq!(state.histograms_computed(), counted + 1);

    // New samples: counted again, and different.
    state.run("image.invert", &Args::new());
    let inverted = image(&state, id);
    let position = inverted.meta().plane_position(4).unwrap();
    let expected = imager_render::plane_histogram(&inverted, &position, &options).unwrap();
    let recounted = histogram(&mut state, id, 4);
    assert_eq!(recounted, expected);
    assert_eq!(state.histograms_computed(), counted + 2);

    // Undo brings the old samples back, and with them a recount.
    state.undo();
    assert_eq!(histogram(&mut state, id, 4), reference(4));
    assert_eq!(state.histograms_computed(), counted + 3);

    // A closed image has no histogram.
    state.run("file.close", &Args::new());
    assert!(matches!(
        state.histogram(id, 4, false),
        HistogramState::Failed(_)
    ));
}

#[test]
fn a_small_image_in_memory_is_counted_on_the_spot() {
    let path = temp("inline.png");
    let pixels: Vec<u8> = (0..=255).collect();
    imager_io::save(&Image::from_vec(16, 16, pixels).unwrap(), &path).unwrap();
    let mut state = AppState::new();
    state.open(path);
    let id = state.active().unwrap().id;
    assert!(image(&state, id).is_in_memory());
    let HistogramState::Ready(found) = state.histogram(id, 0, false) else {
        panic!("a small plane in memory needs no waiting");
    };
    assert_eq!(found.histogram.count, 256);
    assert_eq!(found.histogram.mean, 127.5);
}

#[test]
fn an_rgb_image_has_a_grey_histogram_and_one_per_channel() {
    let (mut state, id) = open("rgb.tif");
    let source = image(&state, id);
    let options = HistogramOptions::default();
    state.histogram(id, 0, true);
    state.histogram(id, 1, false);
    state.wait_for_histograms();
    let grey = state.histogram(id, 0, true);
    let green = state.histogram(id, 1, false);
    assert_eq!(
        grey.ready().unwrap().histogram,
        imager_render::plane_histogram(&source, &[0], &options).unwrap()
    );
    assert_eq!(
        green.ready().unwrap().histogram,
        imager_render::buffer_histogram(&source.read_plane(1).unwrap(), &options).unwrap()
    );
    assert_ne!(grey, green);
}

/// A plane too large to read whole: reading all of it is an error, and
/// every region read is counted.
#[derive(Debug)]
struct Huge {
    meta: ImageMeta,
    samples_read: std::sync::atomic::AtomicUsize,
}

impl Huge {
    fn value(x: usize, y: usize) -> u16 {
        ((x / 64) * 7 + (y / 64) * 13) as u16
    }
}

impl PixelSource for Huge {
    fn meta(&self) -> &ImageMeta {
        &self.meta
    }

    fn read_plane(&self, _plane: usize) -> imager_core::Result<Buffer> {
        Err(imager_core::Error::Unsupported(
            "the whole plane must not be read".to_string(),
        ))
    }

    fn read_region(&self, _plane: usize, region: Rect) -> imager_core::Result<Buffer> {
        self.samples_read
            .fetch_add(region.w * region.h, std::sync::atomic::Ordering::Relaxed);
        let mut samples = Vec::with_capacity(region.w * region.h);
        for y in region.y..region.y + region.h {
            for x in region.x..region.x + region.w {
                samples.push(Huge::value(x, y));
            }
        }
        Ok(Buffer::from_vec(samples))
    }
}

#[test]
fn a_huge_plane_is_estimated_from_a_sample_and_says_so() {
    let side = 8192;
    assert!(side * side > SAMPLE_ABOVE);
    let source = Arc::new(Huge {
        meta: ImageMeta::plane(DType::U16, side, side)
            .unwrap()
            .with_name("huge"),
        samples_read: Default::default(),
    });
    // `Image::new` takes the source by value; share the counter instead.
    #[derive(Debug)]
    struct Shared(Arc<Huge>);
    impl PixelSource for Shared {
        fn meta(&self) -> &ImageMeta {
            self.0.meta()
        }
        fn read_plane(&self, plane: usize) -> imager_core::Result<Buffer> {
            self.0.read_plane(plane)
        }
        fn read_region(&self, plane: usize, region: Rect) -> imager_core::Result<Buffer> {
            self.0.read_region(plane, region)
        }
    }
    let image = Image::new(Shared(source.clone()));
    let key = HistogramKey {
        image: DatasetId(0),
        plane: 0,
        gray: false,
    };
    let found = histogram::count(&image, key).unwrap();

    let read = source
        .samples_read
        .load(std::sync::atomic::Ordering::Relaxed);
    assert_eq!(read, 64 * 256 * 256, "an 8 x 8 grid of 256 x 256 regions");
    assert_eq!(found.histogram.count as usize, read);
    assert_eq!(found.sampled, Some(read as f64 / (side * side) as f64));
    // The grid reaches every corner, so the extremes of this ramp are seen.
    assert_eq!(found.histogram.min, 0.0);
    assert_eq!(
        found.histogram.max,
        f64::from(Huge::value(side - 1, side - 1))
    );
}

// ---- History ----

#[test]
fn history_names_images_as_they_were_and_can_be_returned_to() {
    let (mut state, id) = open("gray8.tif");
    state.run("image.invert", &Args::new());
    display(
        &mut state,
        "view.set_lut",
        id,
        Args::new().with("lut", "fire"),
    );
    state.run("image.info", &Args::new());
    state.run("file.close", &Args::new());

    let rows = state.history_rows();
    let titles: Vec<&str> = rows.iter().map(|row| row.title.as_str()).collect();
    assert_eq!(
        titles,
        ["Open", "Invert", "Set Lookup Table", "Image Info", "Close"]
    );
    // The image is closed, and still called by its name.
    assert_eq!(rows[1].detail, "gray8.tif");
    assert_eq!(rows[2].detail, "gray8.tif, lut fire, channel 1");
    assert_eq!(rows[3].detail, "gray8.tif");
    assert_eq!(rows[4].detail, "gray8.tif");
    assert!(rows[4].current && rows.iter().all(|row| !row.undone));
    assert!(!rows[3].changed, "Image Info changes nothing");

    // Back to just after Invert: the image is open again, inverted, and
    // without the lookup table.
    state.go_to_history(1);
    let rows = state.history_rows();
    assert!(rows[1].current);
    assert_eq!(
        rows.iter().map(|row| row.undone).collect::<Vec<_>>(),
        [false, false, true, true, true]
    );
    let now = image(&state, id);
    assert_eq!(
        now.meta().channels.first().and_then(|c| c.lut.clone()),
        None
    );
    let opened = imager_io::open(fixture("gray8.tif")).unwrap();
    assert_ne!(now.read_plane(0).unwrap(), opened.read_plane(0).unwrap());

    // Forward to the lookup table, but not on to Close.
    state.go_to_history(3);
    let rows = state.history_rows();
    assert!(!rows[2].undone && rows[4].undone);
    assert!(state.session().workspace.get(id).is_ok());
    assert!(image(&state, id).meta().channels[0].lut.is_some());

    // All the way back, and all the way forward.
    state.go_to_history(0);
    assert!(state.history_rows()[0].current);
    assert_eq!(
        image(&state, id).read_plane(0).unwrap(),
        opened.read_plane(0).unwrap()
    );
    state.go_to_history(4);
    assert!(state.session().workspace.get(id).is_err());
    assert!(state.history_rows().iter().all(|row| !row.undone));
}
