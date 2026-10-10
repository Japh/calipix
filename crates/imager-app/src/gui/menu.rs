//! The menu bar, drawn from [`AppState::menu`]. Nothing here knows any
//! particular command.

use eframe::egui;
use imager_command::Args;

use crate::state::{AppState, Mark, MenuCommand, MenuEntry};

/// Draws the menu bar and returns the command chosen from it, if any, with
/// the arguments its menu item carries.
pub fn show(ui: &mut egui::Ui, state: &AppState) -> Option<(String, Args)> {
    let mut chosen = None;
    egui::MenuBar::new().ui(ui, |ui| {
        entries(ui, state, state.menu(), &mut chosen);
    });
    chosen
}

fn entries(
    ui: &mut egui::Ui,
    state: &AppState,
    items: &[MenuEntry],
    chosen: &mut Option<(String, Args)>,
) {
    for item in items {
        match item {
            MenuEntry::Separator => {
                ui.separator();
            }
            MenuEntry::Submenu { title, items } => {
                ui.menu_button(title, |ui| {
                    // Room between the longest label and its shortcut.
                    ui.set_min_width(190.0);
                    entries(ui, state, items, chosen);
                });
            }
            MenuEntry::Command(command) => {
                if item_clicked(ui, state, command) {
                    *chosen = Some((command.id.clone(), command.args.clone()));
                }
            }
        }
    }
}

fn item_clicked(ui: &mut egui::Ui, state: &AppState, command: &MenuCommand) -> bool {
    let unavailable = state.unavailable_with(&command.id, &command.args);
    let label = command.label.as_str();
    let response = ui
        .add_enabled_ui(unavailable.is_none(), |ui| {
            // On or off is shown by the shape of the mark, not by colour.
            match state.mark(&command.id) {
                Some(Mark::Toggle(mut on)) => ui.checkbox(&mut on, label),
                Some(Mark::Choice(on)) => ui.radio(on, label),
                None => {
                    let mut button = egui::Button::new(label);
                    // Items with preset arguments are not what the shortcut runs.
                    if command.args.is_empty()
                        && let Some(shortcut) = state.shortcut_text(&command.id)
                    {
                        button = button.shortcut_text(shortcut);
                    }
                    ui.add(button)
                }
            }
        })
        .inner;
    let clicked = response.clicked();
    let summary = state.spec(&command.id).map(|spec| spec.summary.as_str());
    match (unavailable, command.hint.as_deref().or(summary)) {
        (Some(reason), _) => {
            response.on_disabled_hover_text(reason);
        }
        (None, Some(hint)) if !hint.is_empty() => {
            response.on_hover_text(hint);
        }
        _ => {}
    }
    clicked
}
