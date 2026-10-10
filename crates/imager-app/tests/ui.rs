//! The real widgets, driven without a window through the accessibility
//! tree: what a person sees and clicks, not how it is painted.
#![cfg(feature = "gui")]

use std::cell::RefCell;
use std::collections::VecDeque;
use std::path::PathBuf;
use std::rc::Rc;

use eframe::egui::accesskit::{Role, Toggled};
use eframe::egui::{self, Key, Modifiers};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT as _, Queryable as _};
use imager_app::demo;
use imager_app::gui::{FilePicker, Shell};
use imager_app::state::settings::Settings;
use imager_app::state::{AppState, LogLevel, Panel, PathRequest, Status, Theme};
use imager_command::PathMode;
use imager_core::Image;

type H = Harness<'static, Shell>;

fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("imager-ui-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(name)
}

fn sample(name: &str) -> PathBuf {
    let path = temp(name);
    imager_io::save(&Image::from_vec(2, 1, vec![10u8, 250]).unwrap(), &path).unwrap();
    path
}

/// Stands in for the system's file dialog: answers with the paths it was
/// given, in order, and keeps what it was asked.
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

fn state(files: &[&str]) -> AppState {
    let mut registry = imager_commands::standard_registry();
    demo::register(&mut registry).unwrap();
    let mut state = AppState::build(registry, Settings::default(), None);
    for file in files {
        state.open(sample(file));
    }
    state
}

fn harness_with(state: AppState, picker: FakePicker) -> H {
    let shell = Shell::with_picker(state, Box::new(picker));
    let mut harness = Harness::builder()
        .with_size(egui::vec2(1100.0, 720.0))
        // Room for animations and table columns to settle.
        .with_max_steps(24)
        .build_ui_state(|ui, shell: &mut Shell| shell.ui(ui), shell);
    harness.run();
    harness
}

fn harness(files: &[&str]) -> H {
    harness_with(state(files), FakePicker::default())
}

/// Lets the running command finish and the interface catch up, as time
/// passing would.
fn settle(h: &mut H) {
    // One frame to start the command; no more, since frames drawn while
    // it runs would race with the worker thread.
    h.step();
    h.state_mut().state.wait();
    h.run();
}

/// The menu item with this label. Its accessible name also carries its
/// shortcut ("Undo Ctrl+Z") or submenu arrow, so the label is matched as a
/// prefix. If the same command is also offered elsewhere (Open, on the
/// welcome tab), the one in the menu is the one nearest the menu bar.
fn item<'a>(h: &'a H, label: &'a str) -> egui_kittest::Node<'a> {
    let prefix = format!("{label} ");
    h.get_all_by_label_contains(label)
        .filter(|node| {
            let node = node.accesskit_node();
            let name = node.label().or_else(|| node.value()).unwrap_or_default();
            name == label || name.starts_with(&prefix)
        })
        .min_by(|a, b| a.rect().top().total_cmp(&b.rect().top()))
        .unwrap_or_else(|| panic!("no menu item {label}"))
}

/// Opens a top-level menu and clicks an item in it.
fn choose(h: &mut H, menu: &str, label: &str) {
    h.get_by_role_and_label(Role::Button, menu).click();
    h.run();
    item(h, label).click();
    settle(h);
}

fn pixels(h: &H) -> Vec<u8> {
    let plane = h
        .state()
        .state
        .active()
        .unwrap()
        .image
        .read_plane(0)
        .unwrap();
    plane.as_slice::<u8>().unwrap().to_vec()
}

fn active_name(h: &H) -> String {
    h.state().state.active().unwrap().image.meta().name.clone()
}

fn is_disabled(h: &H, label: &str) -> bool {
    item(h, label).accesskit_node().is_disabled()
}

/// What a node is called: its label, or for plain text its value.
fn name(node: &egui_kittest::Node<'_>) -> String {
    let node = node.accesskit_node();
    node.label().or_else(|| node.value()).unwrap_or_default()
}

fn ctrl(key: &str) -> String {
    let modifier = if cfg!(target_os = "macos") {
        "Cmd"
    } else {
        "Ctrl"
    };
    format!("{modifier}+{key}")
}

// ---- Command palette ----

#[test]
fn palette_opens_by_shortcut_searches_and_runs() {
    let mut h = harness(&["palette.png"]);
    assert!(h.query_by_role(Role::TextInput).is_none());

    h.key_press_modifiers(Modifiers::COMMAND, Key::K);
    h.run();
    assert!(h.state().palette.open, "Ctrl+K opens the palette");
    let search = h.get_by_role(Role::TextInput);
    assert!(search.is_focused(), "the search box takes the keyboard");
    // With no query, every command is listed.
    assert!(h.query_by_label("Reset Layout").is_some());

    search.type_text("inv");
    h.run();
    assert_eq!(h.state().palette.query, "inv");
    assert!(h.query_by_label("Invert").is_some());
    assert!(
        h.query_by_label("Reset Layout").is_none(),
        "non-matches are filtered out"
    );

    h.key_press(Key::Enter);
    settle(&mut h);
    assert!(!h.state().palette.open, "the palette closes after running");
    assert!(h.query_by_role(Role::TextInput).is_none());
    assert_eq!(pixels(&h), [245, 5]);
    assert_eq!(
        h.state().state.status,
        Status::Done("Invert finished".into())
    );
}

#[test]
fn palette_keyboard_navigation_and_escape() {
    let mut h = harness(&[]);
    // The other shortcut opens it too; Esc closes it without running.
    h.key_press_modifiers(Modifiers::COMMAND | Modifiers::SHIFT, Key::P);
    h.run();
    assert!(h.state().palette.open, "Ctrl+Shift+P opens the palette");
    h.key_press(Key::Escape);
    h.run();
    assert!(!h.state().palette.open);

    h.key_press_modifiers(Modifiers::COMMAND, Key::K);
    h.run();
    h.get_by_role(Role::TextInput).type_text("theme");
    h.run();
    let hits: Vec<String> = h
        .state()
        .state
        .search("theme")
        .iter()
        .map(|spec| spec.id.clone())
        .collect();
    assert_eq!(
        &hits[..3],
        ["view.theme_dark", "view.theme_light", "view.theme_system"]
    );
    // Down, down, up: the second hit.
    for key in [Key::ArrowDown, Key::ArrowDown, Key::ArrowUp] {
        h.key_press(key);
        h.run();
    }
    assert_eq!(h.state().palette.selected, 1);
    h.key_press(Key::Enter);
    h.run();
    assert_eq!(h.state().state.settings().theme, Theme::Light);
    assert_eq!(h.ctx.theme(), egui::Theme::Light);
    assert!(!h.ctx.global_style().visuals.dark_mode);
}

#[test]
fn palette_lists_unavailable_commands_with_the_reason() {
    let mut h = harness(&[]);
    h.key_press_modifiers(Modifiers::COMMAND, Key::K);
    h.run();
    h.get_by_role(Role::TextInput).type_text("invert");
    h.run();
    // Listed, but marked as not runnable now.
    assert!(is_disabled(&h, "Invert"));
    h.key_press(Key::Enter);
    h.run();
    assert_eq!(
        h.state().state.status,
        Status::Done("Invert: Open an image first".into())
    );
    assert!(h.query_by_label("Invert: Open an image first").is_some());
}

#[test]
fn shortcuts_do_nothing_while_a_dialog_or_the_palette_is_open() {
    let mut h = harness(&["modal.png"]);
    choose(&mut h, "Process", "Invert");
    assert_eq!(pixels(&h), [245, 5]);

    h.key_press_modifiers(Modifiers::COMMAND, Key::K);
    h.run();
    // Ctrl+Z belongs to the search box now, not to Undo.
    h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    h.run();
    assert_eq!(pixels(&h), [245, 5]);
    h.key_press(Key::Escape);
    h.run();

    h.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    h.run();
    assert_eq!(pixels(&h), [10, 250]);
    h.key_press_modifiers(Modifiers::COMMAND | Modifiers::SHIFT, Key::Z);
    h.run();
    assert_eq!(pixels(&h), [245, 5]);
}

// ---- Parameter dialogs ----

/// The number field that is being typed into.
fn type_number(h: &mut H, label: &str, text: &str) {
    let field = h
        .get_all_by_role(Role::SpinButton)
        .find(|node| {
            node.accesskit_node()
                .labelled_by()
                .any(|l| l.value().as_deref() == Some(label))
        })
        .unwrap_or_else(|| panic!("no number field labelled {label}"));
    field.click();
    h.run();
    // While being edited the field is a text box under the same name.
    let editing = h
        .get_all_by_role(Role::SpinButton)
        .chain(h.get_all_by_role(Role::TextInput))
        .find(|node| node.is_focused())
        .expect("clicking a number field starts editing it");
    editing.type_text(text);
    h.run();
}

#[test]
fn a_dialog_is_generated_for_every_parameter_type() {
    let mut h = harness(&["dialog.png", "dialog-mask.png"]);
    h.get_by_role_and_label(Role::Button, "Demo").click();
    h.run();
    h.get_by_label("Parameter Gallery…").click();
    h.run();
    assert_eq!(
        h.state().dialog.as_ref().unwrap().spec.id,
        "demo.parameters"
    );
    assert!(h.query_by_label("Parameter Gallery").is_some(), "the title");
    assert!(
        h.query_by_label_contains("Takes one parameter of every type")
            .is_some()
    );

    // One control per parameter, by type.
    for label in [
        "Image",
        "Radius",
        "Count",
        "Title",
        "Interpolation",
        "Report file",
        "Weights",
    ] {
        assert!(
            h.query_by_role_and_label(Role::Label, label).is_some(),
            "no label {label}"
        );
    }
    // bool: a checkbox carrying its own label.
    let normalize = h.get_by_role_and_label(Role::CheckBox, "Normalize result");
    assert_eq!(normalize.accesskit_node().toggled(), Some(Toggled::True));
    // int and float: number fields; the bounded one also has a slider.
    assert_eq!(h.get_all_by_role(Role::SpinButton).count(), 2);
    assert_eq!(h.get_all_by_role(Role::Slider).count(), 1);
    // string, path and list: text fields; the path has a browse button.
    assert_eq!(h.get_all_by_role(Role::TextInput).count(), 3);
    assert!(h.query_by_role_and_label(Role::Button, "Browse…").is_some());
    // image and choice: drop-downs, the image one on the active image.
    assert_eq!(h.get_all_by_role(Role::ComboBox).count(), 2);
    // Each control is tied to its label, and shows the current choice.
    let chosen: Vec<(String, Option<String>)> = h
        .get_all_by_role(Role::ComboBox)
        .map(|n| (name(&n), n.accesskit_node().value()))
        .collect();
    assert_eq!(
        chosen,
        [
            ("Image".to_string(), Some("dialog-mask.png".to_string())),
            ("Interpolation".to_string(), Some("Bilinear".to_string()))
        ]
    );
    // The unit follows its field.
    assert!(h.query_by_label("pixels").is_some());
    // Advanced parameters are folded away until asked for.
    assert!(h.query_by_label("Iterations").is_none());
    h.get_by_label("More options").click();
    h.run();
    assert!(h.query_by_label("Iterations").is_some());
    assert!(h.query_by_label("Mask").is_some());
    assert_eq!(h.get_all_by_role(Role::SpinButton).count(), 3);
    assert_eq!(h.get_all_by_role(Role::ComboBox).count(), 3);

    for button in ["OK", "Cancel", "Reset"] {
        assert!(
            h.query_by_role_and_label(Role::Button, button).is_some(),
            "{button}"
        );
    }
    assert!(!is_disabled(&h, "OK"));
}

#[test]
fn an_invalid_value_is_marked_and_disables_ok() {
    let mut h = harness(&["invalid.png"]);
    h.get_by_role_and_label(Role::Button, "Demo").click();
    h.run();
    h.get_by_label("Parameter Gallery…").click();
    h.run();

    type_number(&mut h, "Radius", "-5");
    h.key_press(Key::Tab);
    h.run();
    let problem = "⚠ Must be at least 0";
    assert!(
        h.query_by_label(problem).is_some(),
        "the field says what is wrong"
    );
    assert!(is_disabled(&h, "OK"));
    // Enter does not get round it.
    h.key_press(Key::Enter);
    h.run();
    assert!(h.state().dialog.is_some());
    assert!(h.state().state.running().is_none());

    type_number(&mut h, "Radius", "4");
    h.key_press(Key::Tab);
    h.run();
    assert!(h.query_by_label(problem).is_none());
    assert!(!is_disabled(&h, "OK"));

    h.get_by_role_and_label(Role::CheckBox, "Normalize result")
        .click();
    h.run();
    h.get_by_role_and_label(Role::Button, "OK").click();
    settle(&mut h);
    assert!(h.state().dialog.is_none(), "OK closes the dialog");

    // The command ran with what was entered, and said so in the Log.
    let log = h.state().state.log();
    let entry = log.last().expect("the command logged its result");
    assert_eq!(entry.level, LogLevel::Info);
    assert!(entry.text.contains("radius=4.0"), "{}", entry.text);
    assert!(entry.text.contains("normalize=false"), "{}", entry.text);
    assert!(entry.text.contains("count=3"), "{}", entry.text);
    assert!(
        h.query_by_label_contains("radius=4.0").is_some(),
        "the Log panel shows it"
    );

    // Next time the dialog starts from those values; Reset restores the
    // defaults; Cancel runs nothing.
    h.get_by_role_and_label(Role::Button, "Demo").click();
    h.run();
    h.get_by_label("Parameter Gallery…").click();
    h.run();
    let normalize = h.get_by_role_and_label(Role::CheckBox, "Normalize result");
    assert_eq!(normalize.accesskit_node().toggled(), Some(Toggled::False));
    h.get_by_role_and_label(Role::Button, "Reset").click();
    h.run();
    let normalize = h.get_by_role_and_label(Role::CheckBox, "Normalize result");
    assert_eq!(normalize.accesskit_node().toggled(), Some(Toggled::True));
    h.get_by_role_and_label(Role::Button, "Cancel").click();
    h.run();
    assert!(h.state().dialog.is_none());
    assert_eq!(h.state().state.log().len(), 1);
}

#[test]
fn browse_fills_a_path_field_from_the_file_dialog() {
    let picker = FakePicker::default();
    let report = temp("report.txt");
    picker.answers.borrow_mut().push_back(report.clone());
    let mut h = harness_with(state(&["browse.png"]), picker.clone());
    h.get_by_role_and_label(Role::Button, "Demo").click();
    h.run();
    h.get_by_label("Parameter Gallery…").click();
    h.run();
    h.get_by_role_and_label(Role::Button, "Browse…").click();
    h.run();
    assert_eq!(picker.asked.borrow()[0].mode, PathMode::Save);
    h.key_press(Key::Enter);
    settle(&mut h);
    let entry = h.state().state.log().last().unwrap().text.clone();
    assert!(
        entry.contains(&format!("report={}", report.display())),
        "{entry}"
    );
}

#[test]
fn file_open_goes_straight_to_the_file_dialog() {
    let picker = FakePicker::default();
    let file = sample("picked.png");
    picker.answers.borrow_mut().push_back(file.clone());
    let mut h = harness_with(state(&[]), picker.clone());
    assert!(
        h.query_by_label("Drop image files here to open them")
            .is_some()
    );

    choose(&mut h, "File", "Open…");
    assert!(
        h.state().dialog.is_none(),
        "no parameter dialog for a lone path"
    );
    let asked = picker.asked.borrow();
    assert_eq!(asked.len(), 1);
    assert_eq!(asked[0].mode, PathMode::Open);
    assert_eq!(asked[0].title, "Open");
    assert_eq!(active_name(&h), "picked.png");
    assert!(
        h.query_by_role_and_label(Role::Button, "picked.png")
            .is_some(),
        "its tab"
    );
    assert!(
        h.query_by_label("Drop image files here to open them")
            .is_none()
    );

    // It is now a recent file, which opens with no dialog at all.
    drop(asked);
    h.get_by_role_and_label(Role::Button, "File").click();
    h.run();
    let tab = h.get_by_role_and_label(Role::Button, "picked.png").rect();
    item(&h, "Open Recent").click();
    h.run();
    h.get_all_by_role_and_label(Role::Button, "picked.png")
        .find(|node| node.rect() != tab)
        .expect("the file is listed under Open Recent")
        .click();
    settle(&mut h);
    assert_eq!(picker.asked.borrow().len(), 1);
    assert_eq!(h.state().state.session().workspace.len(), 2);

    // Dismissing the file dialog runs nothing.
    h.key_press_modifiers(Modifiers::COMMAND, Key::O);
    settle(&mut h);
    assert_eq!(picker.asked.borrow().len(), 2);
    assert_eq!(h.state().state.session().workspace.len(), 2);
}

// ---- Panels and tabs ----

/// Clicks the close button on the tab with this title.
fn close_tab(h: &mut H, title: &str) {
    let tab = h.get_by_role_and_label(Role::Button, title).rect();
    let close = h
        .get_all_by_role_and_label(Role::Button, "Close")
        .find(|node| tab.contains(node.rect().center()))
        .expect("a close button on the tab");
    close.click();
    h.run();
}

#[test]
fn a_panel_closes_and_reopens_in_place_from_the_window_menu() {
    let mut h = harness(&[]);
    let content = "No image is open";
    assert!(
        h.query_by_role_and_label(Role::Button, "Info").is_some(),
        "tab"
    );
    assert!(h.query_by_label(content).is_some(), "contents");
    let place = h.get_by_role_and_label(Role::Button, "Info").rect();

    close_tab(&mut h, "Info");
    assert!(
        h.query_by_role_and_label(Role::Button, "Info").is_none(),
        "tab gone"
    );
    assert!(h.query_by_label(content).is_none(), "contents gone");
    assert!(!h.state().state.panel_open(Panel::Info));
    // The other panels are untouched.
    assert!(h.query_by_role_and_label(Role::Button, "History").is_some());

    // The Window menu shows it unticked; ticking brings it back where it was.
    h.get_by_role_and_label(Role::Button, "Window").click();
    h.run();
    let item = h.get_by_role_and_label(Role::CheckBox, "Info Panel");
    assert_eq!(item.accesskit_node().toggled(), Some(Toggled::False));
    let other = h.get_by_role_and_label(Role::CheckBox, "Log Panel");
    assert_eq!(other.accesskit_node().toggled(), Some(Toggled::True));
    item.click();
    h.run();
    assert!(h.state().state.panel_open(Panel::Info));
    assert!(h.query_by_label(content).is_some(), "contents are back");
    assert_eq!(h.get_by_role_and_label(Role::Button, "Info").rect(), place);

    // The same menu item hides it again, and Reset Layout restores it.
    choose(&mut h, "Window", "Info Panel");
    assert!(h.query_by_label(content).is_none());
    choose(&mut h, "Window", "Log Panel");
    choose(&mut h, "Window", "Reset Layout");
    assert!(h.query_by_label(content).is_some());
    assert!(h.query_by_role_and_label(Role::Button, "Log").is_some());
    assert_eq!(h.get_by_role_and_label(Role::Button, "Info").rect(), place);
}

#[test]
fn choosing_an_image_tab_makes_that_image_active() {
    let mut h = harness(&["first.png", "second.png"]);
    assert_eq!(active_name(&h), "second.png");
    assert!(h.query_by_label("View of second.png").is_some());
    assert!(
        h.query_by_label("View of first.png").is_none(),
        "hidden behind the other tab"
    );

    h.get_by_role_and_label(Role::Button, "first.png").click();
    h.run();
    assert_eq!(active_name(&h), "first.png");
    assert!(h.query_by_label("View of first.png").is_some());
    // The Info panel and the status bar follow the active image.
    assert!(h.get_all_by_label_contains("first.png").count() >= 3);

    // Commands act on the chosen image.
    choose(&mut h, "Process", "Invert");
    let workspace = &h.state().state.session().workspace;
    let planes: Vec<Vec<u8>> = workspace
        .datasets()
        .map(|d| {
            d.image
                .read_plane(0)
                .unwrap()
                .as_slice::<u8>()
                .unwrap()
                .to_vec()
        })
        .collect();
    assert_eq!(planes, [vec![245, 5], vec![10, 250]]);

    // Closing a tab runs Close on that image, not on the active one.
    close_tab(&mut h, "second.png");
    settle(&mut h);
    assert_eq!(h.state().state.session().workspace.len(), 1);
    assert_eq!(active_name(&h), "first.png");
    assert!(
        h.query_by_role_and_label(Role::Button, "second.png")
            .is_none()
    );
    // And it can be undone, which brings the tab back.
    choose(&mut h, "Edit", "Undo");
    assert!(
        h.query_by_role_and_label(Role::Button, "second.png")
            .is_some()
    );
}

#[test]
fn undo_and_redo_from_the_edit_menu() {
    let mut h = harness(&["undo.png"]);
    h.get_by_role_and_label(Role::Button, "Edit").click();
    h.run();
    // Opening a file is undoable, so Undo is available; Redo is not yet.
    assert!(!is_disabled(&h, "Undo"));
    assert!(is_disabled(&h, "Redo"));
    h.key_press(Key::Escape);
    h.run();

    choose(&mut h, "Process", "Invert");
    assert_eq!(pixels(&h), [245, 5]);
    choose(&mut h, "Edit", "Undo");
    assert_eq!(pixels(&h), [10, 250]);
    assert!(
        h.query_by_label("Undid Invert").is_some(),
        "the status bar says so"
    );
    choose(&mut h, "Edit", "Redo");
    assert_eq!(pixels(&h), [245, 5]);
    assert!(h.query_by_label("Redid Invert").is_some());

    // The History panel lists what was run.
    assert!(h.query_by_label("1. Open").is_some());
    assert!(h.query_by_label("2. Invert").is_some());
}

#[test]
fn menus_show_shortcuts_and_disable_what_cannot_run() {
    let mut h = harness(&[]);
    h.get_by_role_and_label(Role::Button, "File").click();
    h.run();
    let disabled = |label: String| h.get_by_label(&label).accesskit_node().is_disabled();
    assert!(!disabled(format!("Open… {}", ctrl("O"))));
    assert!(disabled(format!("Save As… {}", ctrl("Shift+S"))));
    assert!(disabled(format!("Close {}", ctrl("W"))));
}

/// A file dragged onto the window.
#[derive(Debug)]
struct Dropped(PathBuf);

impl egui::DroppedFile for Dropped {
    fn path(&self) -> &std::path::Path {
        &self.0
    }

    fn bytes(&self) -> Result<Vec<u8>, String> {
        std::fs::read(&self.0).map_err(|e| e.to_string())
    }
}

#[test]
fn dropped_files_open_as_tabs() {
    let mut h = harness(&[]);
    for name in ["dropped-a.png", "dropped-b.png"] {
        h.input_mut()
            .dropped_files
            .push(std::sync::Arc::new(Dropped(sample(name))));
    }
    settle(&mut h);
    assert!(
        h.query_by_role_and_label(Role::Button, "dropped-a.png")
            .is_some()
    );
    assert!(
        h.query_by_role_and_label(Role::Button, "dropped-b.png")
            .is_some()
    );
    assert_eq!(active_name(&h), "dropped-b.png");
    assert!(h.query_by_role_and_label(Role::Button, "Welcome").is_none());
}

// ---- Feedback ----

#[test]
fn errors_show_in_the_status_bar_and_the_log() {
    let picker = FakePicker::default();
    let missing = temp("no-such-file.png");
    picker.answers.borrow_mut().push_back(missing);
    let mut h = harness_with(state(&[]), picker);
    choose(&mut h, "File", "Open…");

    let Status::Failed(message) = h.state().state.status.clone() else {
        panic!("expected a failure, got {:?}", h.state().state.status);
    };
    assert!(message.contains("no-such-file.png"), "{message}");
    // Status bar and Log both carry the text, marked as a problem.
    h.get_by_role_and_label(Role::Button, "Log").click();
    h.run();
    assert_eq!(h.get_all_by_label(&format!("⚠ {message}")).count(), 2);
}

#[test]
fn the_layout_is_remembered_between_sessions() {
    let mut h = harness(&[]);
    close_tab(&mut h, "Info");
    h.state_mut().persist();
    let settings = h.state().state.settings().clone();
    let saved = settings.layout.clone().expect("the layout is saved");
    assert_eq!(saved.version, imager_app::gui::layout::Layout::VERSION);

    // The next session starts with Info still closed.
    let next = AppState::build(imager_commands::standard_registry(), settings.clone(), None);
    let mut h = harness_with(next, FakePicker::default());
    assert!(!h.state().state.panel_open(Panel::Info));
    assert!(h.query_by_role_and_label(Role::Button, "Info").is_none());
    assert!(h.query_by_role_and_label(Role::Button, "History").is_some());
    choose(&mut h, "Window", "Info Panel");
    assert!(h.query_by_label("No image is open").is_some());

    // A layout that cannot be used gives the default, not an error.
    for broken in [
        imager_app::state::SavedLayout {
            version: saved.version,
            data: "{ not a layout".into(),
        },
        imager_app::state::SavedLayout {
            version: saved.version + 1,
            data: saved.data.clone(),
        },
    ] {
        let mut settings = settings.clone();
        settings.layout = Some(broken);
        let state = AppState::build(imager_commands::standard_registry(), settings, None);
        let h = harness_with(state, FakePicker::default());
        assert!(h.query_by_role_and_label(Role::Button, "Info").is_some());
        assert_eq!(h.state().state.status, Status::Idle);
    }
}
