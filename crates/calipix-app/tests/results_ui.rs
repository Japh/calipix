//! The Results and Plot panels, the generated dialogs and the progress
//! display, driven through the accessibility tree without a window or a
//! GPU, on ImageJ's fixtures.
#![cfg(feature = "gui")]

use std::path::PathBuf;

use calipix_app::demo;
use calipix_app::gui::Shell;
use calipix_app::state::settings::Settings;
use calipix_app::state::{AppState, Status};
use calipix_command::Args;
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

/// The application with these fixtures open, and the demo commands for
/// making a stack.
fn harness_sized(fixtures: &[&str], size: egui::Vec2) -> H {
    let mut registry = calipix_commands::standard_registry();
    demo::register(&mut registry).unwrap();
    let mut state = AppState::build(registry, Settings::default(), None);
    for name in fixtures {
        state.open(fixture(name));
        assert!(
            !matches!(state.status, Status::Failed(_)),
            "{:?}",
            state.status
        );
    }
    let mut harness = Harness::builder()
        .with_size(size)
        .with_max_steps(24)
        .build_ui_state(|ui, shell: &mut Shell| shell.ui(ui), Shell::new(state));
    harness.step();
    catch_up(&mut harness);
    harness
}

fn harness(fixtures: &[&str]) -> H {
    harness_sized(fixtures, egui::vec2(1400.0, 1000.0))
}

/// Runs frames until the interface stops asking to be redrawn. Work on
/// other threads keeps it redrawing for as long as it takes, which on a
/// slow machine is longer than any fixed number of frames; so this waits
/// in real time, up to a generous limit.
fn quiet(h: &mut H) {
    for _ in 0..600 {
        if h.run_ok().is_some() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    panic!("the interface was still redrawing after several seconds");
}

/// Lets what runs off the interface thread finish (commands, histograms)
/// and the interface show it, as time passing would.
fn catch_up(h: &mut H) {
    for _ in 0..3 {
        h.state_mut().state.wait();
        h.state_mut().state.wait_for_histograms();
        quiet(h);
    }
}

/// One frame to start what was asked for, then everything it leads to.
fn settle(h: &mut H) {
    h.step();
    catch_up(h);
}

/// What a node is called: its label, or for plain text its value.
fn name(node: &egui_kittest::Node<'_>) -> String {
    let node = node.accesskit_node();
    node.label().or_else(|| node.value()).unwrap_or_default()
}

/// The menu item with this label; its accessible name may also carry a
/// shortcut, so the label is matched as a prefix.
fn item<'a>(h: &'a H, label: &'a str) -> egui_kittest::Node<'a> {
    let prefix = format!("{label} ");
    h.get_all_by_label_contains(label)
        .filter(|node| {
            let name = name(node);
            name == label || name.starts_with(&prefix)
        })
        .min_by(|a, b| a.rect().top().total_cmp(&b.rect().top()))
        .unwrap_or_else(|| panic!("no menu item {label}"))
}

/// Opens a top-level menu and clicks an item in it.
fn choose(h: &mut H, menu: &str, label: &str) {
    h.get_by_role_and_label(Role::Button, menu).click();
    quiet(h);
    item(h, label).click();
    settle(h);
}

/// Chooses a command that sits in a submenu, as its menu item would.
fn invoke(h: &mut H, id: &str) {
    h.state_mut().invoke(id, &Args::new());
    settle(h);
}

fn press(h: &mut H, modifiers: Modifiers, key: Key) {
    h.key_press_modifiers(modifiers, key);
    settle(h);
}

fn ok(h: &mut H) {
    h.get_by_role_and_label(Role::Button, "OK").click();
    settle(h);
}

fn failed(h: &H) -> bool {
    matches!(h.state().state.status, Status::Failed(_))
}

/// The texts in a horizontal band of the window, left to right.
fn texts_in(h: &H, band: egui::Rect) -> Vec<String> {
    let mut cells: Vec<(f32, String)> = h
        .get_all_by_role(Role::Label)
        .filter(|node| band.contains(node.rect().center()))
        .map(|node| (node.rect().left(), name(&node)))
        .collect();
    cells.sort_by(|a, b| a.0.total_cmp(&b.0));
    cells.into_iter().map(|(_, text)| text).collect()
}

fn row_band(h: &H, row: usize) -> egui::Rect {
    let label = format!("Row {row}");
    h.get_by_label(&label).rect()
}

/// The cells of a row of the Results panel as shown, the number first.
fn row(h: &H, row: usize) -> Vec<String> {
    texts_in(h, row_band(h, row))
}

/// The column headings of the Results panel as shown.
fn headings(h: &H) -> Vec<String> {
    let first = row_band(h, 1);
    texts_in(h, first.translate(egui::vec2(0.0, -first.height())))
}

fn has_row(h: &H, row: usize) -> bool {
    h.query_by_label(&format!("Row {row}")).is_some()
}

fn checkbox(h: &H, label: &str) -> Option<bool> {
    let node = h.query_by_role_and_label(Role::CheckBox, label)?;
    Some(node.accesskit_node().toggled() == Some(Toggled::True))
}

/// The drop-downs of the open dialog: what each is for and what it shows.
fn dropdowns(h: &H) -> Vec<(String, String)> {
    h.get_all_by_role(Role::ComboBox)
        .map(|node| {
            let value = node.accesskit_node().value().unwrap_or_default();
            (name(&node), value)
        })
        // The Display panel behind the dialog has one of its own.
        .filter(|(label, _)| !label.is_empty() && !label.starts_with("Lookup table of"))
        .collect()
}

// ---- Results ----

#[test]
fn measure_adds_rows_to_the_results_panel_and_undo_takes_them_back() {
    let mut h = harness(&["blobs.tif"]);
    assert!(
        h.query_by_label_contains("Measurements are listed here")
            .is_none(),
        "History is in front until there is something to show"
    );

    choose(&mut h, "Analyze", "Measure");
    assert!(!failed(&h), "{:?}", h.state().state.status);
    // The panel has come to the front and shows the row as ImageJ
    // formats it, under ImageJ's headings.
    assert_eq!(headings(&h), ["Area", "Mean", "Min", "Max"]);
    assert_eq!(row(&h, 1), ["1", "3072", "46.567", "0", "255"]);
    assert!(h.query_by_label("1 row").is_some());
    assert!(!has_row(&h, 2));
    assert!(
        h.query_by_label("Measure finished; Results has 1 row")
            .is_some()
    );
    // Showing it took nothing from the image: no widget has the
    // keyboard, and the table does not act on keys.
    assert!(h.ctx.memory(|memory| memory.focused()).is_none());
    assert!(!h.state().state.table_has_attention());

    // Again, by its shortcut.
    press(&mut h, Modifiers::COMMAND, Key::M);
    assert_eq!(row(&h, 2), ["2", "3072", "46.567", "0", "255"]);
    assert!(h.query_by_label("2 rows").is_some());

    // Undo and redo are the table's too.
    press(&mut h, Modifiers::COMMAND, Key::Z);
    assert!(has_row(&h, 1));
    assert!(!has_row(&h, 2));
    assert!(h.query_by_label("1 row").is_some());
    press(&mut h, Modifiers::COMMAND | Modifiers::SHIFT, Key::Z);
    assert!(has_row(&h, 2));
}

#[test]
fn set_measurements_changes_the_columns() {
    let mut h = harness(&["blobs.tif"]);
    choose(&mut h, "Analyze", "Measure");
    assert_eq!(headings(&h), ["Area", "Mean", "Min", "Max"]);

    choose(&mut h, "Analyze", "Set Measurements…");
    assert!(h.state().dialog.is_some());
    // What Measure does now is what is ticked.
    assert_eq!(checkbox(&h, "Area"), Some(true));
    assert_eq!(checkbox(&h, "Standard deviation"), Some(false));
    assert_eq!(checkbox(&h, "Centroid"), Some(false));
    for label in ["Standard deviation", "Centroid", "Min and max gray value"] {
        h.get_by_role_and_label(Role::CheckBox, label).click();
        h.run();
    }
    ok(&mut h);
    assert!(h.state().dialog.is_none());
    assert!(!failed(&h), "{:?}", h.state().state.status);

    choose(&mut h, "Analyze", "Measure");
    // The columns are the union of what has been measured; cells of the
    // earlier row that were never measured read as ImageJ shows them.
    assert_eq!(
        headings(&h),
        ["Area", "Mean", "StdDev", "Min", "Max", "X", "Y"]
    );
    assert_eq!(
        row(&h, 1),
        ["1", "3072", "46.567", "0.000", "0", "255", "0", "0"]
    );
    assert_eq!(
        row(&h, 2),
        ["2", "3072", "46.567", "98.536", "0", "0", "32", "24"]
    );
}

#[test]
fn rows_are_clicked_to_select_and_deleted_with_the_key() {
    let mut h = harness(&["blobs.tif"]);
    for _ in 0..3 {
        press(&mut h, Modifiers::COMMAND, Key::M);
    }
    assert!(has_row(&h, 3));
    let selected = |h: &H, row: usize| {
        let label = format!("Row {row}");
        let node = h.get_by_label(&label);
        // The toolkit reports "selected" as a toggle that is on.
        node.accesskit_node().toggled() == Some(Toggled::True)
    };
    assert!(!selected(&h, 2));
    let delete = "Delete Rows";
    let disabled = |h: &H| {
        h.get_by_role_and_label(Role::Button, delete)
            .accesskit_node()
            .is_disabled()
    };
    assert!(disabled(&h), "nothing to delete yet");

    h.get_by_label("Row 2").click();
    settle(&mut h);
    assert!(selected(&h, 2));
    assert!(!selected(&h, 1));
    assert!(h.query_by_label("3 rows, 1 selected").is_some());
    assert!(!disabled(&h));

    // While the table has the keyboard, Ctrl+A is its rows, not Select
    // All on the image.
    press(&mut h, Modifiers::COMMAND, Key::A);
    assert!(h.query_by_label("3 rows, 3 selected").is_some());
    assert!(h.state().state.active().unwrap().roi.is_none());
    press(&mut h, Modifiers::NONE, Key::Escape);
    assert!(h.query_by_label("3 rows").is_some());

    h.get_by_label("Row 2").click();
    settle(&mut h);
    press(&mut h, Modifiers::NONE, Key::Delete);
    assert!(!failed(&h), "{:?}", h.state().state.status);
    assert!(h.query_by_label("2 rows").is_some());
    assert!(!has_row(&h, 3));
    let history = h.state().state.history_rows();
    assert_eq!(history.last().unwrap().title, "Delete Rows");
    assert_eq!(history.last().unwrap().detail, "rows 2");

    // The button does the same, and Clear empties the panel.
    h.get_by_label("Row 1").click();
    settle(&mut h);
    h.get_by_role_and_label(Role::Button, delete).click();
    settle(&mut h);
    assert!(h.query_by_label("1 row").is_some());
    h.get_by_role_and_label(Role::Button, "Clear").click();
    settle(&mut h);
    assert!(
        h.query_by_label_contains("Measurements are listed here")
            .is_some()
    );
    // All of it can be undone.
    press(&mut h, Modifiers::COMMAND, Key::Z);
    assert!(h.query_by_label("1 row").is_some());

    // A click on the image gives the keyboard back to it.
    h.get_by_label("Row 1").click();
    settle(&mut h);
    assert!(h.state().state.table_has_attention());
    h.get_by_label("View of blobs.tif").click();
    settle(&mut h);
    assert!(!h.state().state.table_has_attention());
    press(&mut h, Modifiers::COMMAND, Key::A);
    assert!(h.state().state.active().unwrap().roi.is_some());
    assert!(
        h.query_by_label("Selection 64 × 48 at 0, 0").is_some(),
        "the status bar shows the selection's bounds"
    );
}

// ---- Plots ----

#[test]
fn plot_profile_draws_a_plot_and_list_shows_its_numbers() {
    let mut h = harness(&["blobs.tif"]);
    let selection = Args::new().with("selection", "rectangle 2 10 60 30");
    h.state_mut().state.run("select.set", &selection);
    quiet(&mut h);
    assert!(h.query_by_label("Selection 60 × 30 at 2, 10").is_some());

    choose(&mut h, "Analyze", "Plot Profile…");
    assert_eq!(checkbox(&h, "Vertical profile"), Some(false));
    ok(&mut h);
    assert!(!failed(&h), "{:?}", h.state().state.status);
    // The Plot panel is in front with the profile in it.
    assert!(h.query_by_label("Plot: Profile of blobs.tif").is_some());
    assert!(h.query_by_label("60 points").is_some());
    assert!(!has_row(&h, 1), "the numbers are not on show yet");
    // A profile has no counts to put on a logarithmic scale.
    assert!(checkbox(&h, "Log scale").is_some(), "the Histogram panel's");
    assert_eq!(
        h.get_all_by_role_and_label(Role::CheckBox, "Log scale")
            .count(),
        1
    );

    h.get_by_role_and_label(Role::Button, "List").click();
    settle(&mut h);
    assert_eq!(headings(&h), ["Distance", "Value"]);
    assert_eq!(row(&h, 1), ["1", "0", "0.000"]);
    assert_eq!(row(&h, 3), ["3", "2", "17.000"]);
    assert!(h.query_by_label("60 rows").is_some());

    // A histogram goes to the Plot panel too, with its own scale switch,
    // and the two plots are chosen between.
    choose(&mut h, "Analyze", "Histogram…");
    ok(&mut h);
    assert!(!failed(&h), "{:?}", h.state().state.status);
    assert!(h.query_by_label("Plot: Histogram of blobs.tif").is_some());
    assert!(h.query_by_label("256 bins").is_some());
    assert_eq!(
        h.get_all_by_role_and_label(Role::CheckBox, "Log scale")
            .count(),
        2
    );
    assert_eq!(
        h.state().state.plot_names(),
        ["Histogram of blobs.tif", "Profile of blobs.tif"]
    );
    let chooser = h
        .get_all_by_role(Role::ComboBox)
        .find(|node| node.accesskit_node().value().as_deref() == Some("Histogram of blobs.tif"))
        .expect("a drop-down to choose the plot");
    chooser.click();
    quiet(&mut h);
    h.get_by_label("Profile of blobs.tif").click();
    settle(&mut h);
    assert!(h.query_by_label("Plot: Profile of blobs.tif").is_some());
}

// ---- Dialogs ----

#[test]
fn whole_stack_is_offered_for_a_stack_and_not_for_a_single_image() {
    let mut h = harness(&["blobs.tif"]);
    invoke(&mut h, "filter.gaussian_blur");
    assert!(h.state().dialog.is_some());
    assert!(h.query_by_label("Sigma").is_some());
    assert_eq!(checkbox(&h, "Scaled units"), Some(false));
    assert_eq!(checkbox(&h, "Whole stack"), None);
    // The number reads as a person would write it, with its unit after.
    let sigma = h
        .get_all_by_role(Role::SpinButton)
        .find(|node| {
            let mut labels = node.accesskit_node().labelled_by();
            labels.any(|label| label.value().as_deref() == Some("Sigma"))
        })
        .expect("a number field labelled Sigma");
    assert_eq!(sigma.accesskit_node().value().as_deref(), Some("2"));
    assert!(h.query_by_label("pixels").is_some());
    h.get_by_role_and_label(Role::Button, "Cancel").click();
    quiet(&mut h);

    let stack = Args::new()
        .with("width", 48)
        .with("height", 32)
        .with("slices", 4);
    h.state_mut().state.run("demo.test_stack", &stack);
    quiet(&mut h);
    invoke(&mut h, "filter.gaussian_blur");
    assert_eq!(checkbox(&h, "Whole stack"), Some(false));
    assert_eq!(checkbox(&h, "Scaled units"), Some(false));
    // Asked after the command's own parameters.
    let top = |label: &str| h.get_by_label(label).rect().top();
    assert!(top("Sigma") < top("Scaled units"));
    assert!(top("Scaled units") < top("Whole stack"));

    // Ticked, every plane changes; left alone, only the one in view.
    let before = h.state().state.active().unwrap().image.clone();
    h.get_by_role_and_label(Role::CheckBox, "Whole stack")
        .click();
    h.run();
    ok(&mut h);
    assert!(!failed(&h), "{:?}", h.state().state.status);
    let after = h.state().state.active().unwrap().image.clone();
    for plane in 0..4 {
        assert_ne!(
            before.read_plane(plane).unwrap(),
            after.read_plane(plane).unwrap(),
            "plane {plane}"
        );
    }
}

#[test]
fn a_choice_shows_labels_in_its_drop_down() {
    let mut h = harness(&["blobs.tif"]);
    invoke(&mut h, "view.set_lut");
    assert_eq!(
        dropdowns(&h),
        [("Lookup table".to_string(), "Fire".to_string())]
    );
    h.get_by_role_and_label(Role::ComboBox, "Lookup table")
        .click();
    quiet(&mut h);
    for label in ["Grays", "Spectrum", "3-3-2 RGB", "Red/Green"] {
        assert!(h.query_by_label(label).is_some(), "no choice {label}");
    }
    assert!(h.query_by_label("rgb332").is_none());
    h.get_by_label("3-3-2 RGB").click();
    quiet(&mut h);
    assert_eq!(dropdowns(&h)[0].1, "3-3-2 RGB");
    ok(&mut h);
    assert!(!failed(&h), "{:?}", h.state().state.status);
    // The command was given the choice, not its label.
    let history = h.state().state.history_rows();
    assert_eq!(history.last().unwrap().detail, "blobs.tif, lut rgb332");

    // A kernel is a field of several lines.
    invoke(&mut h, "filter.convolve");
    let kernel = h.get_by_role(Role::MultilineTextInput);
    let text = kernel.accesskit_node().value().unwrap_or_default();
    assert_eq!(text.lines().count(), 5);
    assert_eq!(text.lines().nth(2), Some("-1 -1 24 -1 -1"));
    // Its row is as tall as the field: the next control is below it.
    let below = h.get_by_role_and_label(Role::CheckBox, "Normalize kernel");
    assert!(below.rect().top() >= kernel.rect().bottom());
}

#[test]
fn image_calculator_offers_the_open_images() {
    let mut h = harness(&["blobs.tif", "fmt_gray8.bmp"]);
    invoke(&mut h, "math.image_calculator");
    // The active image, then the other one.
    assert_eq!(
        dropdowns(&h),
        [
            ("Image 1".to_string(), "fmt_gray8.bmp".to_string()),
            ("Operation".to_string(), "Add".to_string()),
            ("Image 2".to_string(), "blobs.tif".to_string()),
        ]
    );
    assert_eq!(checkbox(&h, "Create new image"), Some(true));
    assert!(h.query_by_label("Divide by zero value").is_none());

    h.get_by_role_and_label(Role::ComboBox, "Operation").click();
    quiet(&mut h);
    h.get_by_label("Subtract").click();
    quiet(&mut h);
    ok(&mut h);
    assert!(!failed(&h), "{:?}", h.state().state.status);
    assert!(h.state().dialog.is_none());

    // The result is a third image: the first less the second, which for
    // 8-bit samples stops at zero.
    let workspace = &h.state().state.session().workspace;
    assert_eq!(workspace.len(), 3);
    let images: Vec<_> = workspace.datasets().map(|d| d.image.clone()).collect();
    let plane = |index: usize| images[index].read_plane(0).unwrap();
    let (blobs, gray, result) = (plane(0), plane(1), plane(2));
    let (blobs, gray) = (
        blobs.as_slice::<u8>().unwrap(),
        gray.as_slice::<u8>().unwrap(),
    );
    let expected: Vec<u8> = gray
        .iter()
        .zip(blobs)
        .map(|(a, b)| a.saturating_sub(*b))
        .collect();
    assert_eq!(result.as_slice::<u8>().unwrap(), expected);
}

/// Every command that asks for something, opened in a window smaller than
/// the default: the dialog stays inside the window, its buttons are
/// reachable, and each parameter has its control.
#[test]
fn every_dialog_fits_the_window_and_shows_its_fields() {
    let size = egui::vec2(1100.0, 720.0);
    let mut h = harness_sized(&["blobs.tif", "fmt_gray8.bmp"], size);
    let stack = Args::new()
        .with("width", 48)
        .with("height", 32)
        .with("slices", 3);
    h.state_mut().state.run("demo.test_stack", &stack);
    quiet(&mut h);
    let window = egui::Rect::from_min_size(egui::Pos2::ZERO, size);

    let ids: Vec<String> = {
        let registry = h.state().state.registry();
        registry.specs().map(|spec| spec.id.clone()).collect()
    };
    let mut opened = Vec::new();
    for id in ids {
        if id.starts_with("demo.") {
            continue;
        }
        // Whether it asks is decided without running it: commands with
        // nothing to ask would run and change what the next one sees.
        let asks = {
            let state = &h.state().state;
            let spec = state.spec(&id).unwrap();
            let params = calipix_app::state::menu::asked_params(spec);
            let needs = params.iter().any(|p| {
                !calipix_app::state::menu::can_go_unasked(p)
                    && !matches!(p.ty, calipix_command::ParamType::Path(_))
            });
            needs && state.unavailable_reason(&id).is_none()
        };
        if !asks {
            continue;
        }
        h.state_mut().invoke(&id, &Args::new());
        quiet(&mut h);
        let Some(dialog) = h.state().dialog.clone() else {
            // Asked through the system's file dialog, or not at all.
            catch_up(&mut h);
            continue;
        };
        let title = dialog.spec.title.clone();
        for button in ["OK", "Cancel", "Reset"] {
            let rect = h
                .get_all_by_role_and_label(Role::Button, button)
                .map(|node| node.rect())
                // The dialog's own, not a panel's behind it.
                .max_by(|a, b| a.top().total_cmp(&b.top()))
                .unwrap_or_else(|| panic!("{title}: no {button} button"));
            assert!(
                window.contains_rect(rect),
                "{title}: {button} is at {rect:?}"
            );
        }
        for field in dialog.fields.iter().filter(|f| !f.param.advanced) {
            let label = field.param.label.as_str();
            let found = h.query_all_by_label(label).next().is_some();
            assert!(found, "{title}: nothing is labelled `{label}`");
        }
        // Nothing in it is wider than the window.
        for node in h.get_all_by_role(Role::Label) {
            let rect = node.rect();
            assert!(
                rect.right() <= window.right() + 0.5,
                "{title}: `{}` runs off the window",
                name(&node)
            );
        }
        opened.push(title);
        h.state_mut().dialog = None;
        quiet(&mut h);
    }
    assert!(
        opened.len() >= 45,
        "only {} dialogs: {opened:?}",
        opened.len()
    );
}

// ---- Long operations ----

/// A real filter on a real stack: the progress bar and Cancel appear, the
/// window keeps drawing, and Cancel leaves the image as it was.
#[test]
fn a_slow_filter_shows_progress_and_can_be_cancelled() {
    let mut h = harness(&[]);
    h.state_mut().state.run("demo.test_stack", &Args::new());
    quiet(&mut h);
    let before = h.state().state.active().unwrap().image.clone();

    invoke(&mut h, "filter.median");
    assert!(h.state().dialog.is_some());
    let dialog = h.state_mut().dialog.as_mut().unwrap();
    dialog.field_mut("radius").unwrap().draft = calipix_app::state::Draft::Float(5.0);
    dialog.field_mut("stack").unwrap().draft = calipix_app::state::Draft::Bool(true);
    h.step();
    h.get_by_role_and_label(Role::Button, "OK").click();
    h.step();
    h.step();
    assert!(h.state().dialog.is_none());
    assert!(h.state().state.running().is_some(), "the filter is running");

    // Frames keep being drawn while it runs; after half a second the
    // status bar shows the command, how far it is, and Cancel.
    let started = std::time::Instant::now();
    let mut frames = 0;
    let mut slowest = std::time::Duration::ZERO;
    loop {
        let frame = std::time::Instant::now();
        h.step();
        slowest = slowest.max(frame.elapsed());
        frames += 1;
        assert!(
            h.state().state.running().is_some(),
            "the filter finished after {:?}, before Cancel could be pressed",
            started.elapsed()
        );
        let bar = h.query_by_role(Role::ProgressIndicator);
        let cancel = h.query_by_role_and_label(Role::Button, "Cancel");
        let running = h.state().state.running().unwrap();
        if bar.is_some() && cancel.is_some() && running.fraction.is_some_and(|f| f > 0.0) {
            break;
        }
        assert!(
            started.elapsed() < std::time::Duration::from_secs(120),
            "no progress was shown"
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(frames > 5, "only {frames} frames were drawn");
    assert!(
        slowest < std::time::Duration::from_millis(500),
        "a frame took {slowest:?} while the filter ran"
    );
    // The image on screen is still the one from before.
    assert!(h.state().state.active().unwrap().image.ptr_eq(&before));

    h.get_by_role_and_label(Role::Button, "Cancel").click();
    h.step();
    h.step();
    assert!(h.state().state.running().is_none());
    quiet(&mut h);
    assert!(h.query_by_label("Median cancelled").is_some());
    assert!(h.query_by_role(Role::ProgressIndicator).is_none());
    assert!(h.state().state.active().unwrap().image.ptr_eq(&before));
    let titles: Vec<String> = h
        .state()
        .state
        .history_rows()
        .into_iter()
        .map(|row| row.title)
        .collect();
    assert_eq!(titles, ["Test Stack"]);
    // And the interface is free for the next command at once.
    assert_eq!(h.state().state.unavailable_reason("image.invert"), None);
}
