//! The status bar: on the left, the outcome of the last action or the
//! progress of the running one; on the right, readouts for the active
//! image.

use calipix_view::Probe;
use eframe::egui;

use super::ui;
use super::viewer::ViewerResponse;
use crate::state::{AppState, Status};

/// What the person did in the status bar.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StatusEvents {
    /// Cancel was pressed on the running command.
    pub cancel: bool,
    /// The zoom readout was double-clicked, which fits the image.
    pub fit: bool,
}

/// Draws the status bar.
pub fn show(ui: &mut egui::Ui, state: &AppState, view: &ViewerResponse) -> StatusEvents {
    let mut events = StatusEvents::default();
    ui.horizontal(|ui| {
        ui.set_min_height(22.0);
        // Laid out first so that a long message is cut short by the
        // readouts rather than pushing them off the window.
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            events.fit = readouts(ui, state, view);
            ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                events.cancel = message(ui, state);
            });
        });
    });
    events
}

fn message(ui: &mut egui::Ui, state: &AppState) -> bool {
    if let Some(running) = state.running().filter(|r| r.visible()) {
        ui.label(&running.title);
        let mut bar = egui::ProgressBar::new(running.fraction.unwrap_or(0.0) as f32)
            .desired_width(180.0)
            .desired_height(14.0);
        if running.fraction.is_some() {
            bar = bar.show_percentage();
        } else {
            bar = bar.animate(true);
        }
        ui.add(bar);
        let cancel = ui
            .small_button("Cancel")
            .on_hover_text("Stop this command and discard its result")
            .clicked();
        ui::weak(ui, running.message);
        return cancel;
    }
    match &state.status {
        Status::Idle => ui::weak(ui, "Ready"),
        Status::Done(text) => ui.add(egui::Label::new(text).truncate()),
        Status::Failed(text) => {
            let color = ui.visuals().error_fg_color;
            let text = egui::RichText::new(format!("⚠ {text}")).color(color);
            ui.add(egui::Label::new(text).truncate())
                .on_hover_text("Recorded in the Log panel")
        }
    };
    false
}

/// What is under the pointer, as the status bar words it: the pixel, its
/// place in calibrated units if the image has any, and the sample there
/// (one per channel for composite and RGB images).
pub fn probe_text(probe: &Probe) -> String {
    let mut text = format!("x {}, y {}", probe.x, probe.y);
    let [x, y] = &probe.calibrated;
    if let (Some(x_unit), Some(y_unit)) = (&x.unit, &y.unit) {
        let (x_value, y_value) = (ui::number_text(x.value), ui::number_text(y.value));
        if x_unit == y_unit {
            text.push_str(&format!("   ({x_value}, {y_value} {x_unit})"));
        } else {
            text.push_str(&format!("   ({x_value} {x_unit}, {y_value} {y_unit})"));
        }
    }
    let values: Vec<String> = probe
        .values
        .iter()
        // A sample that has not been read yet shows as a dash.
        .map(|v| v.value.map_or_else(|| "–".to_string(), ui::number_text))
        .collect();
    match values.as_slice() {
        [] => {}
        [one] => text.push_str(&format!("   value {one}")),
        many => text.push_str(&format!("   values {}", many.join(", "))),
    }
    text
}

/// Right to left: the active image, the zoom, the pixel under the pointer.
/// The viewer fills the last two in. Returns true if the zoom readout was
/// double-clicked.
fn readouts(ui: &mut egui::Ui, state: &AppState, view: &ViewerResponse) -> bool {
    let Some(dataset) = state.active() else {
        return false;
    };
    let mut fit = false;
    let meta = dataset.image.meta();
    let planes = match meta.plane_count() {
        1 => String::new(),
        n => format!("   {n} planes"),
    };
    ui.label(format!(
        "{}   {} × {}   {}{planes}",
        meta.name,
        meta.width(),
        meta.height(),
        meta.dtype().name(),
    ));
    ui.separator();
    if let Some(zoom) = view.zoom {
        let percent = zoom * 100.0;
        let digits = if percent < 10.0 { 1 } else { 0 };
        let readout = egui::Label::new(format!("{percent:.digits$}%")).sense(egui::Sense::click());
        fit = ui
            .add(readout)
            .on_hover_text("Zoom. Double-click to fit the image to its tab")
            .double_clicked();
        if view.loading {
            // Quiet on purpose: parts of the picture are still arriving.
            ui.add(egui::Spinner::new().size(12.0))
                .on_hover_text("Reading the image");
            ui::weak(ui, "Loading");
        }
        ui.separator();
    }
    if let Some(probe) = &view.probe {
        ui.label(probe_text(probe));
        ui.separator();
    }
    fit
}
