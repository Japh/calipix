//! The selection tools and the Regions panel, driven with real pointer
//! and key events through the accessibility tree, without a window or a
//! GPU: the viewer draws through its CPU path.
#![cfg(feature = "gui")]

use std::path::PathBuf;

use calipix_app::gui::Shell;
use calipix_app::state::settings::Settings;
use calipix_app::state::{AppState, Status, Tool};
use calipix_roi::Roi;
use eframe::egui::accesskit::{Role, Toggled};
use eframe::egui::{self, Key, Modifiers, PointerButton};
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT as _, Queryable as _};

type H = Harness<'static, Shell>;

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/imagej/out")
        .join(name)
}

/// The application with `blobs.tif` (64 by 48 pixels) open.
fn harness() -> H {
    let mut state = AppState::build(
        calipix_commands::standard_registry(),
        Settings::default(),
        None,
    );
    state.open(fixture("blobs.tif"));
    assert!(
        !matches!(state.status, Status::Failed(_)),
        "{:?}",
        state.status
    );
    let mut harness = Harness::builder()
        .with_size(egui::vec2(1400.0, 1000.0))
        .with_max_steps(24)
        .build_ui_state(|ui, shell: &mut Shell| shell.ui(ui), Shell::new(state));
    harness.step();
    catch_up(&mut harness);
    harness
}

/// Runs frames until the interface stops asking to be redrawn. Work on
/// other threads keeps it redrawing for as long as it takes, so this
/// waits in real time, up to a generous limit.
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

/// Where a position in the image is in the window.
fn on(h: &H, x: f64, y: f64) -> egui::Pos2 {
    let place = h.state().view().place.expect("the image is on show");
    place.screen([x, y])
}

fn button(h: &H, at: egui::Pos2, button: PointerButton, pressed: bool, modifiers: Modifiers) {
    h.event(egui::Event::PointerButton {
        pos: at,
        button,
        pressed,
        modifiers,
    });
}

/// Presses the primary button at one place in the image, moves to the
/// other in steps as a hand would, and releases.
fn drag(h: &mut H, from: (f64, f64), to: (f64, f64)) {
    let (a, b) = (on(h, from.0, from.1), on(h, to.0, to.1));
    h.hover_at(a);
    h.step();
    button(h, a, PointerButton::Primary, true, Modifiers::NONE);
    h.step();
    for step in 1..=4 {
        h.hover_at(a + (b - a) * (step as f32 / 4.0));
        h.step();
    }
    button(h, b, PointerButton::Primary, false, Modifiers::NONE);
    settle(h);
}

/// A click at a place in the image, with modifier keys held.
fn click_with(h: &mut H, at: (f64, f64), modifiers: Modifiers) {
    let at = on(h, at.0, at.1);
    h.event(egui::Event::ModifiersChanged(modifiers));
    h.hover_at(at);
    h.step();
    button(h, at, PointerButton::Primary, true, modifiers);
    h.step();
    button(h, at, PointerButton::Primary, false, modifiers);
    h.event(egui::Event::ModifiersChanged(Modifiers::NONE));
    settle(h);
}

fn click(h: &mut H, at: (f64, f64)) {
    click_with(h, at, Modifiers::NONE);
}

fn press(h: &mut H, modifiers: Modifiers, key: Key) {
    h.key_press_modifiers(modifiers, key);
    settle(h);
}

fn undo(h: &mut H) {
    press(h, Modifiers::COMMAND, Key::Z);
}

fn choose(h: &mut H, tool: Tool) {
    let name = format!("{} Tool", tool.title());
    h.get_by_role_and_label(Role::RadioButton, &name).click();
    settle(h);
    assert_eq!(h.state().state.tool(), tool);
}

fn roi(h: &H) -> Option<Roi> {
    h.state().state.active().unwrap().roi.clone()
}

fn text(h: &H) -> String {
    roi(h).map_or_else(|| "none".to_string(), |roi| roi.to_string())
}

/// The titles of the steps in effect, after the opening of the image.
fn titles(h: &H) -> Vec<String> {
    let rows = h.state().state.history_rows();
    rows.into_iter()
        .skip(1)
        .filter(|row| !row.undone)
        .map(|row| row.title)
        .collect()
}

fn regions(h: &H) -> usize {
    h.state().state.regions().len()
}

fn has(h: &H, label: &str) -> bool {
    h.query_all_by_label(label).next().is_some()
}

fn is_selected(node: &egui_kittest::Node<'_>) -> bool {
    node.accesskit_node().toggled() == Some(Toggled::True)
}

#[test]
fn the_tool_strip_names_its_tools_and_shows_the_one_in_use() {
    let mut h = harness();
    for tool in Tool::ALL {
        let name = format!("{} Tool", tool.title());
        assert!(
            h.query_by_role_and_label(Role::RadioButton, &name)
                .is_some(),
            "no button called {name}"
        );
    }
    assert!(is_selected(
        &h.get_by_role_and_label(Role::RadioButton, "Pointer Tool")
    ));
    choose(&mut h, Tool::Oval);
    assert!(is_selected(
        &h.get_by_role_and_label(Role::RadioButton, "Oval Tool")
    ));
    assert!(!is_selected(
        &h.get_by_role_and_label(Role::RadioButton, "Pointer Tool")
    ));
    // The status bar says how to use it.
    assert!(
        h.query_by_label_contains("Drag to select an oval")
            .is_some()
    );
    // Its key chooses a tool as well, being the command's shortcut.
    press(&mut h, Modifiers::NONE, Key::L);
    assert_eq!(h.state().state.tool(), Tool::Line);
    press(&mut h, Modifiers::NONE, Key::V);
    assert_eq!(h.state().state.tool(), Tool::Pointer);
    assert!(titles(&h).is_empty());
}

#[test]
fn a_rectangle_is_dragged_moved_and_resized_one_step_each() {
    let mut h = harness();
    choose(&mut h, Tool::Rectangle);

    drag(&mut h, (10.0, 8.0), (40.0, 30.0));
    assert_eq!(roi(&h), Some(Roi::rectangle(10.0, 8.0, 30.0, 22.0)));
    assert_eq!(titles(&h), ["Specify Selection"]);
    assert!(
        has(&h, "Rectangle 30 × 22 at 10, 8"),
        "the status bar describes the selection"
    );

    // From its inside it moves, with the drawing tool still chosen.
    drag(&mut h, (20.0, 20.0), (25.0, 17.0));
    assert_eq!(text(&h), "rectangle 15 5 30 22");
    assert_eq!(titles(&h), ["Specify Selection", "Translate Selection"]);

    // By its bottom right handle it is resized.
    drag(&mut h, (45.0, 27.0), (50.0, 35.0));
    assert_eq!(text(&h), "rectangle 15 5 35 30");
    assert_eq!(
        titles(&h),
        [
            "Specify Selection",
            "Translate Selection",
            "Specify Selection"
        ]
    );

    // A click inside changes nothing; one on the bare image clears.
    click(&mut h, (30.0, 20.0));
    assert_eq!(titles(&h).len(), 3);
    click(&mut h, (60.0, 44.0));
    assert_eq!(roi(&h), None);
    assert_eq!(titles(&h).last().unwrap(), "Select None");

    // Undo walks back through each gesture.
    for expected in [
        "rectangle 15 5 35 30",
        "rectangle 15 5 30 22",
        "rectangle 10 8 30 22",
        "none",
    ] {
        undo(&mut h);
        assert_eq!(text(&h), expected);
    }
    assert!(titles(&h).is_empty());
}

#[test]
fn each_tool_draws_its_shape_on_the_image() {
    let mut h = harness();
    // The image is magnified well beyond 150%, where lines and points
    // keep their place within the pixel; these are pressed on pixel
    // centres.
    assert!(h.state().view().zoom.unwrap() > 1.5);

    choose(&mut h, Tool::Oval);
    drag(&mut h, (8.0, 6.0), (30.0, 26.0));
    assert_eq!(text(&h), "oval 8 6 22 20");

    choose(&mut h, Tool::Line);
    drag(&mut h, (50.5, 5.5), (60.5, 40.5));
    assert_eq!(text(&h), "line 50 5 60 40");
    assert!(
        h.query_by_label_contains("Line   length 36.4 pixels")
            .is_some()
    );

    choose(&mut h, Tool::Polygon);
    for corner in [(10.0, 8.0), (40.0, 6.0), (50.0, 30.0), (20.0, 42.0)] {
        click(&mut h, corner);
        // Nothing is recorded until the shape is finished.
        assert_eq!(titles(&h).len(), 2);
    }
    assert!(h.state().state.drawing_path());
    click(&mut h, (10.0, 8.0));
    assert_eq!(text(&h), "polygon 10,8 40,6 50,30 20,42");
    assert_eq!(titles(&h).len(), 3);

    choose(&mut h, Tool::Point);
    click(&mut h, (5.5, 44.5));
    click_with(&mut h, (30.5, 20.5), Modifiers::SHIFT);
    click_with(&mut h, (55.5, 44.5), Modifiers::SHIFT);
    assert_eq!(text(&h), "points 5,44 30,20 55,44");
    assert!(has(&h, "3 points"));

    choose(&mut h, Tool::Freehand);
    drag(&mut h, (4.0, 30.0), (12.0, 38.0));
    assert!(matches!(roi(&h), Some(Roi::Freehand { points }) if points.len() >= 3));

    choose(&mut h, Tool::Polyline);
    for point in [(6.5, 40.5), (20.5, 10.5), (36.5, 34.5), (36.5, 34.5)] {
        click(&mut h, point);
    }
    assert_eq!(text(&h), "polyline 6,40 20,10 36,34");
    assert_eq!(titles(&h).len(), 8);
    assert!(titles(&h).iter().all(|title| title == "Specify Selection"));
}

#[test]
fn escape_gives_up_a_shape_in_progress() {
    let mut h = harness();
    choose(&mut h, Tool::Polygon);
    click(&mut h, (10.0, 8.0));
    click(&mut h, (40.0, 6.0));
    assert!(h.state().state.drawing_path());
    press(&mut h, Modifiers::NONE, Key::Escape);
    assert!(!h.state().state.gesture_active());
    assert_eq!(roi(&h), None);
    assert!(titles(&h).is_empty());

    // In the middle of a drag as well.
    choose(&mut h, Tool::Rectangle);
    let (a, b) = (on(&h, 10.0, 8.0), on(&h, 30.0, 20.0));
    h.hover_at(a);
    h.step();
    button(&h, a, PointerButton::Primary, true, Modifiers::NONE);
    h.step();
    h.hover_at(b);
    h.step();
    assert!(h.state().state.gesture_active());
    h.key_press(Key::Escape);
    h.step();
    assert!(!h.state().state.gesture_active());
    button(&h, b, PointerButton::Primary, false, Modifiers::NONE);
    settle(&mut h);
    assert_eq!(roi(&h), None);
    assert!(titles(&h).is_empty());
}

#[test]
fn regions_are_added_renamed_measured_and_deleted_from_their_panel() {
    let mut h = harness();
    choose(&mut h, Tool::Rectangle);
    drag(&mut h, (4.0, 4.0), (14.0, 12.0));
    // T keeps the selection as a region, and the panel comes forward.
    press(&mut h, Modifiers::NONE, Key::T);
    assert_eq!(regions(&h), 1);
    assert!(h.query_by_label_contains("draw a selection").is_none());
    assert!(is_selected(
        &h.get_by_role_and_label(Role::Button, "Region 1")
    ));
    drag(&mut h, (25.0, 40.0), (45.0, 43.0));
    h.get_by_role_and_label(Role::Button, "Add").click();
    settle(&mut h);
    assert_eq!(regions(&h), 2);
    assert!(has(&h, "2 regions, 1 selected"));

    // A click on a row picks the region and makes it the selection.
    h.get_by_role_and_label(Role::Button, "Region 1").click();
    settle(&mut h);
    assert_eq!(text(&h), "rectangle 4 4 10 8");
    assert_eq!(titles(&h).last().unwrap(), "Select Region");

    // Rename types the name in the row; Enter keeps it.
    h.get_by_role_and_label(Role::Button, "Rename").click();
    settle(&mut h);
    h.get_by_role(Role::TextInput).type_text("Nucleus");
    h.step();
    h.key_press(Key::Enter);
    settle(&mut h);
    assert!(h.query_by_role_and_label(Role::Button, "Nucleus").is_some());
    assert_eq!(titles(&h).last().unwrap(), "Rename Region");

    // Measure, with none picked, measures them all into Results.
    press(&mut h, Modifiers::NONE, Key::Escape);
    assert!(has(&h, "2 regions"));
    h.get_by_role_and_label(Role::Button, "Measure").click();
    settle(&mut h);
    let rows = |h: &H| {
        let workspace = &h.state().state.session().workspace;
        workspace.table("Results").map_or(0, |table| table.len())
    };
    assert_eq!(rows(&h), 2);
    assert!(has(&h, "2 rows"));
    assert_eq!(titles(&h).last().unwrap(), "Measure Regions");

    // Delete removes the picked region; the key does the same.
    h.get_by_role_and_label(Role::Button, "Region 2").click();
    settle(&mut h);
    h.get_by_role_and_label(Role::Button, "Delete").click();
    settle(&mut h);
    assert_eq!(regions(&h), 1);
    h.get_by_role_and_label(Role::Button, "Nucleus").click();
    settle(&mut h);
    press(&mut h, Modifiers::NONE, Key::Delete);
    assert_eq!(regions(&h), 0);
    assert!(h.query_by_label_contains("draw a selection").is_some());

    // Undo walks back through every step: both deletions, the picking
    // of regions, the measuring, the renaming, and so on to the start.
    let mut seen = Vec::new();
    while h.state().state.can_undo() && seen.len() < 20 {
        let title = titles(&h).pop();
        undo(&mut h);
        seen.push((title.unwrap_or_default(), regions(&h), rows(&h)));
    }
    let steps: Vec<(&str, usize, usize)> = seen
        .iter()
        .map(|(title, regions, rows)| (title.as_str(), *regions, *rows))
        .collect();
    assert_eq!(
        steps,
        [
            ("Delete Regions", 1, 2),
            ("Select Region", 1, 2),
            ("Delete Regions", 2, 2),
            ("Select Region", 2, 2),
            ("Measure Regions", 2, 0),
            ("Rename Region", 2, 0),
            ("Select Region", 2, 0),
            ("Add to Regions", 1, 0),
            ("Specify Selection", 1, 0),
            ("Add to Regions", 0, 0),
            ("Specify Selection", 0, 0),
            // The image itself.
            ("", 0, 0),
        ]
    );
}

#[test]
fn the_wheel_zooms_and_space_or_the_middle_button_pans_with_a_tool_chosen() {
    let mut h = harness();
    choose(&mut h, Tool::Rectangle);
    let zoom = |h: &H| h.state().view().zoom.unwrap();
    let corner = |h: &H| on(h, 0.0, 0.0);

    // The wheel zooms about the pointer and draws nothing.
    let before = zoom(&h);
    h.hover_at(on(&h, 32.0, 24.0));
    h.step();
    for _ in 0..2 {
        h.event(egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Line,
            delta: egui::vec2(0.0, 1.0),
            phase: egui::TouchPhase::Move,
            modifiers: Modifiers::NONE,
        });
        settle(&mut h);
    }
    assert!(zoom(&h) > before, "{} > {before}", zoom(&h));
    assert_eq!(roi(&h), None);

    // Space and a drag pans: the image moves, and no rectangle is drawn.
    let was = corner(&h);
    h.key_down(Key::Space);
    h.step();
    drag(&mut h, (20.0, 15.0), (30.0, 25.0));
    h.key_up(Key::Space);
    settle(&mut h);
    let now = corner(&h);
    assert!(
        now.x > was.x + 20.0 && now.y > was.y + 20.0,
        "{was:?} to {now:?}"
    );
    assert_eq!(roi(&h), None);
    assert!(!h.state().state.gesture_active());

    // So does a drag with the middle button.
    let was = corner(&h);
    let (a, b) = (on(&h, 30.0, 25.0), on(&h, 20.0, 15.0));
    h.hover_at(a);
    h.step();
    button(&h, a, PointerButton::Middle, true, Modifiers::NONE);
    h.step();
    for step in 1..=4 {
        h.hover_at(a + (b - a) * (step as f32 / 4.0));
        h.step();
    }
    assert!(!h.state().state.gesture_active());
    button(&h, b, PointerButton::Middle, false, Modifiers::NONE);
    settle(&mut h);
    let now = corner(&h);
    assert!(
        now.x < was.x - 20.0 && now.y < was.y - 20.0,
        "{was:?} to {now:?}"
    );
    assert_eq!(roi(&h), None);

    // Space pressed in the middle of drawing turns the drag into a pan:
    // the shape is given up, not left half made.
    let (a, b) = (on(&h, 20.0, 15.0), on(&h, 26.0, 21.0));
    h.hover_at(a);
    h.step();
    button(&h, a, PointerButton::Primary, true, Modifiers::NONE);
    h.step();
    h.hover_at(a + (b - a) * 0.5);
    h.step();
    assert!(h.state().state.gesture_active());
    h.key_down(Key::Space);
    h.step();
    assert!(!h.state().state.gesture_active());
    h.hover_at(b);
    h.step();
    button(&h, b, PointerButton::Primary, false, Modifiers::NONE);
    h.key_up(Key::Space);
    settle(&mut h);
    assert_eq!(roi(&h), None);
    assert!(titles(&h).is_empty(), "{:?}", titles(&h));

    // And the tool still draws afterwards, at the new zoom and place.
    drag(&mut h, (20.0, 15.0), (30.0, 25.0));
    assert_eq!(text(&h), "rectangle 20 15 10 10");
}

#[test]
fn arrows_nudge_the_selection_and_otherwise_keep_their_shortcuts() {
    let mut h = harness();
    choose(&mut h, Tool::Rectangle);
    drag(&mut h, (10.0, 8.0), (40.0, 30.0));
    press(&mut h, Modifiers::NONE, Key::ArrowRight);
    press(&mut h, Modifiers::NONE, Key::ArrowDown);
    assert_eq!(text(&h), "rectangle 11 9 30 22");
    press(&mut h, Modifiers::ALT, Key::ArrowRight);
    assert_eq!(text(&h), "rectangle 11 9 31 22");
    assert_eq!(
        titles(&h),
        [
            "Specify Selection",
            "Translate Selection",
            "Translate Selection",
            "Specify Selection"
        ]
    );
    // Ctrl+A and Ctrl+Shift+A are still Select All and Select None.
    press(&mut h, Modifiers::COMMAND | Modifiers::SHIFT, Key::A);
    assert_eq!(roi(&h), None);
    press(&mut h, Modifiers::COMMAND, Key::A);
    assert_eq!(text(&h), "rectangle 0 0 64 48");
}

#[test]
fn select_all_and_none_act_on_whichever_list_has_the_keyboard() {
    let mut h = harness();
    choose(&mut h, Tool::Rectangle);
    for (from, to) in [((4.0, 4.0), (14.0, 12.0)), ((25.0, 40.0), (45.0, 43.0))] {
        drag(&mut h, from, to);
        press(&mut h, Modifiers::NONE, Key::T);
    }
    // With none picked, Measure measures both.
    h.state_mut().state.clear_region_selection();
    h.step();
    h.get_by_role_and_label(Role::Button, "Measure").click();
    settle(&mut h);
    let id = h.state().state.active().unwrap().id;
    let picked = |h: &H| h.state().state.selected_regions(id).len();
    let selection = text(&h);

    // The list of regions has the keyboard after a click in it.
    h.get_by_role_and_label(Role::Button, "Region 1").click();
    settle(&mut h);
    let selection_after_click = text(&h);
    press(&mut h, Modifiers::COMMAND, Key::A);
    assert_eq!(picked(&h), 2);
    press(&mut h, Modifiers::COMMAND | Modifiers::SHIFT, Key::A);
    assert_eq!(picked(&h), 0);
    assert_eq!(
        text(&h),
        selection_after_click,
        "the image's selection is untouched"
    );

    // Then the Results table, after a click in it.
    h.get_by_label("Row 1").click();
    settle(&mut h);
    press(&mut h, Modifiers::COMMAND, Key::A);
    assert_eq!(h.state().state.selected_rows().len(), 2);
    assert_eq!(picked(&h), 0);
    press(&mut h, Modifiers::COMMAND | Modifiers::SHIFT, Key::A);
    assert!(h.state().state.selected_rows().is_empty());
    assert_eq!(text(&h), selection_after_click);

    // And the image again, after a click on it.
    choose(&mut h, Tool::Pointer);
    click(&mut h, (60.0, 2.0));
    press(&mut h, Modifiers::COMMAND, Key::A);
    assert_eq!(text(&h), "rectangle 0 0 64 48");
    let _ = selection;
}
