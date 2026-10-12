//! The Plot panel: a table that is a profile, a histogram or a
//! distribution, drawn as a chart with the painter.
//!
//! The chart is only another way of looking at the table. "List" shows
//! the same numbers in the Results panel, and Copy and Save write them.

use std::f32::consts::FRAC_PI_2;

use calipix_command::Args;
use eframe::egui::{
    self, Align, Align2, Layout, Rect, Sense, Stroke, StrokeKind, pos2, vec2,
};

use super::{results, ui};
use crate::state::tables::ticks;
use crate::state::{AppState, PlotData, PlotKind};

/// Room around the chart for the axis labels.
const BELOW: f32 = 34.0;
const ABOVE: f32 = 6.0;
const RIGHT: f32 = 12.0;

pub fn show(ui: &mut egui::Ui, state: &mut AppState) {
    ui::panel_frame().show(ui, |ui| {
        let names = state.plot_names();
        let (Some(shown), Some(plot)) = (state.shown_plot(), state.plot()) else {
            return ui::empty(
                ui,
                "Analyze > Plot Profile, Histogram and Distribution draw their plots here",
            );
        };
        let log_id = egui::Id::new(("plot_log_scale", &shown));
        let mut log = ui.data(|data| data.get_temp(log_id)).unwrap_or(false);
        ui.horizontal(|ui| {
            if let Some(name) = results::switcher(ui, "plot_choice", &names, &shown) {
                state.show_plot(&name);
            }
            let count = plot.points.len();
            ui::weak(
                ui,
                match plot.kind {
                    PlotKind::Line => format!("{count} points"),
                    PlotKind::Bars => format!("{count} bins"),
                },
            );
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let unavailable = state.unavailable_reason("measure.save_results");
                let save = ui
                    .add_enabled(unavailable.is_none(), egui::Button::new("Save…"))
                    .on_hover_text("Save the numbers as comma-separated values (.csv)");
                if save.clicked() {
                    let args = Args::new().with("table", shown.as_str());
                    state.request("measure.save_results", args);
                }
                let copy = ui.button("Copy");
                if copy
                    .on_hover_text("Copy the numbers, to paste into a spreadsheet")
                    .clicked()
                    && let Some(text) = state.copy_plot(&shown)
                {
                    ui.ctx().copy_text(text);
                }
                let list = ui.button("List");
                if list
                    .on_hover_text("Show the numbers in the Results panel")
                    .clicked()
                {
                    state.list_plot(&shown);
                }
                if plot.kind == PlotKind::Bars {
                    ui.separator();
                    let toggle = ui.checkbox(&mut log, "Log scale");
                    let hint = "Show counts on a logarithmic scale, so that rare values show up";
                    if toggle.on_hover_text(hint).changed() {
                        ui.data_mut(|data| data.insert_temp(log_id, log));
                    }
                }
            });
        });
        ui.add_space(2.0);
        chart(ui, &plot, log && plot.kind == PlotKind::Bars);
    });
}

/// Where values fall on the screen.
struct Scale {
    area: Rect,
    x: (f64, f64),
    y: (f64, f64),
    log: bool,
}

impl Scale {
    fn x_of(&self, value: f64) -> f32 {
        let t = (value - self.x.0) / (self.x.1 - self.x.0);
        self.area.left() + self.area.width() * t as f32
    }

    fn value_at(&self, x: f32) -> f64 {
        let t = ((x - self.area.left()) / self.area.width().max(1.0)) as f64;
        self.x.0 + (self.x.1 - self.x.0) * t
    }

    fn y_of(&self, value: f64) -> f32 {
        let t = if self.log {
            // Counts: zero sits on the axis, and every decade above one
            // gets the same height.
            value.max(0.0).ln_1p() / self.y.1.max(1.0).ln_1p()
        } else {
            (value - self.y.0) / (self.y.1 - self.y.0)
        };
        self.area.bottom() - self.area.height() * t.clamp(0.0, 1.0) as f32
    }
}

/// The marks along the value axis: round numbers, or on a logarithmic
/// scale the powers of ten.
fn y_ticks(scale: &Scale) -> Vec<f64> {
    if !scale.log {
        let wanted = (scale.area.height() / 28.0).clamp(2.0, 6.0) as usize;
        return ticks(scale.y.0, scale.y.1, wanted);
    }
    let mut marks = vec![0.0];
    let mut decade = 1.0;
    while decade <= scale.y.1 {
        marks.push(decade);
        decade *= 10.0;
    }
    marks
}

fn chart(ui: &mut egui::Ui, plot: &PlotData, log: bool) {
    let size = ui.available_size().max(vec2(160.0, 90.0));
    let (rect, response) = ui.allocate_exact_size(size, Sense::hover());
    let name = &plot.name;
    response
        .widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Other, true, format!("Plot: {name}")));
    if !ui.is_rect_visible(rect) {
        return;
    }
    let visuals = ui.visuals().clone();
    let painter = ui.painter_at(rect);
    let small = egui::TextStyle::Small.resolve(ui.style());
    let weak = visuals.weak_text_color();

    let (y_low, y_high) = plot.y_span();
    // A little air above the tallest value, so it does not touch the frame.
    let y_span = match plot.kind {
        PlotKind::Bars => (0.0, y_high * 1.04),
        PlotKind::Line => {
            let margin = (y_high - y_low) * 0.05;
            (y_low - margin, y_high + margin)
        }
    };
    // The value labels decide how much room the axis on the left needs.
    let mut scale = Scale {
        area: rect,
        x: plot.x_span(),
        y: y_span,
        log,
    };
    let label_width = |value: f64| {
        painter
            .layout_no_wrap(ui::number_text(value), small.clone(), weak)
            .size()
            .x
    };
    let widest = y_ticks(&scale)
        .into_iter()
        .map(label_width)
        .fold(18.0, f32::max);
    let left = widest + 26.0;
    scale.area = Rect::from_min_max(
        pos2(rect.left() + left, rect.top() + ABOVE),
        pos2(rect.right() - RIGHT, rect.bottom() - BELOW),
    );
    let area = scale.area;
    if area.width() < 20.0 || area.height() < 20.0 {
        return;
    }
    painter.rect_filled(area, 2.0, visuals.extreme_bg_color);
    let frame = visuals.widgets.noninteractive.bg_stroke;
    let grid = Stroke::new(1.0, frame.color.gamma_multiply(0.45));

    // Value axis: a line across for each mark, its number left of it.
    for tick in y_ticks(&scale) {
        let y = scale.y_of(tick);
        if y < area.top() - 0.5 || y > area.bottom() + 0.5 {
            continue;
        }
        painter.hline(area.x_range(), y, grid);
        painter.text(
            pos2(area.left() - 5.0, y),
            Align2::RIGHT_CENTER,
            ui::number_text(tick),
            small.clone(),
            weak,
        );
    }
    // Along the bottom: a mark and a number at each round place.
    let wanted = (area.width() / 70.0).clamp(2.0, 12.0) as usize;
    for tick in ticks(scale.x.0, scale.x.1, wanted) {
        let x = scale.x_of(tick);
        painter.vline(x, area.y_range(), grid);
        painter.text(
            pos2(x, area.bottom() + 3.0),
            Align2::CENTER_TOP,
            ui::number_text(tick),
            small.clone(),
            weak,
        );
    }
    painter.text(
        pos2(area.center().x, rect.bottom() - 1.0),
        Align2::CENTER_BOTTOM,
        &plot.x_label,
        small.clone(),
        visuals.text_color(),
    );
    // The value axis is named along it, reading upwards.
    let y_label = if log {
        format!("{} (log)", plot.y_label)
    } else {
        plot.y_label.clone()
    };
    let galley = painter.layout_no_wrap(y_label, small.clone(), visuals.text_color());
    let at = pos2(
        rect.left() + 1.0,
        area.center().y + galley.size().x / 2.0,
    );
    painter.add(egui::epaint::TextShape::new(at, galley, visuals.text_color()).with_angle(-FRAC_PI_2));

    // The data, clipped to the chart.
    let data = painter.with_clip_rect(area);
    let color = visuals.text_color();
    match plot.kind {
        PlotKind::Bars => {
            let fill = color.gamma_multiply(0.85);
            for &(start, count) in &plot.points {
                if !(start.is_finite() && count.is_finite()) || count <= 0.0 {
                    continue;
                }
                let left = scale.x_of(start);
                let right = scale.x_of(start + plot.bin_width).max(left + 1.0);
                // Wide bars keep a hairline between them.
                let right = if right - left > 4.0 { right - 1.0 } else { right };
                let top = scale.y_of(count).min(area.bottom() - 1.0);
                data.rect_filled(
                    Rect::from_min_max(pos2(left, top), pos2(right, area.bottom())),
                    0.0,
                    fill,
                );
            }
        }
        PlotKind::Line => {
            // A value that is not a number breaks the line.
            let mut run: Vec<egui::Pos2> = Vec::new();
            let stroke = Stroke::new(1.5, color);
            let flush = |run: &mut Vec<egui::Pos2>| {
                match run.len() {
                    0 => {}
                    1 => {
                        data.circle_filled(run[0], 2.0, color);
                    }
                    _ => {
                        data.add(egui::Shape::line(std::mem::take(run), stroke));
                    }
                }
                run.clear();
            };
            for &(x, y) in &plot.points {
                if x.is_finite() && y.is_finite() {
                    run.push(pos2(scale.x_of(x), scale.y_of(y)));
                } else {
                    flush(&mut run);
                }
            }
            flush(&mut run);
        }
    }
    painter.rect_stroke(area, 2.0, frame, StrokeKind::Inside);
    hover(ui, &response, plot, &scale);
}

/// The point under the pointer: a line marking the place, and its numbers.
fn hover(ui: &egui::Ui, response: &egui::Response, plot: &PlotData, scale: &Scale) {
    let Some(pointer) = response.hover_pos().filter(|at| scale.area.contains(*at)) else {
        return;
    };
    let Some(index) = plot.index_at(scale.value_at(pointer.x)) else {
        return;
    };
    let (x, y) = plot.points[index];
    let painter = ui.painter_at(scale.area);
    let mark = Stroke::new(1.0, ui.visuals().weak_text_color());
    let text = match plot.kind {
        PlotKind::Line => {
            let at = pos2(scale.x_of(x), scale.y_of(y));
            painter.vline(at.x, scale.area.y_range(), mark);
            if y.is_finite() {
                painter.circle_stroke(at, 3.5, Stroke::new(1.5, ui.visuals().strong_text_color()));
            }
            format!(
                "{} {}\n{} {}",
                plot.x_label,
                ui::number_text(x),
                plot.y_label,
                ui::number_text(y)
            )
        }
        PlotKind::Bars => {
            painter.vline(pointer.x, scale.area.y_range(), mark);
            // A bin one value wide is that value; wider ones are a range.
            let bin = if plot.bin_width == 1.0 {
                ui::number_text(x)
            } else {
                format!(
                    "{} to {}",
                    ui::number_text(x),
                    ui::number_text(x + plot.bin_width)
                )
            };
            format!(
                "{} {bin}\n{} {}",
                plot.x_label,
                plot.y_label,
                ui::number_text(y)
            )
        }
    };
    response.clone().on_hover_text_at_pointer(text);
}
