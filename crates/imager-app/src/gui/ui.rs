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
/// Margin inside panels.
pub const MARGIN: i8 = 8;
/// Width of the text fields and drop-downs in dialogs.
pub const FIELD_WIDTH: f32 = 240.0;
/// Fill of the button that carries out a dialog's purpose; white text
/// reads on it in both themes.
const ACCENT: Color32 = Color32::from_rgb(0, 102, 184);

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

/// Frame of dialogs and other boxes shown over the window.
pub fn modal_frame(style: &egui::Style) -> egui::Frame {
    egui::Frame::popup(style).inner_margin(16)
}

/// The button that carries out a dialog's purpose.
pub fn primary_button(ui: &mut egui::Ui, text: &str, enabled: bool) -> egui::Response {
    let button = if enabled {
        egui::Button::new(RichText::new(text).color(Color32::WHITE)).fill(ACCENT)
    } else {
        egui::Button::new(text)
    };
    ui.add_enabled(enabled, button.min_size(egui::vec2(72.0, 0.0)))
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

/// A small heading that starts a group of controls within a panel.
pub fn section(ui: &mut egui::Ui, text: &str) -> egui::Response {
    ui.label(RichText::new(text).strong())
}

/// A number as panels show it: whole numbers without decimals, others to
/// three decimals or four significant digits, whichever is more.
pub fn number_text(value: f64) -> String {
    if !value.is_finite() {
        return "–".to_string();
    }
    if value == value.trunc() && value.abs() < 1e15 {
        return format!("{value:.0}");
    }
    let magnitude = value.abs().log10().floor() as i32;
    if !(-4..9).contains(&magnitude) {
        return format!("{value:.3e}");
    }
    // Three decimals, or enough to show four digits of a small number.
    let decimals = (3 - magnitude).clamp(3, 8) as usize;
    let text = format!("{value:.decimals$}");
    if text.contains('.') {
        text.trim_end_matches('0').trim_end_matches('.').to_string()
    } else {
        text
    }
}

/// A lookup table drawn as a strip from its first entry to its last.
pub fn lut_strip(ui: &mut egui::Ui, table: &[[u8; 3]; 256], size: egui::Vec2) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::hover());
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        let steps = (rect.width().round() as usize).clamp(2, 256);
        for step in 0..steps {
            let [r, g, b] = table[step * 255 / (steps - 1)];
            let left = rect.left() + rect.width() * step as f32 / steps as f32;
            // Overlapping its neighbour, so that no gap shows between
            // steps that do not land on whole pixels.
            let right = (rect.left() + rect.width() * (step + 1) as f32 / steps as f32 + 0.5)
                .min(rect.right());
            painter.rect_filled(
                egui::Rect::from_min_max(
                    egui::pos2(left, rect.top()),
                    egui::pos2(right, rect.bottom()),
                ),
                0.0,
                Color32::from_rgb(r, g, b),
            );
        }
        let outline = ui.visuals().widgets.noninteractive.bg_stroke;
        painter.rect_stroke(rect, 1.0, outline, egui::StrokeKind::Outside);
    }
    response
}

/// The colour that stands for a lookup table in a chart: its brighter
/// end, moved towards the text colour so that it reads on the panel in
/// either theme. Grey tables take the text colour itself.
pub fn series_color(ui: &egui::Ui, table: &[[u8; 3]; 256]) -> Color32 {
    let luminance = |[r, g, b]: [u8; 3]| 0.299 * r as f32 + 0.587 * g as f32 + 0.114 * b as f32;
    let end = if luminance(table[255]) >= luminance(table[0]) {
        table[255]
    } else {
        table[0]
    };
    let text = ui.visuals().text_color();
    let [r, g, b] = end;
    if r.abs_diff(g) < 24 && g.abs_diff(b) < 24 {
        return text;
    }
    let mix = |c: u8, t: u8| (c as f32 * 0.78 + t as f32 * 0.22).round() as u8;
    Color32::from_rgb(mix(r, text.r()), mix(g, text.g()), mix(b, text.b()))
}

/// One histogram in a [`histogram_plot`].
pub struct Series<'a> {
    pub histogram: &'a imager_render::Histogram,
    pub color: Color32,
}

/// Where a histogram plot ended up, for drawing over it and reading
/// positions off it.
pub struct Plot {
    pub response: egui::Response,
    /// The area the bars occupy.
    pub rect: egui::Rect,
    /// The sample values at the left and right edges.
    pub span: (f64, f64),
}

impl Plot {
    /// The screen x of a sample value.
    pub fn x_of(&self, value: f64) -> f32 {
        let (low, high) = self.span;
        let t = if high > low {
            ((value - low) / (high - low)).clamp(0.0, 1.0)
        } else {
            0.0
        };
        self.rect.left() + self.rect.width() * t as f32
    }

    /// The sample value at a screen x.
    pub fn value_at(&self, x: f32) -> f64 {
        let (low, high) = self.span;
        let t = ((x - self.rect.left()) / self.rect.width().max(1.0)).clamp(0.0, 1.0);
        low + (high - low) * t as f64
    }
}

/// The value range a histogram's bins cover.
pub fn histogram_span(histogram: &imager_render::Histogram) -> (f64, f64) {
    let low = histogram.hist_min;
    (
        low,
        low + histogram.counts.len() as f64 * histogram.bin_width,
    )
}

/// Draws histograms over one shared value axis and one shared count axis,
/// `height` tall and as wide as there is room. `span` widens the value
/// axis beyond the histograms' own range. A single very full bin (a black
/// background, say) is cut off rather than allowed to flatten the rest, as
/// ImageJ does; `log` shows counts on a logarithmic scale instead.
pub fn histogram_plot(
    ui: &mut egui::Ui,
    height: f32,
    series: &[Series<'_>],
    log: bool,
    span: Option<(f64, f64)>,
    sense: egui::Sense,
) -> Plot {
    let width = ui.available_width().max(40.0);
    let (rect, response) = ui.allocate_exact_size(egui::vec2(width, height), sense);
    let mut low = span.map_or(f64::INFINITY, |s| s.0);
    let mut high = span.map_or(f64::NEG_INFINITY, |s| s.1);
    for one in series {
        let (start, end) = histogram_span(one.histogram);
        low = low.min(start);
        high = high.max(end);
    }
    if !(low.is_finite() && high.is_finite()) || high <= low {
        (low, high) = (0.0, 1.0);
    }
    let plot = Plot {
        response,
        rect: rect.shrink(1.0),
        span: (low, high),
    };
    if !ui.is_rect_visible(rect) {
        return plot;
    }
    let visuals = ui.visuals();
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 2.0, visuals.extreme_bg_color);
    painter.rect_stroke(
        rect,
        2.0,
        visuals.widgets.noninteractive.bg_stroke,
        egui::StrokeKind::Inside,
    );

    // One count axis for every series.
    let mut top = 0u64;
    for one in series {
        let counts = &one.histogram.counts;
        let fullest = counts.iter().copied().max().unwrap_or(0);
        let second = counts
            .iter()
            .copied()
            .filter(|&c| c != fullest)
            .max()
            .unwrap_or(0);
        // `HistogramWindow`: a bin more than 1.5 times the next fullest
        // is drawn cut off at that height.
        let limit = if !log && second > 0 && fullest as f64 > second as f64 * 1.5 {
            (second as f64 * 1.5) as u64
        } else {
            fullest
        };
        top = top.max(limit);
    }
    let scale = |count: u64| -> f32 {
        if top == 0 || count == 0 {
            return 0.0;
        }
        let t = if log {
            (count as f64).ln_1p() / (top as f64).ln_1p()
        } else {
            count as f64 / top as f64
        };
        t.min(1.0) as f32
    };
    let area = plot.rect;
    let overlaid = series.len() > 1;
    for one in series {
        let histogram = one.histogram;
        let fill = if overlaid {
            one.color.gamma_multiply(0.45)
        } else {
            one.color.gamma_multiply(0.85)
        };
        let mut outline = Vec::with_capacity(histogram.counts.len() * 2);
        for (bin, &count) in histogram.counts.iter().enumerate() {
            let left = plot.x_of(histogram.bin_start(bin));
            let right = plot.x_of(histogram.bin_start(bin + 1)).max(left + 1.0);
            let y = area.bottom() - area.height() * scale(count);
            if count > 0 {
                painter.rect_filled(
                    egui::Rect::from_min_max(egui::pos2(left, y), egui::pos2(right, area.bottom())),
                    0.0,
                    fill,
                );
            }
            outline.push(egui::pos2(left, y));
            outline.push(egui::pos2(right, y));
        }
        if overlaid {
            painter.add(egui::Shape::line(
                outline,
                egui::Stroke::new(1.0, one.color),
            ));
        }
    }
    plot
}
