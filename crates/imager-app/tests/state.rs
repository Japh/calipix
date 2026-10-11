//! Everything the desktop interface can do, exercised without a window.

use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex};

use imager_app::demo;
use imager_app::state::dialog::Dialog;
use imager_app::state::menu::{self, MenuEntry};
use imager_app::state::palette;
use imager_app::state::settings::Settings;
use imager_app::state::shortcut::{KeyPress, Shortcut};
use imager_app::state::{
    AppState, Draft, Invocation, LogLevel, Mark, Panel, Status, Theme, UiAction, Zoom,
};
use imager_command::{
    Args, Command, CommandCtx, CommandSpec, DatasetId, Effects, Error, ImageEffect, Outputs,
    ParamSpec, ParamType, PathMode, Registry, Value, conformance,
};
use imager_core::Image;

fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("imager-state-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(name)
}

fn sample(name: &str) -> PathBuf {
    let path = temp(name);
    imager_io::save(&Image::from_vec(2, 1, vec![10u8, 250]).unwrap(), &path).unwrap();
    path
}

fn pixels(state: &AppState) -> Vec<u8> {
    let plane = state.active().unwrap().image.read_plane(0).unwrap();
    plane.as_slice::<u8>().unwrap().to_vec()
}

fn demo_registry() -> Registry {
    let mut registry = imager_commands::standard_registry();
    demo::register(&mut registry).unwrap();
    registry
}

/// A state that also has the demo commands.
fn demo_state() -> AppState {
    AppState::build(demo_registry(), Settings::default(), None)
}

/// The labels of one menu level; `-` for a separator, `>` after a submenu.
fn labels(items: &[MenuEntry]) -> Vec<String> {
    items
        .iter()
        .map(|item| match item {
            MenuEntry::Separator => "-".to_string(),
            MenuEntry::Submenu { title, .. } => format!("{title} >"),
            MenuEntry::Command(command) => command.label.clone(),
        })
        .collect()
}

fn submenu<'a>(items: &'a [MenuEntry], name: &str) -> &'a [MenuEntry] {
    items
        .iter()
        .find_map(|item| match item {
            MenuEntry::Submenu { title, items } if title == name => Some(items.as_slice()),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no menu {name}"))
}

// ---- Menus ----

#[test]
fn menus_follow_the_prescribed_order_and_grouping() {
    let state = demo_state();
    // Missing menus (Analyze, Extensions) are absent; a menu the design
    // does not name goes before Window.
    assert_eq!(
        labels(state.menu()),
        [
            "File >",
            "Edit >",
            "View >",
            "Image >",
            "Process >",
            "Demo >",
            "Window >",
            "Help >"
        ]
    );
    assert_eq!(
        labels(submenu(state.menu(), "File")),
        ["Open…", "Open Recent >", "Save", "Save As…", "Close"]
    );
    assert_eq!(labels(submenu(state.menu(), "Edit")), ["Undo", "Redo"]);
    // A separator wherever the weight crosses into another hundred.
    assert_eq!(
        labels(submenu(state.menu(), "View")),
        [
            "Zoom In",
            "Zoom Out",
            "Fit to Window",
            "Zoom to 100%",
            "-",
            "Previous Plane",
            "Next Plane",
            "-",
            "Set Display Range…",
            "Auto Contrast",
            "Enhance Contrast…",
            "Reset Display Range",
            "-",
            "Set Lookup Table…",
            "Invert Lookup Table",
            "-",
            "Set Channel Mode…",
            "-",
            "Theme >"
        ]
    );
    assert_eq!(
        labels(submenu(submenu(state.menu(), "View"), "Theme")),
        ["System Theme", "Light Theme", "Dark Theme"]
    );
    assert_eq!(
        labels(submenu(state.menu(), "Window")),
        [
            "Info Panel",
            "Display Panel",
            "Histogram Panel",
            "History Panel",
            "Log Panel",
            "-",
            "Show Images Side by Side",
            "Reset Layout"
        ]
    );
    assert_eq!(
        labels(submenu(state.menu(), "Help")),
        ["Command Palette", "-", "About ImageR2"]
    );
}

#[test]
fn an_ellipsis_marks_commands_that_ask_for_something() {
    let state = demo_state();
    let label = |id: &str| menu::label(state.spec(id).unwrap());
    // A file path is asked for, in the system's file dialog.
    assert_eq!(label("file.open"), "Open…");
    assert_eq!(label("file.save_as"), "Save As…");
    assert_eq!(label("demo.parameters"), "Parameter Gallery…");
    // The only image parameter is the active image: nothing to ask.
    assert_eq!(label("image.invert"), "Invert");
    assert_eq!(label("file.close"), "Close");
    assert_eq!(label("edit.undo"), "Undo");
}

#[test]
fn commands_explain_why_they_are_unavailable() {
    let mut state = AppState::new();
    let reason = |state: &AppState, id: &str| state.unavailable_reason(id);
    assert_eq!(
        reason(&state, "image.invert").as_deref(),
        Some("Open an image first")
    );
    assert_eq!(
        reason(&state, "file.save_as").as_deref(),
        Some("Open an image first")
    );
    assert_eq!(
        reason(&state, "edit.undo").as_deref(),
        Some("Nothing to undo")
    );
    assert_eq!(
        reason(&state, "edit.redo").as_deref(),
        Some("Nothing to redo")
    );
    assert_eq!(
        reason(&state, "view.zoom_in").as_deref(),
        Some("Open an image first")
    );
    assert_eq!(
        reason(&state, "file.clear_recent").as_deref(),
        Some("No recent files")
    );
    assert!(reason(&state, "no.such").is_some());
    // Needing a value from the person does not make a command unavailable.
    assert_eq!(reason(&state, "file.open"), None);
    assert_eq!(reason(&state, "window.toggle_info"), None);

    state.open(sample("available.png"));
    for id in [
        "image.invert",
        "file.save_as",
        "view.zoom_in",
        "file.clear_recent",
    ] {
        assert_eq!(reason(&state, id), None, "{id}");
    }
    assert_eq!(reason(&state, "edit.undo"), None);

    // Choosing an unavailable command says why and does nothing.
    let mut empty = AppState::new();
    assert_eq!(
        empty.invoke("image.invert", &Args::new()),
        Invocation::Unavailable("Open an image first".into())
    );
    assert_eq!(
        empty.status,
        Status::Done("Invert: Open an image first".into())
    );
    assert!(empty.log().is_empty());
}

// ---- Running, undo, feedback ----

#[test]
fn open_run_undo_redo() {
    let mut state = AppState::new();
    assert!(state.active().is_none());
    assert!(!state.can_undo());

    state.open(sample("flow.png"));
    assert_eq!(state.status, Status::Done("Open finished".into()));
    assert_eq!(pixels(&state), [10, 250]);

    state.run("image.invert", &Args::new());
    assert_eq!(state.status, Status::Done("Invert finished".into()));
    assert_eq!(pixels(&state), [245, 5]);

    state.undo();
    assert_eq!(state.status, Status::Done("Undid Invert".into()));
    assert_eq!(pixels(&state), [10, 250]);
    assert!(state.can_redo());

    state.redo();
    assert_eq!(state.status, Status::Done("Redid Invert".into()));
    assert_eq!(pixels(&state), [245, 5]);

    // Undo and redo are not themselves history.
    let titles: Vec<_> = state
        .session()
        .history()
        .entries()
        .iter()
        .map(|e| e.title.as_str())
        .collect();
    assert_eq!(titles, ["Open", "Invert"]);
}

#[test]
fn failures_are_shown_logged_and_harmless() {
    let mut state = AppState::new();
    state.open(temp("missing.png"));
    let Status::Failed(message) = &state.status else {
        panic!("expected a failure, got {:?}", state.status);
    };
    assert!(message.starts_with("Open: "), "{message}");
    assert!(message.contains("missing.png"), "{message}");
    assert!(state.active().is_none());
    assert!(state.session().history().entries().is_empty());

    let [entry] = state.log() else {
        panic!("expected one log entry, got {:?}", state.log());
    };
    assert_eq!(entry.level, LogLevel::Error);
    assert_eq!(&entry.text, message);

    state.undo();
    assert_eq!(state.status, Status::Done("Undo: Nothing to undo".into()));
    assert_eq!(state.log().len(), 1);
}

#[test]
fn results_that_are_not_images_go_to_the_log() {
    let mut state = AppState::new();
    state.open(sample("info.png"));
    assert!(state.log().is_empty());
    state.take_ui_actions();

    state.run("image.info", &Args::new());
    let [entry] = state.log() else {
        panic!("expected one log entry, got {:?}", state.log());
    };
    assert_eq!(entry.level, LogLevel::Info);
    assert!(entry.text.starts_with("Image Info: "), "{}", entry.text);
    assert!(entry.text.contains("width 2"), "{}", entry.text);
    assert!(entry.text.contains("pixel type u8"), "{}", entry.text);
    assert_eq!(
        state.status,
        Status::Done("Image Info finished; its results are in the Log".into())
    );
    // The Log panel is brought into view.
    assert_eq!(state.take_ui_actions(), [UiAction::ShowPanel(Panel::Log)]);
}

// ---- Shortcuts ----

#[test]
fn shortcuts_parse_like_the_command_system_validates_them() {
    let good = [
        "Ctrl+O",
        "Ctrl+Shift+S",
        "F5",
        "+",
        "Ctrl++",
        "0",
        "Alt+Left",
        "Delete",
        "-",
    ];
    let bad = [
        "ctrl+o",
        "Ctrl+",
        "Ctrl+Ctrl+O",
        "Cmd+O",
        "Ctrl+Open",
        "",
        "F13",
        "o",
        "+Ctrl+O",
    ];
    for text in good.iter().chain(&bad) {
        let valid = CommandSpec::new("a.b", "B")
            .shortcut(*text)
            .validate()
            .is_ok();
        assert_eq!(Shortcut::parse(text).is_ok(), valid, "{text}");
    }
    assert_eq!(
        Shortcut::parse("Ctrl+Shift+S").unwrap(),
        Shortcut {
            ctrl: true,
            shift: true,
            alt: false,
            key: "S".into()
        }
    );
    assert_eq!(Shortcut::parse("Ctrl++").unwrap().key, "+");
    assert!(Shortcut::parse("Ctrl++").unwrap().ctrl);

    // Ctrl is Cmd on macOS.
    let save_as = Shortcut::parse("Ctrl+Shift+S").unwrap();
    assert_eq!(save_as.display(false), "Ctrl+Shift+S");
    assert_eq!(save_as.display(true), "Cmd+Shift+S");
    assert_eq!(Shortcut::parse("+").unwrap().display(true), "+");
}

fn press(text: &str) -> KeyPress {
    let shortcut = Shortcut::parse(text).unwrap();
    KeyPress {
        ctrl: shortcut.ctrl,
        shift: shortcut.shift,
        alt: shortcut.alt,
        key: shortcut.key,
    }
}

#[test]
fn shortcuts_come_from_command_specs() {
    let state = AppState::new();
    let command = |text: &str| state.shortcut_command(&press(text));
    assert_eq!(command("Ctrl+O"), Some("file.open"));
    assert_eq!(command("Ctrl+Shift+S"), Some("file.save_as"));
    assert_eq!(command("Ctrl+W"), Some("file.close"));
    assert_eq!(command("Ctrl+Z"), Some("edit.undo"));
    assert_eq!(command("Ctrl+Shift+Z"), Some("edit.redo"));
    assert_eq!(command("Ctrl+K"), Some("help.command_palette"));
    assert_eq!(command("Ctrl+Shift+P"), Some("help.command_palette"));
    assert_eq!(command("+"), Some("view.zoom_in"));
    assert_eq!(command("-"), Some("view.zoom_out"));
    assert_eq!(command("0"), Some("view.zoom_fit"));
    assert_eq!(command("1"), Some("view.zoom_actual"));
    // `+` is Shift+`=` on many keyboards; either way of reaching it works.
    assert_eq!(command("Shift++"), Some("view.zoom_in"));
    assert_eq!(command("="), Some("view.zoom_in"));
    // Modifiers must match exactly.
    assert_eq!(command("Z"), None);
    assert_eq!(command("Ctrl+Alt+Z"), None);
    assert_eq!(command("Ctrl+1"), None);
    assert_eq!(command("Shift+1"), None);
    assert_eq!(command("Ctrl+P"), None);

    // Every shortcut a spec declares reaches its own command.
    for spec in state.registry().specs() {
        if let Some(shortcut) = &spec.shortcut {
            assert_eq!(command(shortcut), Some(spec.id.as_str()), "{shortcut}");
            assert!(state.shortcut_text(&spec.id).is_some());
        }
    }
    assert_eq!(state.shortcut_text("image.invert"), None);
}

// ---- Command palette ----

fn ranked<'a>(state: &'a AppState, query: &str) -> Vec<&'a str> {
    state.search(query).iter().map(|s| s.id.as_str()).collect()
}

#[test]
fn palette_ranks_prefix_and_word_start_matches_first() {
    let state = demo_state();
    let first = |query: &str| ranked(&state, query).first().copied();

    // Title: whole, prefix, word start, initials.
    assert_eq!(first("invert"), Some("image.invert"));
    assert_eq!(first("inv"), Some("image.invert"));
    assert_eq!(first("blur"), None);
    assert_eq!(first("palette"), Some("help.command_palette"));
    assert_eq!(first("zi"), Some("view.zoom_in"));
    // Scattered letters still find it.
    assert_eq!(first("gllry"), Some("demo.parameters"));
    // Ids, ImageJ names and summaries are searched too.
    assert_eq!(first("file.cl"), Some("file.close"));
    assert_eq!(first("scale to fit"), Some("view.zoom_fit"));
    assert_eq!(first("view 100"), Some("view.zoom_actual"));
    assert_eq!(first("dark to bright"), Some("image.invert"));
    // Words in any order, each matching somewhere.
    assert_eq!(first("theme dark"), Some("view.theme_dark"));
    assert_eq!(first("panel hist"), Some("window.toggle_history"));
    assert_eq!(first("xyzzy"), None);

    // "re": titles starting with it, then titles with a word starting with
    // it, before anything that merely contains it.
    let re = ranked(&state, "re");
    let at = |id: &str| re.iter().position(|hit| *hit == id).unwrap_or(usize::MAX);
    assert_eq!(&re[..2], ["edit.redo", "window.reset_layout"]);
    assert!(at("file.clear_recent") < at("image.invert"), "{re:?}");
    // "Open" itself beats "Open Recent"'s command and summary matches.
    assert_eq!(ranked(&state, "open")[0], "file.open");
    // A title match beats a summary match of the same kind.
    let save = ranked(&state, "save");
    assert_eq!(&save[..2], ["file.save", "file.save_as"]);

    // No query: everything, by title.
    let all = state.search("");
    assert_eq!(all.len(), state.registry().len());
    let titles: Vec<_> = all.iter().map(|s| s.title.as_str()).collect();
    let mut sorted = titles.clone();
    sorted.sort();
    assert_eq!(titles, sorted);
}

#[test]
fn palette_scores_order_the_match_kinds() {
    let spec = CommandSpec::new("filter.gaussian_blur", "Gaussian Blur")
        .summary("Smooths an image.")
        .ijm_name("Gaussian Blur...");
    let score = |query: &str| palette::score(query, &spec);
    let whole = score("gaussian blur").unwrap();
    let prefix = score("gauss").unwrap();
    let word = score("blur").unwrap();
    let initials = score("gb").unwrap();
    let inside = score("ssian").unwrap();
    let scattered = score("gsblr").unwrap();
    assert!(
        whole > prefix && prefix > word && word > initials,
        "{whole} {prefix} {word}"
    );
    assert!(
        initials > inside && inside > scattered,
        "{initials} {inside} {scattered}"
    );
    // A summary only counts for real substrings, and for less than a title.
    assert!(score("smooths").unwrap() < inside);
    assert_eq!(score("smths"), None);
    assert_eq!(score("  "), Some(0));
}

// ---- Application commands ----

#[test]
fn interface_operations_are_commands() {
    let mut state = AppState::new();
    assert_eq!(conformance::check(state.registry()), Vec::<String>::new());

    let ui_commands = [
        "edit.undo",
        "edit.redo",
        "view.zoom_in",
        "view.zoom_out",
        "view.zoom_fit",
        "view.zoom_actual",
        "view.theme_system",
        "view.theme_light",
        "view.theme_dark",
        "window.toggle_info",
        "window.toggle_history",
        "window.toggle_log",
        "window.reset_layout",
        "help.command_palette",
        "help.about",
        "file.clear_recent",
    ];
    for id in ui_commands {
        let spec = state
            .spec(id)
            .unwrap_or_else(|| panic!("{id} is not registered"));
        assert!(!spec.flags.headless_ok, "{id} must not be headless");
        assert!(!spec.flags.recordable, "{id} must not be recorded");
        assert!(spec.menu.is_some(), "{id} is in no menu");
        assert!(spec.summary.ends_with('.'), "{id}");
    }

    state.open(sample("actions.png"));
    state.take_ui_actions();
    let act = |state: &mut AppState, id: &str| {
        state.run(id, &Args::new());
        state.take_ui_actions()
    };
    assert_eq!(act(&mut state, "view.zoom_in"), [UiAction::Zoom(Zoom::In)]);
    assert_eq!(
        act(&mut state, "view.zoom_out"),
        [UiAction::Zoom(Zoom::Out)]
    );
    assert_eq!(
        act(&mut state, "view.zoom_fit"),
        [UiAction::Zoom(Zoom::Fit)]
    );
    assert_eq!(
        act(&mut state, "view.zoom_actual"),
        [UiAction::Zoom(Zoom::Actual)]
    );
    assert_eq!(
        act(&mut state, "help.command_palette"),
        [UiAction::OpenPalette]
    );
    assert_eq!(act(&mut state, "help.about"), [UiAction::About]);

    // Panels: the command toggles, and the menu mark follows.
    assert_eq!(state.mark("window.toggle_info"), Some(Mark::Toggle(true)));
    assert_eq!(
        act(&mut state, "window.toggle_info"),
        [UiAction::TogglePanel(Panel::Info)]
    );
    assert!(!state.panel_open(Panel::Info));
    assert_eq!(state.mark("window.toggle_info"), Some(Mark::Toggle(false)));
    assert_eq!(state.mark("window.toggle_log"), Some(Mark::Toggle(true)));
    assert_eq!(
        act(&mut state, "window.reset_layout"),
        [UiAction::ResetLayout]
    );
    assert!(state.panel_open(Panel::Info));

    // Theme: one of three, marked as a choice.
    assert_eq!(state.settings().theme, Theme::System);
    assert_eq!(state.mark("view.theme_system"), Some(Mark::Choice(true)));
    assert_eq!(
        act(&mut state, "view.theme_dark"),
        [UiAction::SetTheme(Theme::Dark)]
    );
    assert_eq!(state.settings().theme, Theme::Dark);
    assert_eq!(state.mark("view.theme_dark"), Some(Mark::Choice(true)));
    assert_eq!(state.mark("view.theme_system"), Some(Mark::Choice(false)));
    assert_eq!(state.mark("image.invert"), None);

    // None of this is history.
    assert_eq!(state.session().history().entries().len(), 1);
}

// ---- Choosing commands: straight away, file dialog, or parameter dialog ----

#[test]
fn choosing_a_command_asks_only_for_what_is_missing() {
    let mut state = demo_state();

    // A lone file path goes straight to the system's file dialog.
    let Invocation::PickPath {
        request,
        command,
        param,
        ..
    } = state.invoke("file.open", &Args::new())
    else {
        panic!("Open should ask for a file");
    };
    assert_eq!((command.as_str(), param.as_str()), ("file.open", "path"));
    assert_eq!(request.mode, PathMode::Open);
    assert_eq!(request.title, "Open");
    assert_eq!(request.filters[0].0, "Images");
    assert!(request.filters[0].1.contains(&"png".to_string()));
    assert!(request.filters[0].1.contains(&"tif".to_string()));

    let path = sample("chosen.png");
    state.picked(&command, &param, &Args::new(), path.clone());
    state.wait();
    assert_eq!(
        state.active().unwrap().path.as_deref(),
        Some(path.as_path())
    );
    // The next file dialog starts in the same folder.
    assert_eq!(state.settings().last_dir.as_deref(), path.parent());

    // Nothing missing: runs at once.
    assert_eq!(
        state.invoke("image.invert", &Args::new()),
        Invocation::Started
    );
    state.wait();
    assert_eq!(pixels(&state), [245, 5]);

    // Save As: the image is the active one, so only the path is asked for,
    // with the image's name suggested.
    let Invocation::PickPath { request, param, .. } = state.invoke("file.save_as", &Args::new())
    else {
        panic!("Save As should ask for a file");
    };
    assert_eq!(param, "path");
    assert_eq!(request.mode, PathMode::Save);
    assert_eq!(request.file_name.as_deref(), Some("chosen.png"));
    assert_eq!(request.directory.as_deref(), path.parent());

    // Several things to decide: a dialog.
    let Invocation::Dialog(dialog) = state.invoke("demo.parameters", &Args::new()) else {
        panic!("Parameter Gallery should open a dialog");
    };
    assert_eq!(dialog.spec.id, "demo.parameters");

    // Everything already given, as for a recent file: runs at once.
    let given = Args::new().with("path", sample("given.png"));
    assert_eq!(state.invoke("file.open", &given), Invocation::Started);
    state.wait();
    assert_eq!(state.session().workspace.len(), 2);
}

// ---- Parameter dialogs ----

fn gallery(state: &mut AppState) -> Dialog {
    match state.invoke("demo.parameters", &Args::new()) {
        Invocation::Dialog(dialog) => *dialog,
        other => panic!("expected a dialog, got {other:?}"),
    }
}

fn draft<'a>(dialog: &'a mut Dialog, name: &str) -> &'a mut Draft {
    &mut dialog.field_mut(name).unwrap().draft
}

#[test]
fn dialogs_start_from_defaults_and_the_active_image() {
    let mut state = demo_state();
    state.open(sample("first.png"));
    state.open(sample("second.png"));
    let active = state.active().unwrap().id;
    let mut dialog = gallery(&mut state);

    let names: Vec<_> = dialog
        .fields
        .iter()
        .map(|f| f.param.name.as_str())
        .collect();
    assert_eq!(
        names,
        [
            "input",
            "radius",
            "count",
            "normalize",
            "title",
            "method",
            "report",
            "weights",
            "mask",
            "iterations"
        ]
    );
    assert_eq!(*draft(&mut dialog, "input"), Draft::Image(Some(active)));
    assert_eq!(*draft(&mut dialog, "radius"), Draft::Float(2.0));
    assert_eq!(*draft(&mut dialog, "count"), Draft::Int(3));
    assert_eq!(*draft(&mut dialog, "normalize"), Draft::Bool(true));
    assert_eq!(*draft(&mut dialog, "title"), Draft::Text("Result".into()));
    assert_eq!(
        *draft(&mut dialog, "method"),
        Draft::Text("Bilinear".into())
    );
    assert_eq!(*draft(&mut dialog, "report"), Draft::Text(String::new()));
    assert_eq!(
        *draft(&mut dialog, "weights"),
        Draft::Text("1.0, 2.0, 1.0".into())
    );
    assert!(dialog.has_advanced());
    assert!(!dialog.show_advanced);

    let workspace = &state.session().workspace;
    assert!(dialog.is_valid(workspace));
    let args = dialog.args(workspace).unwrap();
    assert_eq!(args.get("input"), Some(&Value::Image(active)));
    assert_eq!(args.get("radius"), Some(&Value::Float(2.0)));
    assert_eq!(args.get("count"), Some(&Value::Int(3)));
    assert_eq!(args.get("method"), Some(&Value::from("Bilinear")));
    assert_eq!(
        args.get("weights"),
        Some(&Value::List(vec![
            Value::Float(1.0),
            Value::Float(2.0),
            Value::Float(1.0)
        ]))
    );
    // Optional parameters left empty are simply not passed.
    assert_eq!(args.get("report"), None);
    // An optional image still defaults to the active one, as the executor
    // would fill it in.
    assert_eq!(args.get("mask"), Some(&Value::Image(active)));
}

#[test]
fn dialogs_mark_invalid_values_and_refuse_them() {
    let mut state = demo_state();
    state.open(sample("validate.png"));
    let mut dialog = gallery(&mut state);
    let problem = |dialog: &mut Dialog, state: &AppState, name: &str| {
        dialog
            .field_mut(name)
            .unwrap()
            .problem(&state.session().workspace)
    };

    *draft(&mut dialog, "radius") = Draft::Float(-1.0);
    assert_eq!(
        problem(&mut dialog, &state, "radius").as_deref(),
        Some("Must be at least 0")
    );
    assert!(!dialog.is_valid(&state.session().workspace));
    assert_eq!(dialog.args(&state.session().workspace), None);
    // OK does nothing while a value is invalid.
    assert!(!state.accept(&dialog));
    assert!(state.running().is_none());
    assert!(state.settings().remembered.is_empty());

    *draft(&mut dialog, "radius") = Draft::Float(0.5);
    assert_eq!(problem(&mut dialog, &state, "radius"), None);

    *draft(&mut dialog, "count") = Draft::Int(11);
    assert_eq!(
        problem(&mut dialog, &state, "count").as_deref(),
        Some("Must be between 1 and 10")
    );
    *draft(&mut dialog, "count") = Draft::Int(10);

    *draft(&mut dialog, "weights") = Draft::Text("1, two, 3".into());
    assert_eq!(
        problem(&mut dialog, &state, "weights").as_deref(),
        Some("“two” is not a number")
    );
    *draft(&mut dialog, "weights") = Draft::Text(" 1 ,2.5".into());
    assert_eq!(problem(&mut dialog, &state, "weights"), None);

    *draft(&mut dialog, "method") = Draft::Text("Cubic".into());
    assert!(
        problem(&mut dialog, &state, "method")
            .unwrap()
            .contains("not one of")
    );
    *draft(&mut dialog, "method") = Draft::Text("Bicubic".into());

    *draft(&mut dialog, "input") = Draft::Image(None);
    assert_eq!(
        problem(&mut dialog, &state, "input").as_deref(),
        Some("Choose an image")
    );
    *draft(&mut dialog, "input") = Draft::Image(Some(DatasetId(99)));
    assert_eq!(
        problem(&mut dialog, &state, "input").as_deref(),
        Some("This image is no longer open")
    );
    let active = state.active().unwrap().id;
    *draft(&mut dialog, "input") = Draft::Image(Some(active));

    assert!(dialog.is_valid(&state.session().workspace));
    let args = dialog.args(&state.session().workspace).unwrap();
    assert_eq!(
        args.get("weights"),
        Some(&Value::List(vec![Value::Float(1.0), Value::Float(2.5)]))
    );

    // Reset restores the defaults.
    dialog.reset(&state.session().workspace);
    assert_eq!(*draft(&mut dialog, "radius"), Draft::Float(2.0));
    assert_eq!(
        *draft(&mut dialog, "method"),
        Draft::Text("Bilinear".into())
    );
}

#[test]
fn path_fields_check_what_is_on_disk() {
    let open = ParamSpec::new("source", ParamType::Path(PathMode::Open));
    let folder = ParamSpec::new("folder", ParamType::Path(PathMode::Directory));
    let save = ParamSpec::new("target", ParamType::Path(PathMode::Save));
    let spec = CommandSpec::new("test.paths", "Paths")
        .input(open)
        .input(folder)
        .input(save)
        .effects(Effects {
            reads_fs: true,
            writes_fs: true,
            ..Effects::default()
        });
    spec.validate().unwrap();
    let state = AppState::new();
    let workspace = &state.session().workspace;
    let mut dialog = Dialog::new(&spec, workspace, &Args::new(), None);
    let problem =
        |dialog: &mut Dialog, name: &str| dialog.field_mut(name).unwrap().problem(workspace);

    assert_eq!(
        problem(&mut dialog, "source").as_deref(),
        Some("Choose a file")
    );
    assert_eq!(
        problem(&mut dialog, "folder").as_deref(),
        Some("Choose a folder")
    );
    assert_eq!(
        problem(&mut dialog, "target").as_deref(),
        Some("Choose a file")
    );

    let existing = sample("on-disk.png");
    let missing = temp("not-on-disk.png");
    *draft(&mut dialog, "source") = Draft::Text(missing.display().to_string());
    assert_eq!(
        problem(&mut dialog, "source").as_deref(),
        Some("There is no file at this path")
    );
    *draft(&mut dialog, "source") = Draft::Text(existing.display().to_string());
    assert_eq!(problem(&mut dialog, "source"), None);

    *draft(&mut dialog, "folder") = Draft::Text(existing.display().to_string());
    assert_eq!(
        problem(&mut dialog, "folder").as_deref(),
        Some("There is no folder at this path")
    );
    *draft(&mut dialog, "folder") = Draft::Text(existing.parent().unwrap().display().to_string());
    assert_eq!(problem(&mut dialog, "folder"), None);

    // A file to save need not exist yet.
    *draft(&mut dialog, "target") = Draft::Text(missing.display().to_string());
    assert_eq!(problem(&mut dialog, "target"), None);
    assert_eq!(
        dialog.args(workspace).unwrap().get("target"),
        Some(&Value::Path(missing))
    );
}

#[test]
fn dialog_values_are_remembered_per_command() {
    let settings_file = temp("remember/settings.json");
    let _ = std::fs::remove_file(&settings_file);
    let mut state = AppState::build(
        demo_registry(),
        Settings::default(),
        Some(settings_file.clone()),
    );
    state.open(sample("remember.png"));

    let mut dialog = gallery(&mut state);
    *draft(&mut dialog, "radius") = Draft::Float(7.5);
    *draft(&mut dialog, "normalize") = Draft::Bool(false);
    *draft(&mut dialog, "method") = Draft::Text("Nearest".into());
    *draft(&mut dialog, "report") = Draft::Text(temp("report.txt").display().to_string());
    assert!(state.accept(&dialog));
    state.wait();
    assert!(
        matches!(state.status, Status::Done(_)),
        "{:?}",
        state.status
    );
    state.save_settings();

    let remembered = &state.settings().remembered["demo.parameters"];
    assert_eq!(remembered["radius"], 7.5);
    assert_eq!(remembered["method"], "Nearest");
    // Images mean nothing next session; `report` opted out with `persist`.
    assert!(!remembered.contains_key("input"));
    assert!(!remembered.contains_key("mask"));
    assert!(!remembered.contains_key("report"));

    // The next dialog starts from the remembered values...
    let mut again = gallery(&mut state);
    assert_eq!(*draft(&mut again, "radius"), Draft::Float(7.5));
    assert_eq!(*draft(&mut again, "normalize"), Draft::Bool(false));
    assert_eq!(*draft(&mut again, "method"), Draft::Text("Nearest".into()));
    assert_eq!(*draft(&mut again, "report"), Draft::Text(String::new()));
    // ...and Reset still goes back to the command's own defaults.
    again.reset(&state.session().workspace);
    assert_eq!(*draft(&mut again, "radius"), Draft::Float(2.0));

    // They survive into the next session.
    let (loaded, problem) = Settings::load(&settings_file);
    assert_eq!(problem, None);
    let mut next = AppState::build(demo_registry(), loaded, None);
    next.open(sample("remember-next.png"));
    let mut dialog = gallery(&mut next);
    assert_eq!(*draft(&mut dialog, "radius"), Draft::Float(7.5));
    assert_eq!(*draft(&mut dialog, "count"), Draft::Int(3));

    // A remembered value that no longer fits is ignored, not an error.
    let mut stale = Settings::default();
    let values = stale
        .remembered
        .entry("demo.parameters".into())
        .or_default();
    values.insert("radius".into(), serde_json::json!(-4.0));
    values.insert("method".into(), serde_json::json!("Lanczos"));
    values.insert("count".into(), serde_json::json!("many"));
    let mut state = AppState::build(demo_registry(), stale, None);
    state.open(sample("remember-stale.png"));
    let mut dialog = gallery(&mut state);
    assert_eq!(*draft(&mut dialog, "radius"), Draft::Float(2.0));
    assert_eq!(
        *draft(&mut dialog, "method"),
        Draft::Text("Bilinear".into())
    );
    assert_eq!(*draft(&mut dialog, "count"), Draft::Int(3));
}

// ---- Background runs ----

/// A latch a test opens to let a gated command proceed.
#[derive(Clone, Default)]
struct Gate(Arc<(Mutex<bool>, Condvar)>);

impl Gate {
    fn open(&self) {
        *self.0.0.lock().unwrap() = true;
        self.0.1.notify_all();
    }

    fn wait(&self) {
        let mut open = self.0.0.lock().unwrap();
        while !*open {
            open = self.0.1.wait(open).unwrap();
        }
    }
}

enum Ending {
    Succeed,
    Fail,
    Panic,
}

/// `test.gated`: waits for the gate, then replaces the active image with a
/// one-pixel image, fails, or panics.
struct Gated {
    spec: CommandSpec,
    gate: Gate,
    ending: Ending,
}

impl Command for Gated {
    fn spec(&self) -> &CommandSpec {
        &self.spec
    }

    fn run(&self, ctx: &mut CommandCtx<'_>, args: &Args) -> imager_command::Result<Outputs> {
        ctx.progress(0.25, "Waiting at the gate");
        self.gate.wait();
        ctx.check_cancelled()?;
        let Some(Value::Image(input)) = args.get("input") else {
            unreachable!("the executor checked the arguments");
        };
        match self.ending {
            Ending::Succeed => {
                let image = Image::from_vec(1, 1, vec![7u8]).unwrap();
                ctx.workspace_mut().set_image(*input, image)?;
                Ok(Outputs::new())
            }
            Ending::Fail => Err(Error::failed("the gate was the wrong colour")),
            Ending::Panic => panic!("deliberate panic in a test command"),
        }
    }
}

fn gated_state(ending: Ending) -> (AppState, Gate) {
    let gate = Gate::default();
    let mut registry = imager_commands::standard_registry();
    let spec = CommandSpec::new("test.gated", "Gated")
        .summary("Waits until a test lets it through.")
        .input(ParamSpec::new("input", ParamType::Image))
        .effects(Effects {
            image: ImageEffect::Modifies,
            ..Effects::default()
        });
    registry
        .register(Arc::new(Gated {
            spec,
            gate: gate.clone(),
            ending,
        }))
        .unwrap();
    let mut state = AppState::build(registry, Settings::default(), None);
    // Tests run in parallel, so each needs its own file.
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    state.open(sample(&format!("gated-{n}.png")));
    (state, gate)
}

/// Polls like the interface does each frame, until the run is over.
fn poll_until_idle(state: &mut AppState) {
    for _ in 0..2000 {
        state.poll();
        if state.running().is_none() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    panic!("the command never finished");
}

#[test]
fn commands_run_in_the_background_and_commit_as_a_whole() {
    let (mut state, gate) = gated_state(Ending::Succeed);
    state.start("test.gated", &Args::new());

    // Running: the interface is not blocked and the session is untouched.
    let running = state.running().expect("the command is running");
    assert_eq!(running.title, "Gated");
    state.poll();
    assert!(state.running().is_some());
    assert_eq!(pixels(&state), [10, 250]);
    assert_eq!(state.session().history().entries().len(), 1);

    // Other commands are held off, with the reason; interface commands
    // that do not touch the session still work.
    assert_eq!(
        state.unavailable_reason("image.invert").as_deref(),
        Some("Wait for Gated to finish")
    );
    assert_eq!(
        state.unavailable_reason("edit.undo").as_deref(),
        Some("Wait for Gated to finish")
    );
    assert_eq!(state.unavailable_reason("window.toggle_log"), None);
    assert_eq!(state.unavailable_reason("view.zoom_in"), None);
    state.start("image.invert", &Args::new());
    assert_eq!(
        state.status,
        Status::Done("Invert: Wait for Gated to finish".into())
    );
    state.start("window.toggle_log", &Args::new());
    assert!(!state.panel_open(Panel::Log));

    gate.open();
    poll_until_idle(&mut state);
    assert_eq!(state.status, Status::Done("Gated finished".into()));
    assert_eq!(pixels(&state), [7]);
    assert_eq!(state.session().history().entries().len(), 2);
    assert_eq!(state.unavailable_reason("image.invert"), None);

    // It is one undo step.
    state.undo();
    assert_eq!(pixels(&state), [10, 250]);
}

#[test]
fn progress_is_reported_while_running() {
    let (mut state, gate) = gated_state(Ending::Succeed);
    state.start("test.gated", &Args::new());
    let mut latest = state.running().unwrap();
    for _ in 0..2000 {
        if latest.fraction.is_some() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
        latest = state.running().unwrap();
    }
    assert_eq!(latest.fraction, Some(0.25));
    assert_eq!(latest.message, "Waiting at the gate");
    // Quick commands never flash a progress bar.
    assert!(
        !imager_app::state::Running {
            elapsed: std::time::Duration::from_millis(499),
            ..latest.clone()
        }
        .visible()
    );
    assert!(
        imager_app::state::Running {
            elapsed: std::time::Duration::from_millis(500),
            ..latest
        }
        .visible()
    );
    gate.open();
    state.wait();
}

#[test]
fn a_cancelled_command_leaves_no_trace() {
    let (mut state, gate) = gated_state(Ending::Succeed);
    state.start("test.gated", &Args::new());
    state.cancel();
    assert_eq!(state.status, Status::Done("Gated cancelled".into()));
    assert!(state.running().is_none());
    // The interface is free again at once, whatever the command is doing.
    assert_eq!(state.unavailable_reason("image.invert"), None);

    gate.open();
    std::thread::sleep(std::time::Duration::from_millis(50));
    state.poll();
    assert_eq!(pixels(&state), [10, 250]);
    assert_eq!(state.session().history().entries().len(), 1);
    assert!(state.log().is_empty());
    assert_eq!(state.status, Status::Done("Gated cancelled".into()));
}

#[test]
fn a_failed_command_changes_nothing_and_is_logged() {
    let (mut state, gate) = gated_state(Ending::Fail);
    state.start("test.gated", &Args::new());
    gate.open();
    poll_until_idle(&mut state);
    assert_eq!(
        state.status,
        Status::Failed("Gated: the gate was the wrong colour".into())
    );
    assert_eq!(state.log().len(), 1);
    assert_eq!(state.log()[0].level, LogLevel::Error);
    assert_eq!(pixels(&state), [10, 250]);
    assert_eq!(state.session().history().entries().len(), 1);
}

#[test]
fn a_command_that_panics_is_reported_not_fatal() {
    let (mut state, gate) = gated_state(Ending::Panic);
    state.start("test.gated", &Args::new());
    gate.open();
    poll_until_idle(&mut state);
    let Status::Failed(message) = &state.status else {
        panic!("expected a failure, got {:?}", state.status);
    };
    assert!(
        message.starts_with("Gated: the command stopped unexpectedly"),
        "{message}"
    );
    assert_eq!(pixels(&state), [10, 250]);
    // The application carries on.
    state.run("image.invert", &Args::new());
    assert_eq!(pixels(&state), [245, 5]);
}

#[test]
fn a_tab_chosen_during_a_run_stays_chosen() {
    let (mut state, gate) = gated_state(Ending::Succeed);
    let first = state.active().unwrap().id;
    state.open(sample("gated-second.png"));
    let second = state.active().unwrap().id;

    state.start("test.gated", &Args::new().with("input", second));
    state.set_active(first);
    gate.open();
    poll_until_idle(&mut state);
    assert_eq!(state.active().unwrap().id, first);
    // The command still changed the image it was started on.
    let changed = state.session().workspace.get(second).unwrap();
    assert_eq!(changed.image.meta().width(), 1);
}

// ---- Files ----

#[test]
fn several_files_open_one_after_another() {
    let mut state = AppState::new();
    let files = [
        sample("drop-a.png"),
        sample("drop-b.png"),
        sample("drop-c.png"),
    ];
    state.open_files(files.clone());
    assert!(state.running().is_some());
    poll_until_idle(&mut state);
    // The queue drains over several polls.
    for _ in 0..3 {
        poll_until_idle(&mut state);
    }
    let names: Vec<_> = state
        .session()
        .workspace
        .datasets()
        .map(|d| d.image.meta().name.clone())
        .collect();
    assert_eq!(names, ["drop-a.png", "drop-b.png", "drop-c.png"]);
    assert_eq!(state.active().unwrap().image.meta().name, "drop-c.png");
}

#[test]
fn recent_files_are_listed_persisted_and_cleared() {
    let settings_file = temp("recent/settings.json");
    let _ = std::fs::remove_file(&settings_file);
    let mut state = AppState::build(
        imager_commands::standard_registry(),
        Settings::default(),
        Some(settings_file.clone()),
    );
    let recent = |state: &AppState| labels(submenu(submenu(state.menu(), "File"), "Open Recent"));
    assert_eq!(recent(&state), ["Clear Recent Files"]);

    let (a, b) = (sample("recent-a.png"), sample("recent-b.png"));
    state.open(a.clone());
    state.open(b.clone());
    state.open(a.clone());
    state.open(temp("recent-missing.png"));
    // Most recent first, each file once, failures not listed.
    assert_eq!(state.settings().recent, [a.clone(), b.clone()]);
    assert_eq!(
        recent(&state),
        ["recent-a.png", "recent-b.png", "-", "Clear Recent Files"]
    );

    // An entry is the Open command with its file filled in.
    let items = submenu(submenu(state.menu(), "File"), "Open Recent");
    let MenuEntry::Command(entry) = items[1].clone() else {
        panic!("expected a command");
    };
    assert_eq!(entry.id, "file.open");
    assert_eq!(entry.args, Args::new().with("path", b.clone()));
    assert_eq!(
        entry.hint.as_deref(),
        Some(b.display().to_string().as_str())
    );
    assert_eq!(state.invoke(&entry.id, &entry.args), Invocation::Started);
    state.wait();
    assert_eq!(state.settings().recent, [b.clone(), a.clone()]);

    // Saving somewhere makes that file recent too.
    let saved = temp("recent-saved.png");
    state.run("file.save_as", &Args::new().with("path", saved.clone()));
    assert_eq!(state.settings().recent[0], saved);

    // Kept for the next session.
    state.save_settings();
    let (loaded, problem) = Settings::load(&settings_file);
    assert_eq!(problem, None);
    assert_eq!(loaded.recent, [saved, b, a]);
    let mut next = AppState::build(imager_commands::standard_registry(), loaded, None);
    assert_eq!(recent(&next).len(), 5);

    next.run("file.clear_recent", &Args::new());
    assert_eq!(next.take_ui_actions(), [UiAction::ClearRecent]);
    assert!(next.settings().recent.is_empty());
    assert_eq!(recent(&next), ["Clear Recent Files"]);
    assert_eq!(next.status, Status::Done("Recent files cleared".into()));
}

#[test]
fn the_recent_list_is_bounded() {
    let mut settings = Settings::default();
    for index in 0..25 {
        settings.add_recent(&temp(&format!("many-{index}.png")));
    }
    assert_eq!(settings.recent.len(), Settings::MAX_RECENT);
    assert_eq!(settings.recent[0], temp("many-24.png"));
}

#[test]
fn unreadable_settings_fall_back_to_defaults_with_a_message() {
    let path = temp("broken/settings.json");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, "{ this is not json").unwrap();
    let state = AppState::with_settings_file(path.clone());
    assert_eq!(*state.settings(), Settings::default());
    let Status::Failed(message) = &state.status else {
        panic!("expected a message, got {:?}", state.status);
    };
    assert!(message.contains("settings.json"), "{message}");
    assert_eq!(state.log().len(), 1);

    // No file at all is the normal first start: no message.
    let fresh = AppState::with_settings_file(temp("fresh/settings.json"));
    assert_eq!(fresh.status, Status::Idle);

    // Unknown and missing fields are tolerated, so settings written by
    // other versions still load.
    std::fs::write(&path, r#"{ "theme": "dark", "from_the_future": 1 }"#).unwrap();
    let (settings, problem) = Settings::load(&path);
    assert_eq!(problem, None);
    assert_eq!(settings.theme, Theme::Dark);
    assert!(settings.recent.is_empty());
}
