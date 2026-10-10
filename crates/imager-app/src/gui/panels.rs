//! Contents of the workspace panels. To add a panel, add a variant to
//! [`Panel`] and an arm to [`show`].

use eframe::egui::{self, RichText};
use imager_command::{Dataset, HistoryEntry, Value};
use imager_core::AxisKind;

use super::ui;
use crate::state::{AppState, LogLevel, Panel};

/// Draws a panel's contents.
pub fn show(ui: &mut egui::Ui, panel: Panel, state: &AppState) {
    ui::panel_frame().show(ui, |ui| {
        egui::ScrollArea::vertical()
            .id_salt(panel)
            .auto_shrink(false)
            .stick_to_bottom(panel == Panel::Log)
            .show(ui, |ui| match panel {
                Panel::Info => info(ui, state),
                Panel::History => history(ui, state),
                Panel::Log => log(ui, state),
            });
    });
}

fn info(ui: &mut egui::Ui, state: &AppState) {
    let Some(dataset) = state.active() else {
        return ui::empty(ui, "No image is open");
    };
    let meta = dataset.image.meta();
    ui.label(RichText::new(&meta.name).strong());
    if let Some(path) = &dataset.path {
        // Paths are long: wrapped under the name rather than cut off.
        ui.add(egui::Label::new(RichText::new(path.display().to_string()).weak().small()).wrap());
    }
    ui.add_space(ui::GAP / 2.0);
    ui::properties(ui, "info", |ui| {
        ui::property(
            ui,
            "Size",
            format!("{} × {} pixels", meta.width(), meta.height()),
        );
        ui::property(ui, "Type", meta.dtype().name());
        for (kind, name) in [
            (AxisKind::Channel, "Channels"),
            (AxisKind::Z, "Slices"),
            (AxisKind::Time, "Frames"),
        ] {
            if meta.axis_index(&kind).is_some() {
                ui::property(ui, name, meta.size(&kind).to_string());
            }
        }
        ui::property(ui, "Pixel size", calibration(dataset));
        ui::property(
            ui,
            "Memory",
            if dataset.image.is_in_memory() {
                "Loaded"
            } else {
                "Loaded on demand"
            },
        );
    });
}

/// The physical size of a pixel, e.g. `0.5 × 0.5 µm`.
fn calibration(dataset: &Dataset) -> String {
    let axes = dataset.image.meta().axes();
    let (x, y) = (&axes[0], &axes[1]);
    match (&x.unit, &y.unit) {
        (Some(x_unit), Some(y_unit)) if x_unit == y_unit => {
            format!("{} × {} {x_unit}", x.scale, y.scale)
        }
        (Some(x_unit), Some(y_unit)) => format!("{} {x_unit} × {} {y_unit}", x.scale, y.scale),
        _ => "Not calibrated".to_string(),
    }
}

fn history(ui: &mut egui::Ui, state: &AppState) {
    let history = state.session().history();
    let entries = history.entries();
    if entries.is_empty() {
        return ui::empty(ui, "Commands you run are listed here");
    }
    let applied = history.applied().len();
    egui::Grid::new("history")
        .num_columns(3)
        .spacing([ui::GAP, 3.0])
        .show(ui, |ui| {
            for (index, entry) in entries.iter().enumerate() {
                let undone = index >= applied;
                let title = RichText::new(format!("{}. {}", index + 1, entry.title));
                ui.horizontal(|ui| {
                    // The arrow marks the undo position: the last step in
                    // effect. Every row leaves room for it.
                    let (slot, _) = ui.allocate_exact_size(
                        egui::vec2(10.0, ui.spacing().interact_size.y),
                        egui::Sense::hover(),
                    );
                    if index + 1 == applied {
                        ui.painter().text(
                            slot.center(),
                            egui::Align2::CENTER_CENTER,
                            "▶",
                            egui::TextStyle::Small.resolve(ui.style()),
                            ui.visuals().text_color(),
                        );
                    }
                    if undone {
                        ui.label(title.weak().strikethrough());
                    } else {
                        ui.label(title);
                    }
                });
                if undone {
                    ui::weak(ui, "undone");
                } else {
                    ui::weak(ui, arguments(state, entry));
                }
                ui::weak(ui, format!("{:.0?}", entry.duration));
                ui.end_row();
            }
        });
}

/// The arguments of a history entry as `name value` pairs, with images
/// named rather than numbered where they are still open.
fn arguments(state: &AppState, entry: &HistoryEntry) -> String {
    entry
        .args
        .iter()
        .map(|(name, value)| {
            let text = match value {
                Value::Image(id) => state.session().workspace.get(*id).map_or_else(
                    |_| format!("image {id}"),
                    |dataset| dataset.image.meta().name.clone(),
                ),
                other => other.to_text(),
            };
            format!("{} {text}", name.replace('_', " "))
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn log(ui: &mut egui::Ui, state: &AppState) {
    if state.log().is_empty() {
        return ui::empty(ui, "Errors and results are recorded here");
    }
    for entry in state.log() {
        ui.horizontal_top(|ui| {
            ui::weak(ui, entry.time.format("%H:%M:%S").to_string());
            match entry.level {
                LogLevel::Info => ui.label(&entry.text),
                LogLevel::Error => ui::problem(ui, &entry.text),
            };
        });
    }
}
