//! The display commands: they change how an image is shown, agree with
//! the reference renderer about what to show, and never touch a sample.

use std::path::PathBuf;
use std::sync::Arc;

use calipix_command::{Args, DatasetId, EffectsPolicy, Error, Executor, Session};
use calipix_commands::standard_registry;
use calipix_commands::view::{DISPLAY_COMMANDS, MODE_PROPERTY, channel_mode, plane_position};
use calipix_core::{AxisKind, ColorModel, Image, ImageMeta};
use calipix_render::{ChannelMode, DisplayOptions};

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/imagej/out")
        .join(name)
}

fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("calipix-view-commands-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(name)
}

fn executor() -> Executor {
    Executor::new(Arc::new(standard_registry()), EffectsPolicy::allow_all())
}

/// A session holding one fixture, read into memory so that its planes can
/// be compared by where they are stored.
fn open(name: &str) -> (Executor, Session, DatasetId) {
    let mut session = Session::new();
    let image = calipix_io::open(fixture(name))
        .unwrap()
        .materialize()
        .unwrap();
    let id = session.workspace.insert(image, None);
    (executor(), session, id)
}

fn image(session: &Session, id: DatasetId) -> Image {
    session.workspace.image(id).unwrap().clone()
}

fn meta(session: &Session, id: DatasetId) -> ImageMeta {
    image(session, id).meta().clone()
}

fn range(session: &Session, id: DatasetId, channel: usize) -> Option<(f64, f64)> {
    meta(session, id).channels[channel].display_range
}

fn shares_every_plane(a: &Image, b: &Image) -> bool {
    (0..a.meta().plane_count()).all(|plane| {
        a.read_plane(plane)
            .unwrap()
            .shares_storage(&b.read_plane(plane).unwrap())
    })
}

/// Arguments that make every display command do something to a
/// three-channel image.
fn working_args(id: &str) -> Args {
    match id {
        "view.set_display_range" => Args::new().with("min", 3.0).with("max", 90.0),
        "view.set_lut" => Args::new().with("lut", "fire"),
        "view.set_channel_mode" => Args::new().with("mode", "grayscale"),
        "view.auto_contrast" => Args::new().with("slice", 2).with("frame", 3),
        _ => Args::new(),
    }
}

#[test]
fn display_commands_change_metadata_and_share_every_sample() {
    for command in DISPLAY_COMMANDS {
        let (exec, mut session, id) = open("hyperstack_c3z4t5.tif");
        let before = image(&session, id);
        exec.run(&mut session, command, &working_args(command))
            .unwrap_or_else(|e| panic!("{command}: {e}"));
        let after = image(&session, id);

        assert!(!after.ptr_eq(&before), "{command} changed nothing");
        assert_ne!(after.meta(), before.meta(), "{command}");
        assert_eq!(after.meta().shape(), before.meta().shape(), "{command}");
        assert_eq!(after.meta().dtype(), before.meta().dtype(), "{command}");
        assert!(after.is_in_memory(), "{command} left the image a view");
        assert!(shares_every_plane(&before, &after), "{command}");

        // Recorded once, and undone by putting the old image back.
        let history = session.history();
        assert_eq!(history.entries().len(), 1, "{command}");
        assert_eq!(history.entries()[0].command, command);
        assert!(history.entries()[0].changed_workspace(), "{command}");
        exec.undo(&mut session).unwrap();
        assert!(image(&session, id).ptr_eq(&before), "{command}");
        exec.redo(&mut session).unwrap();
        assert!(image(&session, id).ptr_eq(&after), "{command}");
    }
}

#[test]
fn every_display_command_is_listed() {
    let registry = standard_registry();
    let mut found: Vec<&str> = registry
        .specs()
        .filter(|spec| {
            spec.menu.as_ref().is_some_and(|m| m.path == ["View"]) && spec.flags.headless_ok
        })
        .map(|spec| spec.id.as_str())
        .collect();
    let mut listed = DISPLAY_COMMANDS.to_vec();
    found.sort_unstable();
    listed.sort_unstable();
    assert_eq!(found, listed);
    assert_eq!(
        registry.by_ijm_name("Invert LUT").unwrap().spec().id,
        "view.invert_lut"
    );
    assert_eq!(
        registry
            .by_ijm_name("Enhance Contrast...")
            .unwrap()
            .spec()
            .id,
        "view.enhance_contrast"
    );
}

#[test]
fn set_display_range_sets_one_channel_or_all() {
    let (exec, mut session, id) = open("hyperstack_c3z4t5.tif");
    let before = meta(&session, id);
    let args = Args::new().with("min", 10.0).with("max", 20.5);
    exec.run(
        &mut session,
        "view.set_display_range",
        &args.clone().with("channel", 2),
    )
    .unwrap();
    assert_eq!(range(&session, id, 1), Some((10.0, 20.5)));
    assert_eq!(range(&session, id, 0), before.channels[0].display_range);
    assert_eq!(range(&session, id, 2), before.channels[2].display_range);

    exec.run(&mut session, "view.set_display_range", &args)
        .unwrap();
    for channel in 0..3 {
        assert_eq!(range(&session, id, channel), Some((10.0, 20.5)));
    }
    // Nothing but the ranges moved.
    let mut expected = before;
    for channel in &mut expected.channels {
        channel.display_range = Some((10.0, 20.5));
    }
    assert_eq!(meta(&session, id), expected);
}

#[test]
fn ranges_equal_the_reference_renderer() {
    for name in [
        "hyperstack_c3z4t5.tif",
        "gray8.tif",
        "gray16.tif",
        "gray16_signed.tif",
        "gray32f.tif",
        "rgb.tif",
    ] {
        let (exec, mut session, id) = open(name);
        let source = image(&session, id);
        let channels = source.meta().size(&AxisKind::Channel);
        let rgb = source.meta().color == ColorModel::Rgb;
        let (slice, frame) = if name.starts_with("hyperstack") {
            (3, 4)
        } else {
            (1, 1)
        };
        let place = Args::new().with("slice", slice).with("frame", frame);
        // An RGB image is one picture: every channel gets the range found
        // for its grey values.
        let position = |channel: usize| {
            plane_position(source.meta(), if rgb { 0 } else { channel }, slice, frame).unwrap()
        };

        for press in [1, 2, 5] {
            exec.run(
                &mut session,
                "view.auto_contrast",
                &place.clone().with("press", press),
            )
            .unwrap();
            for channel in 0..channels {
                let expected =
                    calipix_render::auto_contrast(&source, &position(channel), press as u32)
                        .unwrap()
                        .range;
                assert_eq!(
                    range(&session, id, channel),
                    Some(expected),
                    "{name} auto press {press} channel {channel}"
                );
            }
        }

        exec.run(&mut session, "view.reset_display_range", &place)
            .unwrap();
        for channel in 0..channels {
            let expected = calipix_render::reset_range(&source, &position(channel)).unwrap();
            assert_eq!(range(&session, id, channel), Some(expected), "{name} reset");
        }

        for (saturated, stack) in [(0.35, false), (5.0, false), (1.0, true)] {
            let args = place
                .clone()
                .with("saturated", saturated)
                .with("use_stack_histogram", stack);
            let expected: Vec<Option<(f64, f64)>> = (0..channels)
                .map(|channel| {
                    calipix_render::enhance_contrast(&source, &position(channel), saturated, stack)
                        .unwrap()
                })
                .collect();
            let before: Vec<_> = (0..channels).map(|c| range(&session, id, c)).collect();
            let result = exec.run(&mut session, "view.enhance_contrast", &args);
            if expected.iter().all(Option::is_none) {
                assert!(result.is_err(), "{name} enhance {saturated}");
                continue;
            }
            result.unwrap();
            for channel in 0..channels {
                // Where ImageJ finds no range it leaves the old one.
                assert_eq!(
                    range(&session, id, channel),
                    expected[channel].or(before[channel]),
                    "{name} enhance {saturated} stack {stack} channel {channel}"
                );
            }
        }
    }
}

#[test]
fn auto_contrast_on_one_channel_leaves_the_others() {
    let (exec, mut session, id) = open("hyperstack_c3z4t5.tif");
    let before = meta(&session, id);
    exec.run(
        &mut session,
        "view.auto_contrast",
        &Args::new().with("channel", 3).with("slice", 2),
    )
    .unwrap();
    let source = image(&session, id);
    let expected = calipix_render::auto_contrast(&source, &[2, 1, 0], 1)
        .unwrap()
        .range;
    assert_eq!(range(&session, id, 2), Some(expected));
    assert_eq!(range(&session, id, 0), before.channels[0].display_range);
    assert_eq!(range(&session, id, 1), before.channels[1].display_range);
}

#[test]
fn lookup_tables_are_the_reference_ones() {
    let (exec, mut session, id) = open("hyperstack_c3z4t5.tif");
    let original = image(&session, id);
    for lut in calipix_render::builtin_luts() {
        exec.run(
            &mut session,
            "view.set_lut",
            &Args::new().with("lut", lut.id).with("channel", 2),
        )
        .unwrap();
        let meta = meta(&session, id);
        assert_eq!(meta.channels[1].lut, Some(lut.lut()), "{}", lut.id);
        assert_eq!(meta.channels[0].lut, original.meta().channels[0].lut);
    }

    // Inverting takes whatever the channel is shown through, including a
    // default colour it never stored, and twice gives the original back.
    let shown: Vec<_> = (0..3)
        .map(|c| calipix_render::channel_lut(&image(&session, id), c, ChannelMode::Auto))
        .collect();
    exec.run(&mut session, "view.invert_lut", &Args::new())
        .unwrap();
    for (channel, lut) in shown.iter().enumerate() {
        assert_eq!(
            meta(&session, id).channels[channel].lut,
            Some(calipix_render::inverted_lut(lut))
        );
    }
    exec.run(&mut session, "view.invert_lut", &Args::new())
        .unwrap();
    for (channel, lut) in shown.iter().enumerate() {
        assert_eq!(meta(&session, id).channels[channel].lut.as_ref(), Some(lut));
    }
}

#[test]
fn inverting_a_plain_image_matches_imagej() {
    // gray8_invlut.tif is gray8.tif after ImageJ's Invert LUT.
    let (exec, mut session, id) = open("gray8.tif");
    exec.run(&mut session, "view.invert_lut", &Args::new())
        .unwrap();
    let ours = calipix_render::render_plane(&image(&session, id), &[]).unwrap();
    let theirs = calipix_io::open(fixture("gray8_invlut.tif")).unwrap();
    assert_eq!(ours, calipix_render::render_plane(&theirs, &[]).unwrap());
}

#[test]
fn channel_mode_is_kept_in_the_metadata() {
    let (exec, mut session, id) = open("hyperstack_c3z4t5.tif");
    assert_eq!(channel_mode(&meta(&session, id)), ChannelMode::Composite);
    let position = [1, 2, 3];
    for (name, mode) in [
        ("grayscale", ChannelMode::Grayscale),
        ("color", ChannelMode::Color),
        ("composite", ChannelMode::Composite),
    ] {
        exec.run(
            &mut session,
            "view.set_channel_mode",
            &Args::new().with("mode", name),
        )
        .unwrap();
        let shown = image(&session, id);
        assert_eq!(channel_mode(shown.meta()), mode);
        assert_eq!(
            shown.meta().properties.contains_key(MODE_PROPERTY),
            mode != ChannelMode::Composite
        );
        // The mode read back from the metadata draws what the renderer
        // draws when told that mode outright.
        let options = |mode| DisplayOptions {
            mode,
            ..DisplayOptions::default()
        };
        assert_eq!(
            calipix_render::render_plane_with(
                &shown,
                &position,
                &options(channel_mode(shown.meta()))
            )
            .unwrap(),
            calipix_render::render_plane_with(
                &image_with_mode(&shown, ColorModel::Composite),
                &position,
                &options(mode)
            )
            .unwrap(),
            "{name}"
        );
    }
}

/// `image` with its colour model replaced, to render it in a mode chosen
/// by the caller rather than by its metadata.
fn image_with_mode(image: &Image, color: ColorModel) -> Image {
    let mut meta = image.meta().clone();
    meta.color = color;
    image.with_meta(meta).unwrap()
}

#[test]
fn an_rgb_image_made_composite_looks_the_same() {
    let (exec, mut session, id) = open("rgb.tif");
    let before = calipix_render::render_plane(&image(&session, id), &[0]).unwrap();
    exec.run(&mut session, "view.set_channel_mode", &Args::new())
        .unwrap();
    let composite = image(&session, id);
    assert_eq!(composite.meta().color, ColorModel::Composite);
    assert_eq!(
        calipix_render::render_plane(&composite, &[0]).unwrap(),
        before
    );
    // Which is what gives it lookup tables to change.
    exec.run(
        &mut session,
        "view.set_lut",
        &Args::new().with("lut", "cyan").with("channel", 1),
    )
    .unwrap();
    assert_ne!(
        calipix_render::render_plane(&image(&session, id), &[0]).unwrap(),
        before
    );
}

#[test]
fn refusals_say_why_and_change_nothing() {
    let cases: [(&str, &str, Args, &str); 6] = [
        (
            "rgb.tif",
            "view.set_lut",
            Args::new().with("lut", "fire"),
            "RGB image has no lookup table",
        ),
        (
            "rgb.tif",
            "view.invert_lut",
            Args::new(),
            "RGB image has no lookup table",
        ),
        (
            "gray8.tif",
            "view.set_channel_mode",
            Args::new(),
            "single channel",
        ),
        (
            "gray8.tif",
            "view.auto_contrast",
            Args::new().with("channel", 2),
            "the image has one channel",
        ),
        (
            "hyperstack_c3z4t5.tif",
            "view.reset_display_range",
            Args::new().with("slice", 5),
            "5 is outside 1 to 4",
        ),
        (
            "gray8.tif",
            "view.set_display_range",
            Args::new().with("min", f64::INFINITY).with("max", 1.0),
            "finite",
        ),
    ];
    for (file, command, args, reason) in cases {
        let (exec, mut session, id) = open(file);
        let before = image(&session, id);
        let error = exec.run(&mut session, command, &args).unwrap_err();
        assert!(error.to_string().contains(reason), "{command}: {error}");
        assert!(image(&session, id).ptr_eq(&before), "{command}");
        assert!(session.history().entries().is_empty(), "{command}");
    }

    let (exec, mut session, _) = open("gray8.tif");
    let unknown = exec.run(
        &mut session,
        "view.set_lut",
        &Args::new().with("lut", "plasma"),
    );
    assert!(matches!(unknown, Err(Error::InvalidArg { .. })));
}

#[test]
fn a_lazily_read_image_stays_lazy_and_keeps_its_samples() {
    let mut session = Session::new();
    let exec = executor();
    let lazy = calipix_io::open(fixture("hyperstack_c3z4t5.tif")).unwrap();
    let in_memory = lazy.is_in_memory();
    let id = session.workspace.insert(lazy.clone(), None);
    exec.run(
        &mut session,
        "view.set_lut",
        &Args::new().with("lut", "ice"),
    )
    .unwrap();
    exec.run(&mut session, "view.auto_contrast", &Args::new())
        .unwrap();
    let shown = image(&session, id);
    assert_eq!(shown.is_in_memory(), in_memory);
    for plane in 0..lazy.meta().plane_count() {
        assert_eq!(
            shown.read_plane(plane).unwrap(),
            lazy.read_plane(plane).unwrap()
        );
    }
}

#[test]
fn a_lookup_table_survives_saving_as_tiff() {
    let (exec, mut session, id) = open("gray8.tif");
    exec.run(
        &mut session,
        "view.set_lut",
        &Args::new().with("lut", "fire"),
    )
    .unwrap();
    let saved = temp("gray8_fire.tif");
    exec.run(
        &mut session,
        "file.save_as",
        &Args::new().with("path", saved.clone()),
    )
    .unwrap();

    let reopened = calipix_io::open(&saved).unwrap();
    let fire = calipix_render::builtin_lut("fire").unwrap();
    assert_eq!(
        reopened.meta().channels[0].lut.as_ref().unwrap().table,
        fire.table
    );
    assert_eq!(
        reopened.read_plane(0).unwrap(),
        image(&session, id).read_plane(0).unwrap()
    );
    // ImageJ's own gray8 with Fire applied shows the same picture.
    let theirs = calipix_io::open(fixture("gray8_fire.tif")).unwrap();
    assert_eq!(
        calipix_render::render_plane(&reopened, &[]).unwrap(),
        calipix_render::render_plane(&theirs, &[]).unwrap()
    );
}
