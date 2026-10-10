//! Everything the desktop interface can do, exercised without a window.

use std::path::PathBuf;

use imager_app::state::{AppState, Status};
use imager_command::{Args, MenuItem};
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

fn titles(items: &[MenuItem]) -> Vec<&str> {
    items
        .iter()
        .map(|item| match item {
            MenuItem::Submenu { title, .. } | MenuItem::Command { title, .. } => title.as_str(),
        })
        .collect()
}

#[test]
fn menus_come_from_the_registry() {
    let state = AppState::new();
    assert_eq!(titles(state.menu()), ["File", "Image"]);
    let MenuItem::Submenu { items, .. } = &state.menu()[0] else {
        panic!("File is a menu");
    };
    assert_eq!(titles(items), ["Open...", "Save As...", "Close"]);
}

#[test]
fn commands_explain_why_they_are_unavailable() {
    let mut state = AppState::new();
    assert_eq!(
        state.unavailable_reason("image.invert").as_deref(),
        Some("Open an image first")
    );
    assert_eq!(
        state.unavailable_reason("file.open").as_deref(),
        Some("Needs: File")
    );
    assert!(state.unavailable_reason("no.such").is_some());

    state.open(sample("available.png"));
    assert_eq!(state.unavailable_reason("image.invert"), None);
    assert_eq!(
        state.unavailable_reason("file.save_as").as_deref(),
        Some("Needs: File")
    );
}

#[test]
fn open_run_undo_redo() {
    let mut state = AppState::new();
    assert!(state.active().is_none());
    assert!(!state.can_undo());

    state.open(sample("flow.png"));
    assert_eq!(state.status, Status::Done("Open".into()));
    assert_eq!(pixels(&state), [10, 250]);

    state.run("image.invert", &Args::new());
    assert_eq!(state.status, Status::Done("Invert".into()));
    assert_eq!(pixels(&state), [245, 5]);

    state.undo();
    assert_eq!(state.status, Status::Done("Undid Invert".into()));
    assert_eq!(pixels(&state), [10, 250]);
    assert!(state.can_redo());

    state.redo();
    assert_eq!(state.status, Status::Done("Redid Invert".into()));
    assert_eq!(pixels(&state), [245, 5]);
}

#[test]
fn failures_are_shown_and_harmless() {
    let mut state = AppState::new();
    state.open(temp("missing.png"));
    let Status::Failed(message) = &state.status else {
        panic!("expected a failure, got {:?}", state.status);
    };
    assert!(message.starts_with("Open: "), "{message}");
    assert!(message.contains("missing.png"), "{message}");
    assert!(state.active().is_none());

    state.undo();
    assert_eq!(state.status, Status::Failed("nothing to undo".into()));
}
