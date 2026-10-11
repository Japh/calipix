//! Contents of the workspace panels. To add a panel, add a variant to
//! [`Panel`] and an arm to [`show`]. The Display and Histogram panels have
//! modules of their own.

use eframe::egui::{self, RichText};
use imager_command::Dataset;
use imager_core::{AxisKind, ColorModel, DType, ImageMeta};
use imager_render::ChannelMode;

use super::{display, histogram, ui};
use crate::state::{AppState, LogLevel, Panel};

/// Draws a panel's contents.
pub fn show(ui: &mut egui::Ui, panel: Panel, state: &mut AppState) {
    ui::panel_frame().show(ui, |ui| {
        egui::ScrollArea::vertical()
            .id_salt(panel)
            .auto_shrink(false)
            .stick_to_bottom(panel == Panel::Log)
            .show(ui, |ui| match panel {
                Panel::Info => info(ui, state),
                Panel::Display => display::show(ui, state),
                Panel::Histogram => histogram::show(ui, state),
                Panel::History => history(ui, state),
                Panel::Log => log(ui, state),
            });
    });
}

/// The pixel type in the words ImageJ users know, with the exact type.
pub fn type_text(meta: &ImageMeta) -> String {
    let dtype = meta.dtype();
    let plain = match dtype {
        DType::U8 => "8-bit".to_string(),
        DType::U16 => "16-bit".to_string(),
        DType::F32 => "32-bit float".to_string(),
        DType::F64 => "64-bit float".to_string(),
        other if other.is_complex() => format!("{}-bit complex", other.size() * 8),
        other if other.is_signed() => format!("{}-bit signed", other.size() * 8),
        other => format!("{}-bit unsigned", other.size() * 8),
    };
    if meta.color == ColorModel::Rgb {
        format!("RGB, {plain} per channel ({dtype})")
    } else {
        format!("{plain} ({dtype})")
    }
}

/// A byte count in the largest unit that keeps it above one.
fn bytes_text(bytes: f64) -> String {
    const UNITS: [&str; 5] = ["bytes", "KB", "MB", "GB", "TB"];
    let mut value = bytes;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{value:.0} {}", UNITS[unit])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

/// The name and letter of an axis beyond X and Y, as the Info panel
/// labels its size.
fn axis_title(kind: &AxisKind) -> String {
    match kind {
        AxisKind::Channel => "Channels (C)".to_string(),
        AxisKind::Z => "Slices (Z)".to_string(),
        AxisKind::Time => "Frames (T)".to_string(),
        AxisKind::X => "Width (X)".to_string(),
        AxisKind::Y => "Height (Y)".to_string(),
        AxisKind::Other(name) => name.clone(),
    }
}

fn info(ui: &mut egui::Ui, state: &AppState) {
    let Some(dataset) = state.active() else {
        return ui::empty(ui, "No image is open");
    };
    let meta = dataset.image.meta();
    ui.label(RichText::new(&meta.name).strong());
    let place = match &dataset.path {
        Some(path) => path.display().to_string(),
        None => "Not saved to a file".to_string(),
    };
    // Paths are long: wrapped under the name rather than cut off.
    ui.add(egui::Label::new(RichText::new(place).weak().small()).wrap());
    ui.add_space(ui::GAP / 2.0);

    ui::properties(ui, "info", |ui| {
        ui::property(
            ui,
            "Size",
            format!("{} × {} pixels", meta.width(), meta.height()),
        );
        for (axis, &size) in meta.axes().iter().zip(meta.shape()).skip(2) {
            ui::property(ui, &axis_title(&axis.kind), size.to_string());
        }
        ui::property(ui, "Type", type_text(meta));
        if meta.size(&AxisKind::Channel) > 1 && meta.color != ColorModel::Rgb {
            let mode = match imager_commands::view::channel_mode(meta) {
                ChannelMode::Composite => "Composite",
                ChannelMode::Grayscale => "Grayscale",
                _ => "Color",
            };
            ui::property(ui, "Channel mode", mode);
        }
        for (name, value) in calibration(dataset) {
            ui::property(ui, name, value);
        }
        for (index, channel) in meta.channels.iter().enumerate() {
            if let Some(name) = &channel.name {
                ui::property(ui, &format!("Channel {}", index + 1), name.clone());
            }
        }
        let size = bytes_text(
            meta.plane_len() as f64 * meta.plane_count() as f64 * meta.dtype().size() as f64,
        );
        ui::property(
            ui,
            "Memory",
            if dataset.image.is_in_memory() {
                format!("{size}, in memory")
            } else {
                format!("{size}, read on demand")
            },
        );
    });

    // What the file says about itself: ImageJ's Info text, then whatever
    // other properties it carried.
    if let Some(text) = meta.properties.get("Info").filter(|t| !t.trim().is_empty()) {
        ui.add_space(ui::GAP);
        ui::section(ui, "Info");
        ui.add(egui::Label::new(RichText::new(text.trim_end()).small()).wrap());
    }
    let others: Vec<(&String, &String)> = meta
        .properties
        .iter()
        .filter(|(key, _)| key.as_str() != "Info")
        .collect();
    if !others.is_empty() {
        ui.add_space(ui::GAP);
        egui::CollapsingHeader::new(format!("Properties ({})", others.len()))
            .id_salt("info_properties")
            .show(ui, |ui| {
                for (key, value) in others {
                    // Encoded selections and the like run to kilobytes.
                    let short: String = value.chars().take(120).collect();
                    let value = if short.len() < value.len() {
                        format!("{short}…")
                    } else {
                        short
                    };
                    ui.horizontal_wrapped(|ui| {
                        ui::weak(ui, key.as_str());
                        ui.add(egui::Label::new(RichText::new(value).small()).wrap());
                    });
                }
            });
    }
}

/// The calibration of each axis that has one, as rows for the Info panel.
fn calibration(dataset: &Dataset) -> Vec<(&'static str, String)> {
    let meta = dataset.image.meta();
    let axes = meta.axes();
    let (x, y) = (&axes[0], &axes[1]);
    let number = ui::number_text;
    let mut rows = Vec::new();
    rows.push((
        "Pixel size",
        match (&x.unit, &y.unit) {
            (Some(x_unit), Some(y_unit)) if x_unit == y_unit => {
                format!("{} × {} {x_unit}", number(x.scale), number(y.scale))
            }
            (Some(x_unit), Some(y_unit)) => {
                format!(
                    "{} {x_unit} × {} {y_unit}",
                    number(x.scale),
                    number(y.scale)
                )
            }
            _ => "Not calibrated".to_string(),
        },
    ));
    if x.origin != 0.0 || y.origin != 0.0 {
        let unit = x.unit.clone().unwrap_or_default();
        rows.push((
            "Origin",
            format!("{}, {} {unit}", number(x.origin), number(y.origin)),
        ));
    }
    for (kind, name) in [
        (AxisKind::Z, "Slice spacing"),
        (AxisKind::Time, "Frame interval"),
    ] {
        let axis = meta.axis_index(&kind).map(|index| &axes[index]);
        if let Some(axis) = axis
            && let Some(unit) = &axis.unit
        {
            rows.push((name, format!("{} {unit}", number(axis.scale))));
        }
    }
    rows
}

fn history(ui: &mut egui::Ui, state: &mut AppState) {
    let rows = state.history_rows();
    if rows.is_empty() {
        return ui::empty(ui, "Commands you run are listed here");
    }
    let mut go_to = None;
    egui::Grid::new("history")
        .num_columns(3)
        .spacing([ui::GAP, 3.0])
        .show(ui, |ui| {
            for (index, row) in rows.iter().enumerate() {
                ui.horizontal(|ui| {
                    // The arrow marks the undo position: the last step in
                    // effect. Every row leaves room for it.
                    let (slot, _) = ui.allocate_exact_size(
                        egui::vec2(10.0, ui.spacing().interact_size.y),
                        egui::Sense::hover(),
                    );
                    if row.current {
                        ui.painter().text(
                            slot.center(),
                            egui::Align2::CENTER_CENTER,
                            "▶",
                            egui::TextStyle::Small.resolve(ui.style()),
                            ui.visuals().text_color(),
                        );
                    }
                    let mut title = RichText::new(format!("{}. {}", index + 1, row.title));
                    if row.undone {
                        title = title.weak().strikethrough();
                    }
                    // Clicking a step undoes or redoes up to it.
                    let step = ui.add(egui::Button::selectable(row.current, title).frame(false));
                    let hint = if row.current {
                        "The last step in effect"
                    } else if row.undone {
                        "Redo up to this step"
                    } else {
                        "Undo back to this step"
                    };
                    if step.on_hover_text(hint).clicked() && !row.current {
                        go_to = Some(index);
                    }
                });
                if row.undone {
                    ui::weak(ui, "undone");
                } else {
                    ui::weak(ui, row.detail.as_str());
                }
                ui::weak(ui, format!("{:.0?}", row.duration));
                ui.end_row();
            }
        });
    if let Some(index) = go_to {
        state.go_to_history(index);
    }
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
