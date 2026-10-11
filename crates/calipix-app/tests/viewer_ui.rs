//! The viewer and the panels around it, driven through the accessibility
//! tree without a window or a GPU: the viewer draws through its CPU path,
//! which is the path a machine without a usable GPU takes.
#![cfg(feature = "gui")]

use std::cell::RefCell;
use std::collections::VecDeque;
use std::path::PathBuf;
use std::rc::Rc;

use calipix_app::gui::{FilePicker, Shell};
use calipix_app::state::settings::Settings;
use calipix_app::state::{AppState, PathRequest, Status};
use calipix_command::DatasetId;
use calipix_core::{AxisKind, Image};
use eframe::egui::accesskit::{Role, Toggled};
use eframe::egui::{self, Key, Modifiers};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT as _, Queryable as _};

type H = Harness<'static, Shell>;

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/imagej/out")
        .join(name)
}

fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("calipix-viewer-ui-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(name)
}

/// Stands in for the system's file dialog.
#[derive(Clone, Default)]
struct FakePicker {
    answers: Rc<RefCell<VecDeque<PathBuf>>>,
    asked: Rc<RefCell<Vec<PathRequest>>>,
}

impl FilePicker for FakePicker {
    fn pick(&mut self, request: &PathRequest) -> Option<PathBuf> {
        self.asked.borrow_mut().push(request.clone());
        self.answers.borrow_mut().pop_front()
    }
}

/// The application with these files open, in a window tall enough to
/// show every panel whole.
fn harness_with(files: &[PathBuf], picker: FakePicker) -> H {
    let mut state = AppState::build(
        calipix_commands::standard_registry(),
        Settings::default(),
        None,
    );
    for file in files {
        state.open(file.clone());
        assert!(
            !matches!(state.status, Status::Failed(_)),
            "{:?}",
            state.status
        );
    }
    let shell = Shell::with_picker(state, Box::new(picker));
    let mut harness = Harness::builder()
        .with_size(egui::vec2(1400.0, 1500.0))
        .with_max_steps(24)
        .build_ui_state(|ui, shell: &mut Shell| shell.ui(ui), shell);
    harness.run();
    catch_up(&mut harness);
    harness
}

fn harness(fixtures: &[&str]) -> H {
    let files: Vec<PathBuf> = fixtures.iter().map(|name| fixture(name)).collect();
    harness_with(&files, FakePicker::default())
}

/// Lets what runs off the interface thread finish (commands, histograms)
/// and the interface show it, as time passing would.
fn catch_up(h: &mut H) {
    for _ in 0..3 {
        h.state_mut().state.wait();
        h.state_mut().state.wait_for_histograms();
        h.run();
    }
}

/// One frame to start what was asked for, then everything it leads to.
fn settle(h: &mut H) {
    h.step();
    catch_up(h);
}

/// A click at a place in the window, for things that are not widgets of
/// their own in the accessibility tree.
fn click_at(h: &H, at: egui::Pos2) {
    h.hover_at(at);
    h.drag_at(at);
    h.drop_at(at);
}

fn active(h: &H) -> DatasetId {
    h.state().state.active().expect("an image is open").id
}

fn image(h: &H) -> Image {
    h.state().state.active().unwrap().image.clone()
}

fn range(h: &H, channel: usize) -> Option<(f64, f64)> {
    image(h).meta().channels[channel].display_range
}

fn titles(h: &H) -> Vec<String> {
    let rows = h.state().state.history_rows();
    rows.into_iter().map(|row| row.title).collect()
}

/// What a node is called: its label, or for plain text its value.
fn name(node: &egui_kittest::Node<'_>) -> String {
    let node = node.accesskit_node();
    node.label().or_else(|| node.value()).unwrap_or_default()
}

fn has_text(h: &H, text: &str) -> bool {
    h.query_all_by_label(text).next().is_some()
}

/// The menu item with this label, which is the one nearest the menu bar.
fn item<'a>(h: &'a H, label: &'a str) -> egui_kittest::Node<'a> {
    let prefix = format!("{label} ");
    h.get_all_by_label_contains(label)
        .filter(|node| {
            let text = name(node);
            text == label || text.starts_with(&prefix)
        })
        .min_by(|a, b| a.rect().top().total_cmp(&b.rect().top()))
        .unwrap_or_else(|| panic!("no menu item {label}"))
}

fn choose(h: &mut H, menu: &str, label: &str) {
    h.get_by_role_and_label(Role::Button, menu).click();
    h.run();
    item(h, label).click();
    settle(h);
}

/// Chooses a lookup table for a channel from its drop-down in the Display
/// panel.
fn choose_lut(h: &mut H, channel: &str, lut: &str) {
    h.get_by_label(&format!("Lookup table of {channel}"))
        .click();
    h.run();
    // The drop-down's own entry, not a field that already shows the name.
    h.get_all_by_label(lut)
        .last()
        .unwrap_or_else(|| panic!("no lookup table {lut} on offer"))
        .click();
    settle(h);
}

// ---- The viewer ----

#[test]
fn a_hyperstack_has_channel_buttons_and_sliders_that_move_the_view() {
    let mut h = harness(&["hyperstack_c3z4t5.tif"]);
    let id = active(&h);
    assert!(h.query_by_label("View of hyperstack_c3z4t5.tif").is_some());
    assert_eq!(h.state().state.position(id), [0, 0, 0]);

    // A slider for Z and one for T, named by their labels.
    let sliders: Vec<String> = h
        .get_all_by_role(Role::Slider)
        .map(|node| {
            let labels: Vec<String> = node
                .accesskit_node()
                .labelled_by()
                .filter_map(|label| label.value())
                .collect();
            labels.join(" ")
        })
        .collect();
    assert_eq!(sliders, ["Z", "T"]);

    // Clicking the middle of a slider goes to the middle of its axis.
    let slider = |h: &H, index: usize| h.get_all_by_role(Role::Slider).nth(index).unwrap().rect();
    let z = slider(&h, 0).center();
    click_at(&h, z);
    h.run();
    let position = h.state().state.position(id);
    assert!((1..=2).contains(&position[1]), "{position:?}");
    assert_eq!((position[0], position[2]), (0, 0));
    let t = slider(&h, 1).center();
    click_at(&h, t);
    h.run();
    assert_eq!(h.state().state.position(id)[2], 2, "frame 3 of 5");

    // Right and Left step along the slider used last, here T.
    h.key_press(Key::ArrowRight);
    h.run();
    assert_eq!(h.state().state.position(id)[2], 3);
    h.key_press(Key::ArrowLeft);
    h.run();
    h.key_press(Key::ArrowLeft);
    h.run();
    assert_eq!(h.state().state.position(id)[2], 1);
    choose(&mut h, "View", "Next Plane");
    assert_eq!(h.state().state.position(id)[2], 2);
    // None of which is a change to the image.
    assert_eq!(titles(&h), ["Open"]);

    // In a composite, each channel has a button that shows or hides it,
    // in step with the Display panel's tick box.
    let button = |h: &H, label: &str| {
        let found = h
            .get_all_by_label(label)
            .find(|node| node.accesskit_node().toggled().is_some())
            .unwrap_or_else(|| panic!("no channel button {label}"));
        (found.rect().center(), found.accesskit_node().toggled())
    };
    let (two, on) = button(&h, "2");
    assert_eq!(on, Some(Toggled::True));
    click_at(&h, two);
    h.run();
    assert!(!h.state().state.channel_visible(id, 1));
    assert!(h.state().state.channel_visible(id, 0));
    assert_eq!(button(&h, "2").1, Some(Toggled::False));
    let tick = h.get_by_role_and_label(Role::CheckBox, "Channel 2");
    assert_eq!(tick.accesskit_node().toggled(), Some(Toggled::False));
    // And back, from the panel this time.
    tick.click();
    h.run();
    assert!(h.state().state.channel_visible(id, 1));
    assert_eq!(button(&h, "2").1, Some(Toggled::True));
    assert_eq!(titles(&h), ["Open"], "hiding a channel is not recorded");
}

#[test]
fn the_status_bar_reads_out_what_is_under_the_pointer() {
    let path = temp("probe.png");
    let pixels: Vec<u8> = (0..64).collect();
    calipix_io::save(&Image::from_vec(8, 8, pixels).unwrap(), &path).unwrap();
    let mut h = harness_with(&[path], FakePicker::default());
    assert!(
        !has_text(&h, "100%"),
        "a small image opens enlarged, not at 100%"
    );
    assert!(h.state().view().probe.is_none());

    // Zoomed to fill the tab, the middle of the view is the middle of
    // the image: pixel (4, 4), whose value is 4 * 8 + 4.
    h.key_press(Key::Num0);
    h.run();
    let view = h.get_by_label("View of probe.png").rect();
    h.hover_at(view.center() + egui::vec2(2.0, 2.0));
    h.run();
    let probe = h
        .state()
        .view()
        .probe
        .clone()
        .expect("the pointer is over the image");
    assert_eq!((probe.x, probe.y), (4, 4));
    assert_eq!(probe.values.len(), 1);
    assert_eq!(probe.values[0].value, Some(36.0));
    assert!(has_text(&h, "x 4, y 4   value 36"));
    assert!(
        h.state().view().on_cpu,
        "without a GPU the CPU renderer draws"
    );
}

#[test]
fn composite_and_calibrated_images_read_out_every_channel_and_units() {
    let mut h = harness(&["hyperstack_c3z4t5.tif"]);
    let id = active(&h);
    let view = h.get_by_label("View of hyperstack_c3z4t5.tif").rect();
    // Let the planes arrive from the file before reading them.
    for _ in 0..40 {
        h.hover_at(view.center());
        h.run();
        let loaded = h
            .state()
            .view()
            .probe
            .as_ref()
            .is_some_and(|probe| probe.values.iter().all(|value| value.value.is_some()));
        if loaded {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    let probe = h
        .state()
        .view()
        .probe
        .clone()
        .expect("the pointer is over the image");
    let source = image(&h);
    let width = source.meta().width();
    let expected: Vec<Option<f64>> = (0..3)
        .map(|channel| {
            let plane = source.meta().plane_index(&[channel, 0, 0]).unwrap();
            source
                .read_plane(plane)
                .unwrap()
                .get_f64(probe.y * width + probe.x)
        })
        .collect();
    let values: Vec<Option<f64>> = probe.values.iter().map(|value| value.value).collect();
    assert_eq!(values, expected, "one value per channel of a composite");
    // The fixture is calibrated in micrometres, 0.25 a pixel.
    assert_eq!(probe.calibrated[0].unit.as_deref(), Some("µm"));
    assert_eq!(probe.calibrated[0].value, probe.x as f64 * 0.25);
    let text = calipix_app::gui::status::probe_text(&probe);
    assert!(text.contains("µm)   values "), "{text}");
    assert!(has_text(&h, &text));
    assert_eq!(h.state().state.position(id), [0, 0, 0]);
}

// ---- The Display panel ----

#[test]
fn a_lookup_table_chosen_in_the_display_panel_is_one_recorded_command() {
    let mut h = harness(&["hyperstack_c3z4t5.tif"]);
    let before = image(&h);
    assert_eq!(titles(&h), ["Open"]);

    choose_lut(&mut h, "Channel 2", "Fire");
    let fire = calipix_render::builtin_lut("fire").unwrap();
    assert_eq!(image(&h).meta().channels[1].lut, Some(fire));
    assert_eq!(
        image(&h).meta().channels[0].lut,
        before.meta().channels[0].lut
    );
    assert_eq!(titles(&h), ["Open", "Set Lookup Table"]);
    let rows = h.state().state.history_rows();
    assert_eq!(rows[1].detail, "hyperstack_c3z4t5.tif, lut fire, channel 2");
    // The panel shows what the image now says.
    let chosen = h.get_by_label("Lookup table of Channel 2");
    assert_eq!(chosen.accesskit_node().value().as_deref(), Some("Fire"));
    assert_eq!(
        h.state().state.status,
        Status::Done("Set Lookup Table finished".into())
    );

    // Auto, on the third channel: the range ImageJ's Auto button gives.
    let auto = |h: &H, index: usize| {
        let node = h.get_all_by_role_and_label(Role::Button, "Auto").nth(index);
        node.unwrap().rect().center()
    };
    let third = auto(&h, 2);
    click_at(&h, third);
    settle(&mut h);
    let expected = calipix_render::auto_contrast(&before, &[2, 0, 0], 1).unwrap();
    assert_eq!(range(&h, 2), Some(expected.range));
    assert_eq!(range(&h, 0), before.meta().channels[0].display_range);
    assert_eq!(titles(&h), ["Open", "Set Lookup Table", "Auto Contrast"]);

    // Reset, on the same channel.
    let reset = h
        .get_all_by_role_and_label(Role::Button, "Reset")
        .nth(2)
        .unwrap()
        .rect()
        .center();
    click_at(&h, reset);
    settle(&mut h);
    assert_eq!(
        range(&h, 2),
        Some(calipix_render::reset_range(&before, &[2, 0, 0]).unwrap())
    );
    assert_eq!(titles(&h).len(), 4);

    // Undo, three times, is back at the image as opened; the samples
    // were the same ones throughout.
    for expected in ["Reset Display Range", "Auto Contrast", "Set Lookup Table"] {
        h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
        settle(&mut h);
        assert_eq!(
            h.state().state.status,
            Status::Done(format!("Undid {expected}"))
        );
    }
    assert!(image(&h).ptr_eq(&before));
    assert_eq!(image(&h).meta(), before.meta());
}

#[test]
fn typing_a_range_runs_one_command() {
    let mut h = harness(&["gray16.tif"]);
    let field = h
        .get_all_by_role(Role::SpinButton)
        .find(|node| {
            node.accesskit_node()
                .labelled_by()
                .any(|label| label.value().as_deref() == Some("Max"))
        })
        .expect("the Display panel has a Max field");
    field.click();
    h.run();
    let editing = h
        .query_all_by_role(Role::SpinButton)
        .chain(h.query_all_by_role(Role::TextInput))
        .find(|node| node.is_focused())
        .expect("clicking a number field starts editing it");
    editing.type_text("30000");
    h.run();
    // Nothing happens until the value is entered.
    assert_eq!(titles(&h), ["Open"]);
    h.key_press(Key::Enter);
    settle(&mut h);
    assert_eq!(titles(&h), ["Open", "Set Display Range"]);
    let (min, max) = range(&h, 0).unwrap();
    assert_eq!(max, 30000.0);
    let opened = calipix_io::open(fixture("gray16.tif")).unwrap();
    assert_eq!(min, calipix_render::reset_range(&opened, &[]).unwrap().0);
    // Drawing more frames adds nothing.
    for _ in 0..5 {
        settle(&mut h);
    }
    assert_eq!(titles(&h).len(), 2);
}

#[test]
fn the_channel_mode_is_chosen_in_the_display_panel() {
    let mut h = harness(&["hyperstack_c3z4t5.tif"]);
    let id = active(&h);
    h.get_by_role_and_label(Role::ComboBox, "Channels").click();
    h.run();
    h.get_all_by_label("Grayscale").last().unwrap().click();
    settle(&mut h);
    assert_eq!(titles(&h), ["Open", "Set Channel Mode"]);
    assert_eq!(
        calipix_commands::view::channel_mode(image(&h).meta()),
        calipix_render::ChannelMode::Grayscale
    );
    // One channel at a time now: the panel chooses which.
    assert!(
        h.query_by_role_and_label(Role::CheckBox, "Channel 3")
            .is_none()
    );
    h.get_by_role_and_label(Role::RadioButton, "Channel 3")
        .click();
    h.run();
    assert_eq!(h.state().state.position(id)[0], 2);
    assert_eq!(
        titles(&h).len(),
        2,
        "choosing a channel to look at is not recorded"
    );
    // The Info panel says which mode the image is in.
    assert!(has_text(&h, "Channel mode"));
    assert!(has_text(&h, "Grayscale"));
}

// ---- Open, change the lookup table, save ----

#[test]
fn open_change_lookup_table_save() {
    let picker = FakePicker::default();
    let (png, tif) = (temp("fire.png"), temp("fire.tif"));
    picker
        .answers
        .borrow_mut()
        .extend([png.clone(), tif.clone()]);
    let mut h = harness_with(&[fixture("gray8.tif")], picker.clone());
    let opened = image(&h);
    let samples = opened.read_plane(0).unwrap();

    choose_lut(&mut h, "Channel 1", "Fire");
    assert_eq!(titles(&h), ["Open", "Set Lookup Table"]);
    let shown = image(&h);
    let fire = calipix_render::builtin_lut("fire").unwrap();
    assert_eq!(shown.meta().channels[0].lut, Some(fire.clone()));
    // What is on screen is what the reference renderer makes of it.
    let picture = calipix_render::render_plane(&shown, &[]).unwrap();
    let first = samples.get_f64(0).unwrap() as usize;
    assert_eq!(picture.data[..3], fire.table[first]);

    // Saved as PNG: a PNG holds samples, and a lookup table is not burned
    // into them (docs/compat.md, "Saving with a LUT or display range"), so
    // the file has the samples that were opened.
    choose(&mut h, "File", "Save As…");
    assert_eq!(picker.asked.borrow().len(), 1);
    assert_eq!(
        h.state().state.status,
        Status::Done("Save As finished".into())
    );
    let from_png = calipix_io::open(&png).unwrap();
    assert_eq!(from_png.read_plane(0).unwrap(), samples);
    assert_eq!(from_png.meta().plane_count(), 1);

    // Saved as TIFF, which can hold a lookup table: the file shows the
    // picture the reference renderer gives for that table.
    choose(&mut h, "File", "Save As…");
    let from_tif = calipix_io::open(&tif).unwrap();
    assert_eq!(from_tif.read_plane(0).unwrap(), samples);
    assert_eq!(
        calipix_render::render_plane(&from_tif, &[]).unwrap(),
        picture
    );
    // And ImageJ's own gray8 with Fire applied shows that same picture.
    let theirs = calipix_io::open(fixture("gray8_fire.tif")).unwrap();
    assert_eq!(calipix_render::render_plane(&theirs, &[]).unwrap(), picture);
    assert_eq!(
        titles(&h),
        ["Open", "Set Lookup Table", "Save As", "Save As"]
    );
}

// ---- The Histogram and Info panels ----

#[test]
fn the_histogram_panel_shows_imagej_statistics_for_the_plane_in_view() {
    let mut h = harness(&["gray16.tif"]);
    let source = image(&h);
    let reference =
        calipix_render::plane_histogram(&source, &[], &calipix_render::HistogramOptions::default())
            .unwrap();
    for label in ["Count", "Mean", "Std dev", "Min", "Max", "Mode"] {
        assert!(has_text(&h, label), "no {label}");
    }
    assert!(has_text(&h, &reference.count.to_string()));
    assert!(has_text(&h, &format!("{:.3}", reference.mean)));
    assert!(has_text(
        &h,
        &format!("{} ({})", reference.mode, reference.mode_count)
    ));
    assert!(h.query_by_label("Histogram of the plane shown").is_some());

    let log = h.get_by_role_and_label(Role::CheckBox, "Log scale");
    assert_eq!(log.accesskit_node().toggled(), Some(Toggled::False));
    log.click();
    h.run();
    let log = h.get_by_role_and_label(Role::CheckBox, "Log scale");
    assert_eq!(log.accesskit_node().toggled(), Some(Toggled::True));
    // Looking at the histogram costs one count, however many frames pass.
    let counted = h.state().state.histograms_computed();
    for _ in 0..10 {
        h.run();
    }
    assert_eq!(h.state().state.histograms_computed(), counted);
}

#[test]
fn a_composite_has_a_histogram_per_visible_channel() {
    let mut h = harness(&["hyperstack_c3z4t5.tif"]);
    let id = active(&h);
    // The legend names each channel shown.
    let named = |h: &H, name: &str| h.query_all_by_label(name).count();
    let before = named(&h, "Channel 2");
    assert!(before >= 2, "in the Display panel and in the legend");
    h.state_mut().state.set_channel_visible(id, 1, false);
    catch_up(&mut h);
    assert_eq!(named(&h, "Channel 2"), before - 1, "gone from the legend");
    // Moving to another plane shows that plane's numbers.
    let source = image(&h);
    let options = calipix_render::HistogramOptions::default();
    let mean = |position: &[usize]| {
        let histogram = calipix_render::plane_histogram(&source, position, &options).unwrap();
        format!("{}", histogram.mean)
    };
    assert!(has_text(&h, &mean(&[0, 0, 0])));
    h.state_mut().state.set_position(id, &[0, 3, 4]);
    catch_up(&mut h);
    assert!(has_text(&h, &mean(&[0, 3, 4])));
    assert!(!has_text(&h, &mean(&[0, 0, 0])));
}

#[test]
fn the_info_panel_describes_the_image() {
    let mut h = harness(&["hyperstack_c3z4t5.tif"]);
    let meta = image(&h).meta().clone();
    for text in [
        "32 × 24 pixels",
        "Channels (C)",
        "Slices (Z)",
        "Frames (T)",
        "16-bit (u16)",
        "Composite",
        "0.25 × 0.25 µm",
        "Slice spacing",
        "Frame interval",
    ] {
        assert!(has_text(&h, text), "no {text:?} in the Info panel");
    }
    assert_eq!(meta.size(&AxisKind::Time), 5);
    // The file it came from, and that it is read as needed.
    let contains = |h: &H, text: &str| h.query_all_by_label_contains(text).next().is_some();
    assert!(contains(&h, "hyperstack_c3z4t5.tif"));
    assert!(contains(&h, "read on demand"));

    // ImageJ's Info text, which this file carries, is shown as written,
    // and its other properties are a click away.
    assert_eq!(
        meta.properties.get("Info").map(String::as_str),
        Some(
            "fixture=hyperstack_c3z4t5
key=value"
        )
    );
    assert!(h.query_by_role_and_label(Role::Label, "Info").is_some());
    assert!(contains(&h, "fixture=hyperstack_c3z4t5"));
    assert!(contains(&h, "Properties ("));

    // The panel follows the active image.
    h.state_mut().state.open(fixture("gray8_calibrated.tif"));
    catch_up(&mut h);
    assert!(!contains(&h, "fixture=hyperstack_c3z4t5"));
    assert!(has_text(&h, "8-bit (u8)"));
}

// ---- History ----

#[test]
fn clicking_a_history_step_returns_to_it() {
    let mut h = harness(&["gray8.tif"]);
    let opened = image(&h);
    choose(&mut h, "Process", "Invert");
    choose_lut(&mut h, "Channel 1", "Ice");
    assert_eq!(titles(&h), ["Open", "Invert", "Set Lookup Table"]);

    h.get_by_label("2. Invert").click();
    settle(&mut h);
    let rows = h.state().state.history_rows();
    assert!(rows[1].current && rows[2].undone);
    assert_eq!(
        image(&h)
            .meta()
            .channels
            .first()
            .and_then(|c| c.lut.clone()),
        None
    );

    h.get_by_label("1. Open").click();
    settle(&mut h);
    assert!(image(&h).ptr_eq(&opened));
    h.get_by_label("3. Set Lookup Table").click();
    settle(&mut h);
    assert!(h.state().state.history_rows().iter().all(|row| !row.undone));
    assert!(image(&h).meta().channels[0].lut.is_some());

    // A closed image keeps its name in the steps that used it.
    h.key_press_modifiers(Modifiers::COMMAND, Key::W);
    settle(&mut h);
    assert!(h.state().state.active().is_none());
    let rows = h.state().state.history_rows();
    assert_eq!(rows[1].detail, "gray8.tif");
    assert_eq!(rows[3].title, "Close");
    assert_eq!(rows[3].detail, "gray8.tif");
    assert!(h.query_by_label_contains("image 0").is_none());
}

#[test]
fn two_images_can_be_shown_side_by_side() {
    let mut h = harness(&["gray8.tif", "rgb.tif"]);
    let view = |h: &H, name: &str| {
        h.query_by_label(&format!("View of {name}"))
            .map(|n| n.rect())
    };
    // In tabs, only the active image is on screen.
    assert!(view(&h, "rgb.tif").is_some());
    assert!(view(&h, "gray8.tif").is_none());
    choose(&mut h, "Window", "Show Images Side by Side");
    let (left, right) = (view(&h, "gray8.tif").unwrap(), view(&h, "rgb.tif").unwrap());
    assert!(left.right() <= right.left() + 1.0, "{left:?} {right:?}");
    assert!((left.width() - right.width()).abs() < 4.0);
    // Clicking an image makes it the one the panels describe.
    click_at(&h, left.center());
    h.run();
    assert_eq!(image(&h).meta().name, "gray8.tif");
    // Reset Layout puts them back in tabs.
    choose(&mut h, "Window", "Reset Layout");
    assert!(view(&h, "gray8.tif").is_none() || view(&h, "rgb.tif").is_none());
}
