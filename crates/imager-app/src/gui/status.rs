//! The status bar: on the left, the outcome of the last action or the
//! progress of the running one; on the right, readouts for the active
//! image.

use eframe::egui;

use super::ui;
use super::viewer::ViewerResponse;
use crate::state::{AppState, Status};

/// Draws the status bar. Returns true if Cancel was pressed.
pub fn show(ui: &mut egui::Ui, state: &AppState, view: &ViewerResponse) -> bool {
    let mut cancel = false;
    ui.horizontal(|ui| {
        ui.set_min_height(22.0);
        // Laid out first so that a long message is cut short by the
        // readouts rather than pushing them off the window.
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            readouts(ui, state, view);
            ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                cancel = message(ui, state);
            });
        });
    });
    cancel
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

/// Right to left: the active image, the zoom, the pixel under the pointer.
/// The viewer fills the last two in.
fn readouts(ui: &mut egui::Ui, state: &AppState, view: &ViewerResponse) {
    let Some(dataset) = state.active() else {
        return;
    };
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
    if let Some(scale) = view.scale {
        let percent = scale * 100.0;
        let digits = if percent < 10.0 { 1 } else { 0 };
        ui.add_sized(
            [44.0, 0.0],
            egui::Label::new(format!("{percent:.digits$}%")),
        );
        ui.separator();
    }
    let position = view
        .cursor
        .map_or_else(String::new, |(x, y)| format!("x {x}, y {y}"));
    ui.add_sized([96.0, 0.0], egui::Label::new(position));
    ui.separator();
}
