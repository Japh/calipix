//! Results tables, plots, generated dialogs and menus with every command
//! of the measure milestone registered, exercised without a window.

use std::collections::BTreeMap;
use std::path::PathBuf;

use calipix_app::demo;
use calipix_app::state::dialog::Dialog;
use calipix_app::state::menu::MenuEntry;
use calipix_app::state::settings::Settings;
use calipix_app::state::shortcut::Shortcut;
use calipix_app::state::tables::{PlotNote, plot_of};
use calipix_app::state::{
    AppState, Draft, Invocation, Panel, PlotKind, RowClick, Status, UiAction, action,
};
use calipix_command::{Args, ParamType, PositionAxis, Value};
use calipix_commands::measure::{SETTINGS_TABLE, results_text};
use calipix_core::{Cell, Table};

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/imagej/out")
        .join(name)
}

fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("calipix-tables-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(name)
}

fn demo_state() -> AppState {
    let mut registry = calipix_commands::standard_registry();
    demo::register(&mut registry).unwrap();
    AppState::build(registry, Settings::default(), None)
}

/// A state with the blobs fixture open: 64 x 48 pixels, 8-bit.
fn blobs() -> AppState {
    let mut state = demo_state();
    state.open(fixture("blobs.tif"));
    assert!(
        !matches!(state.status, Status::Failed(_)),
        "{:?}",
        state.status
    );
    state
}

fn run(state: &mut AppState, id: &str, args: Args) {
    state.run(id, &args);
    assert!(
        !matches!(state.status, Status::Failed(_)),
        "{id}: {:?}",
        state.status
    );
}

fn select(state: &mut AppState, selection: &str) {
    run(
        state,
        "select.set",
        Args::new().with("selection", selection),
    );
}

fn rows_of(state: &mut AppState) -> Vec<Vec<String>> {
    let text = state.table_text().expect("a table is on show");
    (0..text.len())
        .map(|row| text.row(row).map(str::to_string).collect())
        .collect()
}

/// A dialog for a command, as choosing it from a menu opens.
fn dialog(state: &mut AppState, id: &str) -> Dialog {
    match state.invoke(id, &Args::new()) {
        Invocation::Dialog(dialog) => *dialog,
        other => panic!("{id}: expected a dialog, got {other:?}"),
    }
}

// ---- The Results panel ----

#[test]
fn tables_are_listed_results_first_without_the_settings() {
    let mut state = blobs();
    assert!(state.table_names().is_empty());
    assert_eq!(state.shown_table(), None);
    assert!(state.table_text().is_none());

    run(&mut state, "analyze.histogram", Args::new());
    run(&mut state, "measure.measure", Args::new());
    run(&mut state, "measure.set_measurements", Args::new());
    // The settings are a table in the workspace, but not a result.
    assert!(state.session().workspace.table(SETTINGS_TABLE).is_some());
    assert_eq!(state.table_names(), ["Results", "Histogram of blobs.tif"]);
    // What was written last is what is on show.
    assert_eq!(state.shown_table().as_deref(), Some("Results"));

    state.show_table("Histogram of blobs.tif");
    assert_eq!(
        state.shown_table().as_deref(),
        Some("Histogram of blobs.tif")
    );
    // A table that goes away is no longer shown; the first there is, is.
    run(
        &mut state,
        "measure.clear_results",
        Args::new().with("table", "Histogram of blobs.tif"),
    );
    assert_eq!(state.table_names(), ["Results"]);
    assert_eq!(state.shown_table().as_deref(), Some("Results"));
}

#[test]
fn a_table_reads_as_imagej_writes_it_and_as_save_results_saves_it() {
    let mut state = blobs();
    run(&mut state, "measure.measure", Args::new());
    let text = state.table_text().unwrap();
    // ImageJ heads the row numbers with a single space.
    assert_eq!(text.headings, [" ", "Area", "Mean", "Min", "Max"]);
    assert_eq!(rows_of(&mut state), [["1", "3072", "46.567", "0", "255"]]);
    assert_eq!(text.numeric, [true; 5]);

    // More columns, labels, and other decimal places.
    let settings = Args::new()
        .with("std_dev", true)
        .with("display_label", true)
        .with("decimal_places", 1);
    run(&mut state, "measure.set_measurements", settings);
    select(&mut state, "rectangle 4 4 12 10");
    run(&mut state, "measure.measure", Args::new());
    let text = state.table_text().unwrap();
    assert_eq!(
        text.headings,
        [" ", "Label", "Area", "Mean", "StdDev", "Min", "Max"]
    );
    assert_eq!(
        rows_of(&mut state),
        [
            ["1", "", "3072", "46.6", "0.0", "0", "255"],
            ["2", "blobs.tif", "120", "170.0", "120.7", "0", "255"],
        ]
    );
    // The labels are text and sit on the left; the numbers on the right.
    assert_eq!(text.numeric, [true, false, true, true, true, true, true]);
    assert_eq!(text.widest[1], "blobs.tif");

    // The panel shows exactly the cells Save Results writes.
    let path = temp("as-saved.txt");
    run(
        &mut state,
        "measure.save_results",
        Args::new().with("path", path.clone()),
    );
    let saved = std::fs::read_to_string(&path).unwrap();
    let shown: Vec<String> = std::iter::once(text.headings.join("\t"))
        .chain((0..text.len()).map(|row| text.line(row).to_string()))
        .collect();
    assert_eq!(saved.lines().collect::<Vec<_>>(), shown);
    assert_eq!(
        Some(saved),
        results_text(&state.session().workspace, "Results", '\t')
    );
}

#[test]
fn the_text_of_a_table_is_made_once_per_change() {
    let mut state = blobs();
    run(&mut state, "measure.measure", Args::new());
    state.table_text().unwrap();
    let made = state.tables_formatted();
    for _ in 0..5 {
        state.table_text().unwrap();
    }
    assert_eq!(state.tables_formatted(), made, "drawing does not format");

    run(&mut state, "measure.measure", Args::new());
    assert_eq!(state.table_text().unwrap().len(), 2);
    assert_eq!(state.tables_formatted(), made + 1);
    // Changing how numbers are written changes the text too.
    let settings = Args::new().with("decimal_places", 5);
    run(&mut state, "measure.set_measurements", settings);
    assert_eq!(rows_of(&mut state)[0][2], "46.56738");
    assert_eq!(state.tables_formatted(), made + 2);
}

#[test]
fn a_hundred_thousand_rows_cost_nothing_to_look_at() {
    let mut state = blobs();
    run(&mut state, "measure.measure", Args::new());
    // A long table by ordinary means: the profile across an image a
    // hundred thousand pixels wide has a row for each column.
    const ROWS: usize = 100_000;
    run(
        &mut state,
        "demo.test_stack",
        Args::new()
            .with("width", ROWS)
            .with("height", 2)
            .with("slices", 1),
    );
    select(&mut state, &format!("rectangle 0 0 {ROWS} 2"));
    run(&mut state, "analyze.plot_profile", Args::new());
    state.show_table("Profile of Test Stack");
    let formatting = std::time::Instant::now();
    let text = state.table_text().unwrap();
    println!("formatting {ROWS} rows took {:?}", formatting.elapsed());
    assert_eq!(text.len(), ROWS);
    // Any row is read straight from the text, without walking the others.
    let last: Vec<&str> = text.row(ROWS - 1).collect();
    assert_eq!(last[0], "100000");
    assert_eq!(last[1], "99999");
    assert_eq!(text.widest[0], "100000");
    let made = state.tables_formatted();
    let started = std::time::Instant::now();
    for frame in 0..10_000 {
        let text = state.table_text().unwrap();
        assert_eq!(text.row(frame * 7 % ROWS).count(), 3);
    }
    assert_eq!(state.tables_formatted(), made);
    assert!(
        started.elapsed() < std::time::Duration::from_secs(2),
        "ten thousand frames took {:?}",
        started.elapsed()
    );
}

#[test]
fn rows_are_selected_by_clicks_and_deleted_by_a_command() {
    let mut state = blobs();
    for _ in 0..5 {
        run(&mut state, "measure.measure", Args::new());
    }
    // Make the rows tell apart.
    select(&mut state, "rectangle 4 4 12 10");
    run(&mut state, "measure.measure", Args::new());
    assert_eq!(rows_of(&mut state).len(), 6);
    let picked = |state: &AppState| state.selected_rows().iter().copied().collect::<Vec<_>>();

    let plain = RowClick::default();
    let shift = RowClick {
        extend: true,
        toggle: false,
    };
    let ctrl = RowClick {
        extend: false,
        toggle: true,
    };
    assert!(!state.table_has_attention());
    state.click_row(1, plain);
    assert_eq!(picked(&state), [1]);
    assert!(state.table_has_attention(), "a click gives it the keyboard");
    state.click_row(3, shift);
    assert_eq!(picked(&state), [1, 2, 3]);
    state.click_row(5, ctrl);
    assert_eq!(picked(&state), [1, 2, 3, 5]);
    state.click_row(2, ctrl);
    assert_eq!(picked(&state), [1, 3, 5]);
    state.click_row(4, plain);
    assert_eq!(picked(&state), [4]);
    state.click_row(99, plain);
    assert_eq!(picked(&state), [4], "a row that is not there is no row");
    state.select_all_rows();
    assert_eq!(picked(&state).len(), 6);
    state.clear_row_selection();
    assert!(picked(&state).is_empty());

    // Copy takes the selection, or everything when nothing is selected.
    let whole = state.copy_table().unwrap();
    assert_eq!(whole.lines().count(), 7);
    assert!(whole.starts_with(" \tArea\tMean\tMin\tMax\n1\t3072\t46.567\t0\t255\n"));
    assert_eq!(state.status, Status::Done("Copied 6 rows".into()));
    state.click_row(5, plain);
    assert_eq!(
        state.copy_table().unwrap(),
        " \tArea\tMean\tMin\tMax\n6\t120\t170.000\t0\t255\n"
    );
    assert_eq!(state.status, Status::Done("Copied 1 row".into()));

    // Delete is a command: in History, with the rows as the table
    // numbers them, and undone like any other.
    state.click_row(0, plain);
    state.click_row(3, shift);
    state.delete_selected_rows();
    state.wait();
    assert_eq!(
        rows_of(&mut state),
        [
            ["1", "3072", "46.567", "0", "255"],
            ["2", "120", "170.000", "0", "255"]
        ]
    );
    assert!(picked(&state).is_empty(), "the rows picked are gone");
    let history = state.history_rows();
    let last = history.last().unwrap();
    assert_eq!(last.title, "Delete Rows");
    assert_eq!(last.detail, "rows 1-4");
    state.undo();
    assert_eq!(rows_of(&mut state).len(), 6);
    state.redo();
    assert_eq!(rows_of(&mut state).len(), 2);

    // With nothing selected it says so and runs nothing.
    let steps = state.history_rows().len();
    state.delete_selected_rows();
    assert_eq!(
        state.status,
        Status::Done("Delete Rows: Select the rows to delete first".into())
    );
    assert_eq!(state.history_rows().len(), steps);

    // The command is usable on its own, and says what is wrong.
    state.run(
        "measure.delete_rows",
        &Args::new().with("rows", "2").with("table", "Results"),
    );
    assert_eq!(rows_of(&mut state), [["1", "3072", "46.567", "0", "255"]]);
    state.run("measure.delete_rows", &Args::new().with("rows", "7"));
    let Status::Failed(problem) = &state.status else {
        panic!("expected a failure, got {:?}", state.status);
    };
    assert!(problem.contains("row 7 is past the end"), "{problem}");
    // All rows gone: the columns stay.
    state.run("measure.delete_rows", &Args::new().with("rows", "1"));
    let text = state.table_text().unwrap();
    assert!(text.is_empty());
    assert_eq!(text.headings, [" ", "Area", "Mean", "Min", "Max"]);
}

#[test]
fn a_table_that_is_not_drawn_does_not_take_keys() {
    let mut state = blobs();
    run(&mut state, "measure.measure", Args::new());
    state.click_row(0, RowClick::default());
    assert!(state.table_has_attention());
    // Drawn this frame: it keeps the keyboard.
    state.note_table_drawn();
    assert!(state.table_was_drawn());
    assert!(state.table_has_attention());
    // Not drawn since (its tab is behind another): it gives it up.
    assert!(!state.table_was_drawn());
    assert!(!state.table_has_attention());
}

#[test]
fn measuring_brings_up_results_and_undo_takes_the_row_back() {
    let mut state = blobs();
    state.take_ui_actions();
    run(&mut state, "measure.measure", Args::new());
    assert_eq!(
        state.take_ui_actions(),
        [UiAction::ShowPanel(Panel::Results)]
    );
    assert_eq!(
        state.status,
        Status::Done("Measure finished; Results has 1 row".into())
    );
    run(&mut state, "measure.measure", Args::new());
    assert_eq!(
        state.status,
        Status::Done("Measure finished; Results has 2 rows".into())
    );
    assert_eq!(rows_of(&mut state).len(), 2);
    state.undo();
    assert_eq!(rows_of(&mut state).len(), 1);
    state.undo();
    assert!(state.table_text().is_none(), "the table itself is undone");
    state.redo();
    state.redo();
    assert_eq!(rows_of(&mut state).len(), 2);

    // Summarize adds rows, so it comes up too; clearing does not.
    state.take_ui_actions();
    run(&mut state, "measure.summarize", Args::new());
    assert_eq!(
        state.take_ui_actions(),
        [UiAction::ShowPanel(Panel::Results)]
    );
    let labels: Vec<String> = rows_of(&mut state)
        .iter()
        .map(|row| row[1].clone())
        .collect();
    assert_eq!(labels, ["", "", "Mean", "SD", "Min", "Max"]);
    run(&mut state, "measure.clear_results", Args::new());
    assert!(state.take_ui_actions().is_empty());
    assert_eq!(state.status, Status::Done("Clear Results finished".into()));
    // A command with other things to say still says them in the Log.
    run(&mut state, "image.info", Args::new());
    assert_eq!(state.take_ui_actions(), [UiAction::ShowPanel(Panel::Log)]);
}

#[test]
fn commands_with_only_options_to_ask_run_at_once() {
    let mut state = blobs();
    // Measure has a table and a list of measurements under "More
    // options", both with defaults: nothing a person must answer.
    assert_eq!(
        state.invoke("measure.measure", &Args::new()),
        Invocation::Started
    );
    state.wait();
    assert_eq!(rows_of(&mut state).len(), 1);
    for id in ["measure.summarize", "measure.clear_results"] {
        let spec = state.spec(id).unwrap();
        assert!(!calipix_app::state::menu::opens_dialog(spec), "{id}");
    }
    // Save Results asks for the file only, under the table's name.
    match state.invoke(
        "measure.save_results",
        &Args::new().with("table", "Results"),
    ) {
        Invocation::PickPath { request, param, .. } => {
            assert_eq!(param, "path");
            assert_eq!(request.file_name.as_deref(), Some("Results.csv"));
        }
        other => panic!("expected the file dialog, got {other:?}"),
    }
}

// ---- Plots ----

#[test]
fn profiles_histograms_and_distributions_are_plots() {
    let mut state = blobs();
    assert!(state.plot_names().is_empty());
    assert!(state.plot().is_none());

    select(&mut state, "rectangle 2 10 60 30");
    state.take_ui_actions();
    run(&mut state, "analyze.plot_profile", Args::new());
    assert_eq!(state.take_ui_actions(), [UiAction::ShowPanel(Panel::Plot)]);
    assert_eq!(
        state.status,
        Status::Done("Plot Profile finished; Profile of blobs.tif is in the Plot panel".into())
    );
    let profile = state.plot().unwrap();
    assert_eq!(profile.name, "Profile of blobs.tif");
    assert_eq!(profile.kind, PlotKind::Line);
    assert_eq!(profile.x_label, "Distance (pixels)");
    assert_eq!(profile.y_label, "Gray value");
    assert_eq!(profile.points.len(), 60);
    assert_eq!(profile.points[0], (0.0, 0.0));
    assert_eq!(profile.x_span(), (0.0, 59.0));
    assert_eq!(profile.y_span(), (0.0, 161.5));
    // The point nearest a place along the axis is the one read out.
    assert_eq!(profile.index_at(29.6), Some(30));
    assert_eq!(profile.index_at(-5.0), Some(0));
    assert_eq!(profile.index_at(f64::NAN), None);

    // A histogram is bars, one per value, standing on zero.
    run(&mut state, "select.none", Args::new());
    run(&mut state, "analyze.histogram", Args::new());
    assert_eq!(
        state.plot_names(),
        ["Histogram of blobs.tif", "Profile of blobs.tif"]
    );
    let histogram = state.plot().unwrap();
    assert_eq!(histogram.name, "Histogram of blobs.tif");
    assert_eq!(histogram.kind, PlotKind::Bars);
    assert_eq!(
        (histogram.x_label.as_str(), histogram.y_label.as_str()),
        ("Value", "Count")
    );
    assert_eq!(histogram.points.len(), 256);
    assert_eq!(histogram.bin_width, 1.0);
    assert_eq!(histogram.points[0], (0.0, 2511.0));
    assert_eq!(histogram.points[255], (255.0, 561.0));
    assert_eq!(histogram.x_span(), (0.0, 256.0));
    assert_eq!(histogram.y_span(), (0.0, 2511.0));
    assert_eq!(histogram.index_at(255.5), Some(255));
    assert_eq!(histogram.index_at(256.0), None);

    // The plot chosen stays chosen; "List" shows its numbers.
    state.show_plot("Profile of blobs.tif");
    assert_eq!(state.plot().unwrap().name, "Profile of blobs.tif");
    state.take_ui_actions();
    state.list_plot("Profile of blobs.tif");
    assert_eq!(
        state.take_ui_actions(),
        [UiAction::ShowPanel(Panel::Results)]
    );
    let text = state.table_text().unwrap();
    assert_eq!(text.headings, [" ", "Distance", "Value"]);
    assert_eq!(text.len(), 60);
    assert_eq!(rows_of(&mut state)[30], ["31", "30", "136.000"]);
    let copied = state.copy_plot("Profile of blobs.tif").unwrap();
    assert!(
        copied.starts_with(" \tDistance\tValue\n1\t0\t0.000\n"),
        "{copied}"
    );

    // A plot lives as long as its table: undone with it, back with redo.
    state.undo();
    assert_eq!(state.plot_names(), ["Profile of blobs.tif"]);
    state.redo();
    assert_eq!(state.plot_names().len(), 2);
    run(
        &mut state,
        "measure.clear_results",
        Args::new().with("table", "Profile of blobs.tif"),
    );
    assert_eq!(state.plot_names(), ["Histogram of blobs.tif"]);
    assert_eq!(state.plot().unwrap().name, "Histogram of blobs.tif");
}

#[test]
fn a_profile_is_plotted_in_the_units_of_the_image() {
    let mut state = blobs();
    let scale = Args::new()
        .with("distance", 2.0)
        .with("known", 1.0)
        .with("unit", "um");
    run(&mut state, "measure.set_scale", scale);
    select(&mut state, "rectangle 0 0 64 48");
    run(&mut state, "analyze.plot_profile", Args::new());
    let plot = state.plot().unwrap();
    assert_eq!(plot.x_label, "Distance (\u{b5}m)");
    // Two pixels to the micrometre: 64 points half a micrometre apart.
    assert_eq!(plot.x_span(), (0.0, 31.5));
    assert_eq!(plot.points[1].0, 0.5);
}

#[test]
fn a_distribution_is_plotted_under_the_name_of_its_column() {
    let mut state = blobs();
    for selection in [
        "rectangle 0 0 10 10",
        "rectangle 0 0 20 10",
        "rectangle 0 0 30 10",
        "rectangle 0 0 30 30",
    ] {
        select(&mut state, selection);
        run(&mut state, "measure.measure", Args::new());
    }
    state.take_ui_actions();
    run(&mut state, "measure.distribution", Args::new());
    assert_eq!(state.take_ui_actions(), [UiAction::ShowPanel(Panel::Plot)]);
    let plot = state.plot().unwrap();
    assert_eq!(plot.name, "Area Distribution");
    assert_eq!(plot.kind, PlotKind::Bars);
    assert_eq!(plot.x_label, "Area");
    assert_eq!(plot.y_label, "Count");
    let counted: f64 = plot.points.iter().map(|p| p.1).sum();
    assert_eq!(counted, 4.0);
    // The bars are as wide as the bins, which here are not one value wide.
    assert!(plot.bin_width > 1.0);
    assert_eq!(plot.x_span().0, 100.0);
    // The Results table is still what Results shows.
    assert_eq!(state.shown_table().as_deref(), Some("Results"));
}

#[test]
fn only_tables_shaped_like_plots_are_plots() {
    let mut table = Table::new();
    table.push_row([("Area", Cell::from(1.0)), ("Mean", Cell::from(2.0))]);
    assert_eq!(plot_of("Results", &table, None), None);

    let mut profile = Table::new();
    for (distance, value) in [(0.0, 5.0), (0.5, f64::NAN), (1.0, 7.0)] {
        profile.push_row([
            ("Distance", Cell::from(distance)),
            ("Value", Cell::from(value)),
        ]);
    }
    let note = PlotNote {
        x_unit: Some("mm".to_string()),
    };
    let plot = plot_of("Profile of x", &profile, Some(&note)).unwrap();
    assert_eq!(plot.x_label, "Distance (mm)");
    // A value that is not a number is kept (it breaks the line) and does
    // not upset the axes.
    assert!(plot.points[1].1.is_nan());
    assert_eq!(plot.y_span(), (5.0, 7.0));
    assert_eq!(plot.x_span(), (0.0, 1.0));
    // Without rows there is nothing to plot.
    profile.clear_rows();
    assert_eq!(plot_of("Profile of x", &profile, None), None);
}

// ---- Shortcuts and menus ----

#[test]
fn no_two_commands_share_a_shortcut() {
    let registry = action::full_registry();
    let declared = registry
        .specs()
        .filter_map(|spec| Some((spec.shortcut.clone()?, spec.id.clone())));
    let extra = action::EXTRA_SHORTCUTS
        .iter()
        .map(|&(id, text)| (text.to_string(), id.to_string()));
    let mut owners: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut count = 0;
    for (text, id) in declared.chain(extra) {
        let shortcut =
            Shortcut::parse(&text).unwrap_or_else(|e| panic!("{id}: `{text}` does not parse: {e}"));
        // Compared as pressed, so that `Ctrl+Shift+A` and `Shift+Ctrl+A`
        // would clash.
        owners.entry(shortcut.display(false)).or_default().push(id);
        count += 1;
    }
    let clashes: Vec<_> = owners.iter().filter(|(_, ids)| ids.len() > 1).collect();
    assert!(clashes.is_empty(), "shared shortcuts: {clashes:?}");
    assert_eq!(count, owners.len());

    // The table of shortcuts, as documented.
    let table: Vec<String> = owners
        .iter()
        .map(|(shortcut, ids)| format!("{shortcut} {}", ids[0]))
        .collect();
    assert_eq!(
        table,
        [
            "+ view.zoom_in",
            "- view.zoom_out",
            "0 view.zoom_fit",
            "1 view.zoom_actual",
            "Ctrl+A select.all",
            "Ctrl+K help.command_palette",
            "Ctrl+M measure.measure",
            "Ctrl+O file.open",
            "Ctrl+S file.save",
            "Ctrl+Shift+A select.none",
            "Ctrl+Shift+D image.duplicate",
            "Ctrl+Shift+P help.command_palette",
            "Ctrl+Shift+S file.save_as",
            "Ctrl+Shift+X image.crop",
            "Ctrl+Shift+Z edit.redo",
            "Ctrl+W file.close",
            "Ctrl+Z edit.undo",
            "Left view.previous_plane",
            "Right view.next_plane",
        ]
    );
}

/// The whole menu bar as indented text: `-` for a separator, `>` after a
/// submenu.
fn tree(items: &[MenuEntry], depth: usize, out: &mut Vec<String>) {
    let indent = "  ".repeat(depth);
    for item in items {
        match item {
            MenuEntry::Separator => out.push(format!("{indent}-")),
            MenuEntry::Submenu { title, items } => {
                out.push(format!("{indent}{title} >"));
                tree(items, depth + 1, out);
            }
            MenuEntry::Command(command) => out.push(format!("{indent}{}", command.label)),
        }
    }
}

#[test]
fn the_menus_with_every_command_read_as_designed() {
    let state = AppState::new();
    let mut lines = Vec::new();
    tree(state.menu(), 0, &mut lines);
    let expected = include_str!("data/menu_tree.txt");
    let expected: Vec<&str> = expected.lines().collect();
    assert_eq!(
        lines,
        expected,
        "the menu tree changed:\n{}",
        lines.join("\n")
    );

    // At most two levels: a menu and one submenu.
    fn depth(items: &[MenuEntry]) -> usize {
        items
            .iter()
            .map(|item| match item {
                MenuEntry::Submenu { items, .. } => 1 + depth(items),
                _ => 0,
            })
            .max()
            .unwrap_or(0)
    }
    assert_eq!(depth(state.menu()), 2);

    // A command is in exactly one place, and no menu starts, ends or
    // doubles up on a separator.
    fn walk(items: &[MenuEntry], ids: &mut Vec<String>) {
        assert_ne!(items.first(), Some(&MenuEntry::Separator));
        assert_ne!(items.last(), Some(&MenuEntry::Separator));
        for pair in items.windows(2) {
            assert!(pair[0] != MenuEntry::Separator || pair[1] != MenuEntry::Separator);
        }
        for item in items {
            match item {
                MenuEntry::Submenu { items, .. } => walk(items, ids),
                MenuEntry::Command(command) => ids.push(command.id.clone()),
                MenuEntry::Separator => {}
            }
        }
    }
    let mut ids = Vec::new();
    walk(state.menu(), &mut ids);
    let mut unique = ids.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(ids.len(), unique.len(), "a command is in two menus");

    // View never changes data; Process and nothing else changes samples.
    for spec in state.registry().specs() {
        let Some(menu) = &spec.menu else { continue };
        let changes = spec.effects.image != calipix_command::ImageEffect::None;
        let display = calipix_commands::view::DISPLAY_COMMANDS.contains(&spec.id.as_str());
        match menu.path[0].as_str() {
            "View" => assert!(!changes || display, "{} is under View", spec.id),
            "Process" => assert!(changes, "{} is under Process", spec.id),
            _ => {}
        }
    }
}

// ---- Dialogs ----

#[test]
fn choices_show_labels_and_keep_their_values() {
    let mut state = blobs();
    let lut = dialog(&mut state, "view.set_lut");
    let field = lut.fields.iter().find(|f| f.param.name == "lut").unwrap();
    let ParamType::Choice(choices) = &field.param.ty else {
        panic!("the lookup table is a choice");
    };
    let shown: Vec<&str> = choices
        .iter()
        .map(|c| field.param.choice_label(c))
        .collect();
    assert_eq!(
        shown,
        [
            "Fire",
            "Grays",
            "Ice",
            "Spectrum",
            "3-3-2 RGB",
            "Red",
            "Green",
            "Blue",
            "Cyan",
            "Magenta",
            "Yellow",
            "Red/Green"
        ]
    );
    // What runs is the choice, not its label.
    let mut lut = lut;
    lut.field_mut("lut").unwrap().draft = Draft::Text("rgb332".into());
    let args = lut.args(&state.session().workspace).unwrap();
    assert_eq!(args.get("lut"), Some(&Value::from("rgb332")));

    // No drop-down anywhere shows a bare identifier.
    for spec in state.registry().specs() {
        for param in &spec.inputs {
            let ParamType::Choice(choices) = &param.ty else {
                continue;
            };
            for choice in choices {
                let label = param.choice_label(choice);
                let first = label.chars().next().unwrap();
                assert!(
                    first.is_uppercase() || first.is_ascii_digit(),
                    "{}: `{}` shows as `{label}`",
                    spec.id,
                    param.name
                );
                assert!(!label.contains('_'), "{}: `{label}`", spec.id);
            }
        }
    }
}

#[test]
fn a_kernel_is_typed_over_several_lines() {
    let mut state = blobs();
    let convolve = dialog(&mut state, "filter.convolve");
    assert!(convolve.has_multiline());
    let kernel = convolve.fields.iter().find(|f| f.param.name == "kernel");
    let kernel = kernel.unwrap();
    assert!(kernel.param.multiline);
    let Draft::Text(text) = &kernel.draft else {
        panic!("a kernel is text");
    };
    // ImageJ's default 5 x 5 kernel, a row to a line.
    assert_eq!(
        text.lines().collect::<Vec<_>>(),
        [
            "-1 -1 -1 -1 -1",
            "-1 -1 -1 -1 -1",
            "-1 -1 24 -1 -1",
            "-1 -1 -1 -1 -1",
            "-1 -1 -1 -1 -1"
        ]
    );
    // It runs as typed, and History tells it on one line.
    let mut convolve = convolve;
    convolve.field_mut("kernel").unwrap().draft = Draft::Text("1 1 1\n1 1 1\n1 1 1".into());
    assert!(state.accept(&convolve));
    state.wait();
    assert!(
        !matches!(state.status, Status::Failed(_)),
        "{:?}",
        state.status
    );
    let history = state.history_rows();
    assert_eq!(
        history.last().unwrap().detail,
        "blobs.tif, kernel 1 1 1; 1 1 1; 1 1 1"
    );
}

#[test]
fn whole_stack_is_asked_only_of_stacks_and_after_the_rest() {
    let mut state = blobs();
    let names = |dialog: &Dialog| -> Vec<String> {
        let fields = dialog.fields.iter().filter(|f| !f.param.advanced);
        fields.map(|f| f.param.label.clone()).collect()
    };
    // One plane: the question has one answer and is not asked. Neither is
    // the image, which is the one in view.
    let single = dialog(&mut state, "filter.gaussian_blur");
    assert_eq!(names(&single), ["Sigma", "Scaled units"]);
    let args = single.args(&state.session().workspace).unwrap();
    assert_eq!(args.get("stack"), Some(&Value::Bool(false)));
    assert_eq!(
        args.get("input"),
        Some(&Value::Image(state.active().unwrap().id))
    );

    let stack = Args::new()
        .with("width", 32)
        .with("height", 32)
        .with("slices", 4);
    run(&mut state, "demo.test_stack", stack);
    let several = dialog(&mut state, "filter.gaussian_blur");
    assert_eq!(names(&several), ["Sigma", "Scaled units", "Whole stack"]);
    let whole = several.fields.last().unwrap();
    assert_eq!(whole.param.position, Some(PositionAxis::Stack));
    assert_eq!(whole.draft, Draft::Bool(false));
    // The plane in view is still not asked about.
    assert!(several.fields.iter().all(|f| f.param.name != "slice"));

    // A command with nothing else to ask asks just that of a stack, and
    // nothing of a single image.
    let invert = dialog(&mut state, "image.invert");
    assert_eq!(names(&invert), ["Whole stack"]);
    state.set_active(calipix_command::DatasetId(0));
    assert_eq!(
        state.invoke("image.invert", &Args::new()),
        Invocation::Started
    );
    state.wait();
}

#[test]
fn two_images_start_as_the_active_one_and_the_other() {
    let mut state = blobs();
    let first = state.active().unwrap().id;
    // Alone, an image is both operands.
    let alone = dialog(&mut state, "math.image_calculator");
    let image = |dialog: &Dialog, name: &str| {
        let field = dialog.fields.iter().find(|f| f.param.name == name).unwrap();
        (field.param.label.clone(), field.draft.clone())
    };
    assert_eq!(
        image(&alone, "input"),
        ("Image 1".to_string(), Draft::Image(Some(first)))
    );
    assert_eq!(
        image(&alone, "image2"),
        ("Image 2".to_string(), Draft::Image(Some(first)))
    );

    state.open(fixture("fmt_gray8.bmp"));
    let second = state.active().unwrap().id;
    assert_ne!(first, second);
    let both = dialog(&mut state, "math.image_calculator");
    assert_eq!(image(&both, "input").1, Draft::Image(Some(second)));
    assert_eq!(image(&both, "image2").1, Draft::Image(Some(first)));
    // In reading order: the first operand, the operation, the second.
    let labels: Vec<&str> = both
        .fields
        .iter()
        .filter(|f| !f.param.advanced)
        .map(|f| f.param.label.as_str())
        .collect();
    assert_eq!(
        labels,
        [
            "Image 1",
            "Operation",
            "Image 2",
            "Create new image",
            "32-bit (float) result"
        ]
    );

    // It runs on the two images chosen.
    let mut both = both;
    both.field_mut("operation").unwrap().draft = Draft::Text("Subtract".into());
    assert!(state.accept(&both));
    state.wait();
    assert!(
        !matches!(state.status, Status::Failed(_)),
        "{:?}",
        state.status
    );
    assert_eq!(state.session().workspace.len(), 3);
    let history = state.history_rows();
    assert_eq!(
        history.last().unwrap().detail,
        "input fmt_gray8.bmp, operation Subtract, image2 blobs.tif"
    );
}

#[test]
fn numbers_that_may_be_left_out_can_be_left_out() {
    let mut state = blobs();
    // Scale's width and height replace its factors when given, so they
    // start empty, which is not the same as zero.
    let mut scale = dialog(&mut state, "image.scale");
    assert_eq!(
        scale.field_mut("width").unwrap().draft,
        Draft::Text(String::new())
    );
    let workspace = state.session().workspace.clone();
    let args = scale.args(&workspace).unwrap();
    assert_eq!(args.get("width"), None);
    assert_eq!(args.get("x_scale"), Some(&Value::Float(0.5)));

    scale.field_mut("width").unwrap().draft = Draft::Text("32".into());
    assert_eq!(
        scale.args(&workspace).unwrap().get("width"),
        Some(&Value::Int(32))
    );
    scale.field_mut("width").unwrap().draft = Draft::Text("wide".into());
    let width = scale.fields.iter().find(|f| f.param.name == "width");
    assert_eq!(
        width.unwrap().problem(&workspace).as_deref(),
        Some("“wide” is not a whole number")
    );
    assert!(!scale.is_valid(&workspace));
    scale.field_mut("width").unwrap().draft = Draft::Text("0".into());
    let width = scale.fields.iter().find(|f| f.param.name == "width");
    assert_eq!(
        width.unwrap().problem(&workspace).as_deref(),
        Some("Must be at least 1")
    );

    // Resize and Canvas Size start from the size the image has.
    for id in ["image.resize", "image.canvas_size"] {
        let mut sized = dialog(&mut state, id);
        assert_eq!(
            sized.field_mut("width").unwrap().draft,
            Draft::Text("64".into()),
            "{id}"
        );
        assert_eq!(
            sized.field_mut("height").unwrap().draft,
            Draft::Text("48".into())
        );
        // Reset keeps them: they are where the image is, not a memory.
        sized.field_mut("width").unwrap().draft = Draft::Text("5".into());
        sized.reset(&workspace);
        assert_eq!(
            sized.field_mut("width").unwrap().draft,
            Draft::Text("64".into())
        );
    }
    // Properties changes only what is filled in.
    let properties = dialog(&mut state, "image.properties");
    let args = properties.args(&workspace).unwrap();
    assert_eq!(args.get("pixel_width"), None);
    assert_eq!(args.get("unit"), None);
}

/// Every command's dialog, opened on a stack with a second image open so
/// that nothing is hidden, and checked against the rules of `docs/ux.md`.
#[test]
fn every_dialog_follows_the_naming_rules() {
    let mut state = blobs();
    let stack = Args::new()
        .with("width", 32)
        .with("height", 32)
        .with("slices", 3);
    run(&mut state, "demo.test_stack", stack);
    let active = state.active().unwrap().id;
    let specs: Vec<_> = state.registry().specs().cloned().collect();
    let mut opened = 0;
    let mut listing = Vec::new();
    for spec in specs {
        let id = spec.id.clone();
        if id.starts_with("demo.") {
            continue;
        }
        // As a menu opens it: the image and the plane come from the view.
        // Built here rather than by choosing the command, so that the
        // commands with nothing to ask do not run and change the session.
        let has_image = spec.inputs.iter().any(|p| p.ty == ParamType::Image);
        let preset = if has_image {
            state.display_args(&id, active)
        } else {
            Args::new()
        };
        let dialog = Dialog::new(&spec, &state.session().workspace, &preset, None);
        let asks = |f: &calipix_app::state::Field| {
            !calipix_app::state::menu::can_go_unasked(&f.param)
                && !matches!(f.param.ty, ParamType::Path(_))
        };
        if !dialog.fields.iter().any(asks) {
            continue;
        }
        opened += 1;
        let spec = &dialog.spec;
        assert!(spec.summary.ends_with('.'), "{id}: summary without a stop");
        assert!(!spec.title.ends_with('…') && !spec.title.ends_with("..."));
        listing.push(format!("{} [{}]", spec.title, id));
        for field in &dialog.fields {
            let label = &field.param.label;
            let place = format!("{id}: `{label}`");
            assert!(!label.is_empty(), "{place}");
            assert!(!label.ends_with(':'), "{place} ends with a colon");
            // Sentence case: a capital (or digit) first.
            let first = label.chars().next().unwrap();
            assert!(first.is_uppercase() || first.is_ascii_digit(), "{place}");
            // Units are declared, not typed into the label.
            for unit in ["(pixels)", "(px)", "(degrees)", "(%)"] {
                assert!(!label.contains(unit), "{place} carries its unit");
            }
            // Room for the label beside a 240-point field in a 430-point
            // dialog.
            assert!(label.chars().count() <= 34, "{place} is too long");
            // No field shows a spurious decimal.
            if let Draft::Text(text) = &field.draft
                && matches!(field.param.ty, ParamType::Int | ParamType::Float)
            {
                assert!(!text.ends_with(".0"), "{place} starts as `{text}`");
            }
            let mark = if field.param.advanced {
                "  (more) "
            } else {
                "  "
            };
            listing.push(format!("{mark}{label}: {:?}", field.draft));
        }
        // The defaults are something a person can press OK on, or the
        // field says why not.
        let workspace = &state.session().workspace;
        for field in &dialog.fields {
            if let Some(problem) = field.problem(workspace) {
                assert!(
                    matches!(field.param.ty, ParamType::Path(_) | ParamType::Roi),
                    "{id}: `{}` starts invalid: {problem}",
                    field.param.label
                );
            }
        }
        // Advanced fields are never the only ones.
        assert!(
            dialog.fields.iter().any(|f| !f.param.advanced),
            "{id} has only advanced fields"
        );
    }
    assert!(opened >= 45, "only {opened} dialogs were opened");
    // Kept for reading: `cargo test -- --nocapture every_dialog`.
    println!("{}", listing.join("\n"));
}

#[test]
fn a_number_field_shows_no_spurious_decimals() {
    use calipix_app::state::dialog::number_text;
    assert_eq!(number_text(2.0), "2");
    assert_eq!(number_text(2.5), "2.5");
    assert_eq!(number_text(-0.35), "-0.35");
    assert_eq!(number_text(1e-7), "0.0000001");
}

// ---- Long operations ----

/// Median with radius 5 on twenty 1024 x 1024 planes takes many seconds.
/// The interface is not held up by it, shows how far it has got, and
/// Cancel leaves the image exactly as it was.
#[test]
fn a_slow_filter_reports_progress_and_cancels_without_a_trace() {
    let mut state = demo_state();
    run(&mut state, "demo.test_stack", Args::new());
    let before = state.active().unwrap().image.clone();
    assert_eq!(before.meta().plane_count(), 20);
    let steps = state.history_rows().len();

    let args = Args::new().with("radius", 5.0).with("stack", true);
    let asked = std::time::Instant::now();
    state.start("filter.median", &args);
    // Starting returns at once: the work is on another thread.
    assert!(asked.elapsed() < std::time::Duration::from_millis(250));
    let running = state.running().expect("the filter is running");
    assert_eq!(running.title, "Median");

    // Polled as the interface polls, each frame staying quick, until the
    // filter has said how far it is and the bar would be on screen.
    let mut slowest = std::time::Duration::ZERO;
    let mut shown = None;
    for _ in 0..3000 {
        let frame = std::time::Instant::now();
        state.poll();
        slowest = slowest.max(frame.elapsed());
        let Some(running) = state.running() else {
            panic!("the filter finished before it could be cancelled");
        };
        if running.visible() && running.fraction.is_some_and(|f| f > 0.0) {
            shown = Some(running);
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    let shown = shown.expect("the filter never reported progress");
    println!(
        "progress {:?} after {:?}; slowest frame {slowest:?}",
        shown.fraction,
        asked.elapsed()
    );
    let fraction = shown.fraction.unwrap();
    assert!(fraction > 0.0 && fraction < 1.0, "{fraction}");
    assert!(
        slowest < std::time::Duration::from_millis(100),
        "a frame took {slowest:?} while the filter ran"
    );
    // Meanwhile the image on screen is the one from before.
    assert!(state.active().unwrap().image.ptr_eq(&before));
    assert_eq!(
        state.unavailable_reason("image.invert").as_deref(),
        Some("Wait for Median to finish")
    );

    state.cancel();
    assert_eq!(state.status, Status::Done("Median cancelled".into()));
    assert!(state.running().is_none());
    assert_eq!(state.unavailable_reason("image.invert"), None);
    // Whatever the worker still does, nothing of it arrives.
    std::thread::sleep(std::time::Duration::from_millis(300));
    state.poll();
    assert!(state.active().unwrap().image.ptr_eq(&before));
    assert_eq!(state.history_rows().len(), steps);
    assert_eq!(state.status, Status::Done("Median cancelled".into()));
}
