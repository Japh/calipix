//! The selection tools and the list of regions, exercised without a
//! window: pointer positions go in, commands and History come out.

use std::path::PathBuf;

use calipix_app::state::{
    AppState, CanvasKey, Invocation, Mark, Mods, Panel, Pointer, RegionLabels, RowClick, Status,
    Tool,
};
use calipix_command::{Args, DatasetId};
use calipix_core::Image;
use calipix_roi::{Kind, Roi};

const WIDTH: usize = 64;
const HEIGHT: usize = 48;

fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("calipix-tools-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(name)
}

/// The application with one image open: 64 by 48, each sample its column.
fn state(name: &str) -> (AppState, DatasetId) {
    let path = temp(name);
    let pixels: Vec<u8> = (0..WIDTH * HEIGHT).map(|i| (i % WIDTH) as u8).collect();
    calipix_io::save(&Image::from_vec(WIDTH, HEIGHT, pixels).unwrap(), &path).unwrap();
    let mut state = AppState::new();
    state.open(path);
    assert!(
        !matches!(state.status, Status::Failed(_)),
        "{:?}",
        state.status
    );
    let id = state.active().unwrap().id;
    (state, id)
}

fn at(x: f64, y: f64) -> Pointer {
    Pointer::new([x, y], 1.0)
}

fn roi(state: &AppState) -> Option<Roi> {
    state.active().unwrap().roi.clone()
}

fn text(state: &AppState) -> String {
    roi(state).map_or_else(|| "none".to_string(), |roi| roi.to_string())
}

/// The History panel's lines after the opening of the image, as
/// `title: detail`.
fn history(state: &AppState) -> Vec<String> {
    state
        .history_rows()
        .into_iter()
        .skip(1)
        .filter(|row| !row.undone)
        .map(|row| {
            let detail = row.detail.split_once(", ").map_or("", |(_, rest)| rest);
            format!("{}: {detail}", row.title)
        })
        .collect()
}

/// Presses at the first position, moves through the others and releases
/// at the last, then lets the command it started finish.
fn drag(state: &mut AppState, id: DatasetId, path: &[Pointer]) {
    state.pointer_pressed(id, path[0]);
    for p in &path[1..] {
        state.pointer_dragged(id, *p);
    }
    state.pointer_released(id, *path.last().unwrap());
    state.wait();
}

fn click(state: &mut AppState, id: DatasetId, p: Pointer) {
    state.pointer_hovered(id, p);
    drag(state, id, &[p, p]);
}

fn choose(state: &mut AppState, tool: Tool) {
    state.run(&tool.command_id(), &Args::new());
    assert_eq!(state.tool(), tool);
}

#[test]
fn a_tool_is_chosen_by_its_command_and_marked_in_the_menu() {
    let (mut state, _) = state("tool-commands.png");
    assert_eq!(state.tool(), Tool::Pointer);
    for tool in Tool::ALL {
        let id = tool.command_id();
        let spec = state.spec(&id).unwrap_or_else(|| panic!("no command {id}"));
        assert_eq!(spec.shortcut.as_deref(), Some(tool.shortcut()));
        assert!(!spec.flags.recordable, "{id} would be recorded in macros");
        choose(&mut state, tool);
        assert_eq!(state.mark(&id), Some(Mark::Choice(true)));
        let others = Tool::ALL.into_iter().filter(|other| *other != tool);
        for other in others {
            assert_eq!(state.mark(&other.command_id()), Some(Mark::Choice(false)));
        }
        // The status bar says how to use it.
        assert_eq!(
            state.status,
            Status::Done(format!("{}: {}", tool.title(), tool.hint()))
        );
    }
    // Choosing tools is not something to undo.
    assert!(history(&state).is_empty());
}

#[test]
fn a_dragged_rectangle_is_one_step_in_history() {
    let (mut state, id) = state("rectangle.png");
    choose(&mut state, Tool::Rectangle);

    // While the pointer is down nothing is recorded: the shape is only
    // on show.
    state.pointer_pressed(id, at(10.3, 8.4));
    state.pointer_dragged(id, at(25.0, 20.0));
    assert!(state.gesture_active());
    assert_eq!(
        state.canvas_view(id).selection,
        Some(Roi::rectangle(10.0, 8.0, 15.0, 12.0))
    );
    assert_eq!(
        state.selection_summary().as_deref(),
        Some("Rectangle 15 × 12 at 10, 8")
    );
    assert_eq!(roi(&state), None);
    assert!(history(&state).is_empty());

    // On release the command starts; until it has run the shape stays
    // on show, so that nothing flickers.
    state.pointer_released(id, at(40.2, 30.1));
    assert!(!state.gesture_active());
    assert_eq!(
        state.canvas_view(id).selection,
        Some(Roi::rectangle(10.0, 8.0, 30.0, 22.0))
    );
    state.wait();
    assert_eq!(text(&state), "rectangle 10 8 30 22");
    assert_eq!(
        history(&state),
        ["Specify Selection: selection rectangle 10 8 30 22"]
    );
    assert_eq!(state.canvas_view(id).selection, roi(&state));

    // Undo takes the whole gesture back; redo brings it back.
    state.undo();
    assert_eq!(roi(&state), None);
    state.redo();
    assert_eq!(text(&state), "rectangle 10 8 30 22");
}

#[test]
fn moving_and_reshaping_are_one_step_each() {
    let (mut state, id) = state("move.png");
    choose(&mut state, Tool::Rectangle);
    drag(&mut state, id, &[at(10.0, 8.0), at(40.0, 30.0)]);

    // Dragged from its inside, with a drawing tool still chosen.
    state.pointer_pressed(id, at(20.0, 20.0));
    state.pointer_dragged(id, at(25.4, 17.2));
    assert_eq!(
        state.canvas_view(id).selection,
        Some(Roi::rectangle(15.0, 5.0, 30.0, 22.0))
    );
    assert_eq!(text(&state), "rectangle 10 8 30 22");
    state.pointer_released(id, at(25.4, 17.2));
    state.wait();
    assert_eq!(text(&state), "rectangle 15 5 30 22");

    // By its bottom right handle, with the pointer tool.
    choose(&mut state, Tool::Pointer);
    state.pointer_pressed(id, at(45.0, 27.0));
    state.pointer_dragged(id, at(50.0, 35.0));
    assert_eq!(state.canvas_view(id).handle, Some(4));
    state.pointer_released(id, at(50.0, 35.0));
    state.wait();
    assert_eq!(text(&state), "rectangle 15 5 35 30");

    assert_eq!(
        history(&state),
        [
            "Specify Selection: selection rectangle 10 8 30 22",
            "Translate Selection: x offset 5.0, y offset -3.0",
            "Specify Selection: selection rectangle 15 5 35 30",
        ]
    );
    for expected in ["rectangle 15 5 30 22", "rectangle 10 8 30 22", "none"] {
        state.undo();
        assert_eq!(text(&state), expected);
    }
}

#[test]
fn a_gesture_that_changes_nothing_or_is_cancelled_records_nothing() {
    let (mut state, id) = state("no-op.png");
    choose(&mut state, Tool::Oval);
    // A click on the bare image, with nothing selected.
    click(&mut state, id, at(20.0, 20.0));
    assert!(history(&state).is_empty());

    drag(&mut state, id, &[at(10.0, 8.0), at(40.0, 30.0)]);
    assert_eq!(history(&state).len(), 1);
    // Pressed and released inside; dragged away and back; a handle
    // dragged out and back.
    click(&mut state, id, at(25.0, 19.0));
    drag(
        &mut state,
        id,
        &[at(25.0, 19.0), at(40.0, 40.0), at(25.2, 19.1)],
    );
    drag(
        &mut state,
        id,
        &[at(40.0, 30.0), at(50.0, 40.0), at(40.1, 29.9)],
    );
    assert_eq!(history(&state).len(), 1);

    // Escape in the middle of a drag gives it up.
    state.pointer_pressed(id, at(25.0, 19.0));
    state.pointer_dragged(id, at(45.0, 35.0));
    assert!(state.canvas_key(CanvasKey::Escape));
    assert!(!state.gesture_active());
    state.pointer_released(id, at(45.0, 35.0));
    state.wait();
    assert_eq!(text(&state), "oval 10 8 30 22");
    assert_eq!(history(&state).len(), 1);
    // With nothing under way Escape is not the tools' to take.
    assert!(!state.canvas_key(CanvasKey::Escape));

    // A click on the bare image removes the selection: one step.
    click(&mut state, id, at(60.0, 45.0));
    assert_eq!(roi(&state), None);
    assert_eq!(history(&state).last().unwrap(), "Select None: ");
}

#[test]
fn a_polygon_is_one_step_however_many_clicks_it_takes() {
    let (mut state, id) = state("polygon.png");
    choose(&mut state, Tool::Polygon);
    for corner in [at(10.0, 8.0), at(40.0, 6.0), at(50.0, 30.0)] {
        click(&mut state, id, corner);
    }
    assert!(state.drawing_path());
    assert!(history(&state).is_empty());
    // The rubber band follows the pointer, and the status bar keeps
    // count.
    state.pointer_hovered(id, at(20.0, 42.0));
    let draft = state.canvas_view(id).draft.unwrap();
    assert_eq!((draft.points.len(), draft.hover), (3, Some((20.0, 42.0))));
    assert_eq!(
        state.selection_summary().as_deref(),
        Some("Polygon 40 × 36 at 10, 6")
    );
    click(&mut state, id, at(20.0, 42.0));
    // Backspace takes back a corner; then it is put again.
    assert!(state.canvas_key(CanvasKey::Backspace));
    assert_eq!(state.canvas_view(id).draft.unwrap().points.len(), 3);
    click(&mut state, id, at(20.0, 42.0));
    // A click on the first corner closes it.
    click(&mut state, id, at(10.5, 8.5));
    assert!(!state.drawing_path());
    assert_eq!(text(&state), "polygon 10,8 40,6 50,30 20,42");
    assert_eq!(
        history(&state),
        ["Specify Selection: selection polygon 10,8 40,6 50,30 20,42"]
    );

    // Enter finishes one too; Escape gives one up, leaving the old
    // selection in place. (They start clear of the selection there is:
    // a press on it would move it, and on a corner reshape it.)
    for corner in [at(57.0, 38.0), at(63.0, 38.0), at(63.0, 46.0)] {
        click(&mut state, id, corner);
    }
    assert!(state.canvas_key(CanvasKey::Enter));
    state.wait();
    assert_eq!(text(&state), "polygon 57,38 63,38 63,46");
    for corner in [at(1.0, 1.0), at(9.0, 1.0), at(9.0, 9.0)] {
        click(&mut state, id, corner);
    }
    assert!(state.canvas_key(CanvasKey::Escape));
    assert_eq!(text(&state), "polygon 57,38 63,38 63,46");
    assert_eq!(history(&state).len(), 2);
    // Choosing another tool gives up a shape under way as well.
    click(&mut state, id, at(20.0, 20.0));
    assert!(state.drawing_path());
    choose(&mut state, Tool::Line);
    assert!(!state.gesture_active());
}

#[test]
fn every_tool_draws_its_shape() {
    let (mut state, id) = state("shapes.png");
    choose(&mut state, Tool::Oval);
    drag(&mut state, id, &[at(8.0, 6.0), at(30.0, 26.0)]);
    assert_eq!(text(&state), "oval 8 6 22 20");

    choose(&mut state, Tool::Line);
    drag(&mut state, id, &[at(50.5, 5.5), at(60.5, 40.5)]);
    assert_eq!(text(&state), "line 50 5 60 40");
    assert_eq!(
        state.selection_summary().as_deref(),
        Some("Line   length 36.4 pixels   angle -74.05°")
    );

    choose(&mut state, Tool::Polyline);
    for p in [
        at(6.5, 40.5),
        at(20.5, 10.5),
        at(36.5, 34.5),
        at(36.5, 34.5),
    ] {
        click(&mut state, id, p);
    }
    assert_eq!(text(&state), "polyline 6,40 20,10 36,34");

    choose(&mut state, Tool::Freehand);
    drag(
        &mut state,
        id,
        &[
            at(10.0, 10.0),
            at(30.0, 10.0),
            at(30.0, 30.0),
            at(10.0, 30.0),
        ],
    );
    assert_eq!(text(&state), "freehand 10,10 30,10 30,30 10,30");

    choose(&mut state, Tool::Point);
    click(&mut state, id, at(10.5, 10.5));
    click(&mut state, id, at(30.5, 20.5).with(Mods::SHIFT));
    click(&mut state, id, at(50.5, 35.5).with(Mods::SHIFT));
    assert_eq!(text(&state), "points 10,10 30,20 50,35");
    assert_eq!(state.selection_summary().as_deref(), Some("3 points"));

    // Magnified beyond 150%, shapes other than boxes keep fractions.
    choose(&mut state, Tool::Line);
    let fine = |x: f64, y: f64| Pointer::new([x, y], 4.0);
    drag(&mut state, id, &[fine(55.25, 2.75), fine(60.5, 44.0)]);
    assert_eq!(text(&state), "line 54.75 2.25 60 43.5");
    choose(&mut state, Tool::Rectangle);
    drag(&mut state, id, &[fine(10.3, 8.4), fine(40.2, 30.1)]);
    assert_eq!(text(&state), "rectangle 10 8 30 22");

    let titles: Vec<String> = history(&state)
        .into_iter()
        .map(|line| line.split(':').next().unwrap().to_string())
        .collect();
    assert_eq!(titles.len(), 9);
    assert!(titles.iter().all(|title| title == "Specify Selection"));
}

#[test]
fn a_long_outline_is_cut_short_in_history() {
    let (mut state, id) = state("freehand.png");
    choose(&mut state, Tool::Freehand);
    let path: Vec<Pointer> = (0..40)
        .map(|step| {
            let angle = std::f64::consts::TAU * step as f64 / 40.0;
            at(32.0 + 20.0 * angle.cos(), 24.0 + 15.0 * angle.sin())
        })
        .collect();
    drag(&mut state, id, &path);
    assert_eq!(roi(&state).map(|roi| roi.kind()), Some(Kind::Freehand));
    let line = history(&state).pop().unwrap();
    assert!(
        line.starts_with("Specify Selection: selection freehand 52,24"),
        "{line}"
    );
    assert!(line.ends_with(" …") && line.len() < 120, "{line}");
}

#[test]
fn arrows_nudge_the_selection_and_alt_resizes_it() {
    let (mut state, id) = state("nudge.png");
    let arrow = |dx, dy, resize| CanvasKey::Arrow { dx, dy, resize };
    // With nothing selected the arrows are free to step through planes.
    assert!(!state.canvas_key(arrow(1, 0, false)));

    state.run(
        "select.set",
        &Args::new().with("selection", Roi::rectangle(10.0, 8.0, 30.0, 22.0)),
    );
    // Keys come faster than commands finish: none is lost.
    assert!(state.canvas_key(arrow(1, 0, false)));
    assert!(state.canvas_key(arrow(1, 0, false)));
    assert!(state.canvas_key(arrow(0, -1, false)));
    state.wait();
    assert_eq!(text(&state), "rectangle 12 7 30 22");
    assert!(state.canvas_key(arrow(1, 0, true)));
    state.wait();
    assert!(state.canvas_key(arrow(0, -1, true)));
    state.wait();
    assert_eq!(text(&state), "rectangle 12 7 31 21");
    let lines = history(&state);
    assert_eq!(lines[1], "Translate Selection: x offset 1.0");
    assert_eq!(lines[3], "Translate Selection: y offset -1.0");
    assert_eq!(
        lines[4],
        "Specify Selection: selection rectangle 12 7 31 22"
    );
    let _ = id;
}

/// Three regions: two rectangles and an oval.
fn with_regions(name: &str) -> (AppState, DatasetId) {
    let (mut state, id) = state(name);
    for roi in [
        Roi::rectangle(4.0, 4.0, 10.0, 8.0),
        Roi::rectangle(40.0, 10.0, 20.0, 6.0),
        Roi::oval(20.0, 25.0, 16.0, 12.0),
    ] {
        state.run("select.set", &Args::new().with("selection", roi));
        state.run("regions.add", &Args::new());
    }
    (state, id)
}

/// Chooses a command as a panel's button does, and lets it run.
fn request(state: &mut AppState, id: &str, args: Args) {
    state.request(id, args);
    for (id, args) in state.take_requests() {
        assert_eq!(state.invoke(&id, &args), Invocation::Started, "{id}");
    }
    state.wait();
}

#[test]
fn regions_are_added_picked_and_made_the_selection() {
    let (mut state, id) = with_regions("regions.png");
    assert_eq!(state.regions().len(), 3);
    // The one just added is picked, and its panel is on show.
    assert_eq!(state.selected_regions(id).iter().collect::<Vec<_>>(), [&2]);
    assert!(state.panel_open(Panel::Regions));
    assert_eq!(
        state.status,
        Status::Done("Add to Regions finished; the image has 3 regions".to_string())
    );

    // A plain click makes the region the selection, as a command.
    let steps = history(&state).len();
    state.click_region(0, RowClick::default());
    state.wait();
    assert!(state.regions_have_attention());
    assert_eq!(text(&state), "rectangle 4 4 10 8");
    assert_eq!(history(&state).last().unwrap(), "Select Region: region 1");
    // Clicking it again changes nothing, so nothing is recorded.
    state.click_region(0, RowClick::default());
    state.wait();
    assert_eq!(history(&state).len(), steps + 1);

    // Shift and Ctrl only change which regions are picked.
    let extend = RowClick {
        extend: true,
        toggle: false,
    };
    let toggle = RowClick {
        extend: false,
        toggle: true,
    };
    state.click_region(2, extend);
    assert_eq!(state.selected_regions(id).len(), 3);
    state.click_region(1, toggle);
    assert_eq!(
        state.selected_regions(id).iter().collect::<Vec<_>>(),
        [&0, &2]
    );
    state.wait();
    assert_eq!(history(&state).len(), steps + 1);
    assert_eq!(text(&state), "rectangle 4 4 10 8");
    assert_eq!(
        state.region_args().get("regions"),
        Some(&calipix_command::Value::from("1,3"))
    );
    state.select_all_regions();
    assert_eq!(state.selected_regions(id).len(), 3);
    state.clear_region_selection();
    assert!(state.selected_regions(id).is_empty());

    // Undo puts the old selection back.
    state.undo();
    assert_eq!(text(&state), "oval 20 25 16 12");
}

#[test]
fn a_click_on_the_image_picks_the_region_there() {
    let (mut state, id) = with_regions("pick.png");
    state.run("select.none", &Args::new());
    choose(&mut state, Tool::Pointer);
    click(&mut state, id, at(50.0, 13.0));
    assert_eq!(state.selected_regions(id).iter().collect::<Vec<_>>(), [&1]);
    assert_eq!(text(&state), "rectangle 40 10 20 6");
    assert_eq!(history(&state).last().unwrap(), "Select Region: region 2");

    // A click on the bare image lets go of both.
    click(&mut state, id, at(2.0, 44.0));
    assert!(state.selected_regions(id).is_empty());
    assert_eq!(roi(&state), None);

    // A drawing tool does not pick: it starts a shape of its own.
    choose(&mut state, Tool::Rectangle);
    click(&mut state, id, at(8.0, 8.0));
    assert!(state.selected_regions(id).is_empty());
    // And hidden regions are not there to be picked.
    choose(&mut state, Tool::Pointer);
    state.run("view.toggle_regions", &Args::new());
    assert!(!state.regions_shown());
    assert_eq!(state.mark("view.toggle_regions"), Some(Mark::Toggle(false)));
    click(&mut state, id, at(50.0, 13.0));
    assert!(state.selected_regions(id).is_empty());
    assert_eq!(state.region_at(id, [50.0, 13.0], 1.0), None);
}

#[test]
fn picked_regions_are_deleted_renamed_and_measured() {
    let (mut state, id) = with_regions("edit-regions.png");
    // Rename: typed in the list, kept with Enter.
    state.begin_rename(1);
    let renaming = state.renaming().expect("a name is being typed");
    assert_eq!((renaming.index, renaming.text.as_str()), (1, ""));
    renaming.text = "Bright".to_string();
    state.end_rename(true);
    state.wait();
    assert_eq!(state.regions()[1].props.name.as_deref(), Some("Bright"));
    assert_eq!(
        history(&state).last().unwrap(),
        "Rename Region: region 2, name Bright"
    );
    // Given up with Escape, or left as it was: nothing runs.
    let steps = history(&state).len();
    state.begin_rename(1);
    state.renaming().unwrap().text = "Other".to_string();
    state.end_rename(false);
    state.begin_rename(1);
    state.end_rename(true);
    state.wait();
    assert_eq!(history(&state).len(), steps);

    // Measure: the picked regions, or all of them.
    state.clear_region_selection();
    state.click_region(0, RowClick::default());
    state.click_region(
        1,
        RowClick {
            extend: true,
            toggle: false,
        },
    );
    state.wait();
    let args = state.region_args();
    request(&mut state, "regions.measure", args);
    let table = state.session().workspace.table("Results").unwrap();
    assert_eq!(table.len(), 2);
    assert_eq!(table.get(0, "Area").as_number(), Some(80.0));
    assert_eq!(table.get(1, "Area").as_number(), Some(120.0));
    assert!(state.panel_open(Panel::Results));
    assert_eq!(
        history(&state).last().unwrap(),
        "Measure Regions: regions 1-2"
    );

    // Delete: the key removes the picked regions while the image or the
    // list has the keyboard.
    assert!(state.canvas_key(CanvasKey::Delete));
    state.wait();
    assert_eq!(state.regions().len(), 1);
    assert_eq!(state.regions()[0].roi.kind(), Kind::Oval);
    assert!(state.selected_regions(id).is_empty());
    assert_eq!(
        history(&state).last().unwrap(),
        "Delete Regions: regions 1-2"
    );
    // With none picked the key has nothing to act on.
    assert!(!state.canvas_key(CanvasKey::Delete));
    state.undo();
    assert_eq!(state.regions().len(), 3);
}

#[test]
fn region_commands_say_what_they_need() {
    let (mut state, _) = state("needs.png");
    assert_eq!(
        state.unavailable_reason("regions.add").as_deref(),
        Some("Make a selection first")
    );
    for id in [
        "regions.measure",
        "regions.delete",
        "regions.save",
        "regions.clear",
    ] {
        assert_eq!(
            state.unavailable_reason(id).as_deref(),
            Some("The image has no regions"),
            "{id}"
        );
    }
    assert_eq!(state.unavailable_reason("regions.open"), None);
    state.run(
        "select.set",
        &Args::new().with("selection", Roi::rectangle(1.0, 1.0, 5.0, 5.0)),
    );
    assert_eq!(state.unavailable_reason("regions.add"), None);
    state.run("regions.add", &Args::new());
    assert_eq!(state.unavailable_reason("regions.measure"), None);
    // Saving and opening ask for a file and nothing else.
    for id in ["regions.save", "regions.open"] {
        assert!(
            matches!(state.invoke(id, &Args::new()), Invocation::PickPath { .. }),
            "{id}"
        );
    }
}

#[test]
fn labels_and_visibility_are_ways_of_looking_not_changes() {
    let (mut state, _) = with_regions("looking.png");
    let steps = history(&state).len();
    assert_eq!(state.region_labels(), RegionLabels::Numbers);
    state.run("view.region_labels_names", &Args::new());
    assert_eq!(state.region_labels(), RegionLabels::Names);
    assert_eq!(
        state.mark("view.region_labels_names"),
        Some(Mark::Choice(true))
    );
    state.run("view.toggle_regions", &Args::new());
    assert!(!state.regions_shown());
    state.run("view.toggle_regions", &Args::new());
    assert!(state.regions_shown());
    assert_eq!(history(&state).len(), steps);
}

#[test]
fn the_list_and_the_table_never_both_have_the_keyboard() {
    let (mut state, _) = with_regions("attention.png");
    state.run("select.none", &Args::new());
    state.run("measure.measure", &Args::new());
    state.click_region(0, RowClick::default());
    state.wait();
    assert!(state.regions_have_attention() && !state.table_has_attention());
    state.click_row(0, RowClick::default());
    assert!(state.table_has_attention() && !state.regions_have_attention());
    state.set_regions_attention(true);
    assert!(!state.table_has_attention());
    // A list that is no longer drawn gives the keyboard up.
    state.note_regions_drawn();
    assert!(state.regions_were_drawn());
    assert!(state.regions_have_attention());
    assert!(!state.regions_were_drawn());
    assert!(!state.regions_have_attention());
}

#[test]
fn closing_the_image_ends_what_was_under_way() {
    let (mut state, id) = with_regions("close.png");
    choose(&mut state, Tool::Polygon);
    click(&mut state, id, at(10.0, 8.0));
    assert!(state.gesture_active());
    state.run("file.close", &Args::new());
    assert!(!state.gesture_active());
    assert!(state.regions().is_empty());
    assert!(state.selected_regions(id).is_empty());
}
