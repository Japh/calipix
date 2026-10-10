//! The one set of widgets and measurements every panel and dialog uses, so
//! that spacing, number fields and buttons look the same everywhere.

use eframe::egui::{self, Color32, RichText};

use crate::state::Theme;

/// Behind images, in both themes, so brightness is judged consistently.
pub const IMAGE_BACKGROUND: Color32 = Color32::from_gray(58);
/// Text drawn on [`IMAGE_BACKGROUND`].
pub const ON_IMAGE_BACKGROUND: Color32 = Color32::from_gray(205);

/// Space between unrelated groups of controls.
pub const GAP: f32 = 10.0;
/// Margin inside panels and dialogs.
pub const MARGIN: i8 = 8;

/// Sets the spacing and text sizes shared by both themes. Call once.
pub fn apply_style(ctx: &egui::Context) {
    use egui::{FontFamily::Proportional, FontId, TextStyle};
    ctx.all_styles_mut(|style| {
        style.spacing.item_spacing = egui::vec2(8.0, 5.0);
        style.spacing.button_padding = egui::vec2(8.0, 3.0);
        style.spacing.interact_size.y = 22.0;
        style.spacing.menu_margin = egui::Margin::same(6);
        style
            .text_styles
            .insert(TextStyle::Body, FontId::new(13.5, Proportional));
        style
            .text_styles
            .insert(TextStyle::Button, FontId::new(13.5, Proportional));
        style
            .text_styles
            .insert(TextStyle::Small, FontId::new(11.5, Proportional));
        style
            .text_styles
            .insert(TextStyle::Heading, FontId::new(17.0, Proportional));
    });
    // The stock error red is hard to read on either background.
    ctx.style_mut_of(egui::Theme::Dark, |style| {
        style.visuals.error_fg_color = Color32::from_rgb(255, 120, 110);
    });
    ctx.style_mut_of(egui::Theme::Light, |style| {
        style.visuals.error_fg_color = Color32::from_rgb(185, 28, 28);
    });
}

pub fn apply_theme(ctx: &egui::Context, theme: Theme) {
    let preference = match theme {
        Theme::System => egui::ThemePreference::System,
        Theme::Light => egui::ThemePreference::Light,
        Theme::Dark => egui::ThemePreference::Dark,
    };
    if ctx.options(|o| o.theme_preference) != preference {
        ctx.set_theme(preference);
    }
}

/// Frame around the contents of a panel.
pub fn panel_frame() -> egui::Frame {
    egui::Frame::new().inner_margin(MARGIN)
}

/// The button that carries out a dialog's purpose.
pub fn primary_button(ui: &mut egui::Ui, text: &str, enabled: bool) -> egui::Response {
    let fill = ui.visuals().selection.bg_fill;
    let color = ui.visuals().selection.stroke.color;
    let button = egui::Button::new(RichText::new(text).color(color))
        .fill(fill)
        .min_size(egui::vec2(72.0, 0.0));
    ui.add_enabled(enabled, button)
}

/// Any other dialog button.
pub fn button(ui: &mut egui::Ui, text: &str) -> egui::Response {
    ui.add(egui::Button::new(text).min_size(egui::vec2(72.0, 0.0)))
}

/// Secondary text: units, paths, explanations.
pub fn weak(ui: &mut egui::Ui, text: impl Into<String>) -> egui::Response {
    ui.label(RichText::new(text).weak())
}

/// A problem, in the error colour and marked with a sign so that colour is
/// not the only signal.
pub fn problem(ui: &mut egui::Ui, text: &str) -> egui::Response {
    let color = ui.visuals().error_fg_color;
    ui.label(RichText::new(format!("⚠ {text}")).color(color))
}

/// Centred text for a panel with nothing to show.
pub fn empty(ui: &mut egui::Ui, text: &str) {
    ui.centered_and_justified(|ui| {
        weak(ui, text);
    });
}

/// One line of a name and value table. Call inside [`properties`].
pub fn property(ui: &mut egui::Ui, name: &str, value: impl Into<String>) {
    weak(ui, name);
    ui.label(value.into());
    ui.end_row();
}

/// A two-column table of names and values.
pub fn properties(ui: &mut egui::Ui, id: &str, add_rows: impl FnOnce(&mut egui::Ui)) {
    egui::Grid::new(id)
        .num_columns(2)
        .spacing([GAP * 1.5, 4.0])
        .show(ui, add_rows);
}

/// What a number field needs to know about its parameter.
#[derive(Clone, Copy, Debug, Default)]
pub struct NumberRange {
    pub min: Option<f64>,
    pub max: Option<f64>,
    pub step: Option<f64>,
    pub integer: bool,
}

impl NumberRange {
    fn speed(&self) -> f64 {
        let fallback = match (self.min, self.max) {
            (Some(min), Some(max)) => (max - min) / 200.0,
            _ => 0.1,
        };
        let speed = self.step.unwrap_or(fallback);
        if self.integer { speed.max(0.1) } else { speed }
    }
}

/// The number field: type a value or drag it, with a slider beside it when
/// the range is bounded on both sides. Values outside the range can still
/// be typed; the caller says what is wrong with them.
pub fn number(ui: &mut egui::Ui, value: &mut f64, range: NumberRange) -> egui::Response {
    fn field(value: &mut f64, range: NumberRange) -> egui::DragValue<'_> {
        let drag = egui::DragValue::new(value).speed(range.speed());
        if range.integer {
            drag.fixed_decimals(0)
        } else {
            drag.max_decimals(6)
        }
    }
    let (Some(min), Some(max)) = (range.min, range.max) else {
        return ui.add(field(value, range));
    };
    ui.horizontal(|ui| {
        let mut slider = egui::Slider::new(value, min..=max)
            .show_value(false)
            .clamping(egui::SliderClamping::Never);
        if range.integer {
            slider = slider.integer();
        } else if let Some(step) = range.step {
            slider = slider.step_by(step);
        }
        let slid = ui.add(slider);
        ui.add(field(value, range)) | slid
    })
    .inner
}
