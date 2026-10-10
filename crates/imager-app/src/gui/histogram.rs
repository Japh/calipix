//! The Histogram panel: the distribution of sample values in the plane on
//! screen, with the statistics ImageJ reports beside its histograms.
//!
//! Channels shown together are drawn together, over one value axis and one
//! count axis. The numbers come from `imager-render` through the
//! application's cache, so they are ImageJ's and are not counted again
//! while only the display changes.

use std::sync::Arc;

use eframe::egui::{self, Color32, RichText};
use imager_core::{AxisKind, ColorModel};

use super::{display, ui};
use crate::state::{AppState, HistogramState, PlaneHistogram, view};

const PLOT_HEIGHT: f32 = 104.0;

/// One channel's histogram, ready to draw.
struct Shown {
    name: String,
    color: Color32,
    data: Arc<PlaneHistogram>,
}

pub fn show(ui: &mut egui::Ui, state: &mut AppState) {
    let Some(id) = state.active().map(|dataset| dataset.id) else {
        return ui::empty(ui, "Open an image to see its histogram");
    };
    let Some(image) = state.displayed(id) else {
        return;
    };
    let meta = image.meta();
    let rgb = meta.color == ColorModel::Rgb;
    let count = meta.size(&AxisKind::Channel);
    let position = state.position(id);
    // The channels on screen: all the visible ones of a composite, the
    // three of an RGB image, or the one selected.
    let channels: Vec<usize> = if rgb {
        (0..count).collect()
    } else if view::shown_together(meta) {
        (0..count)
            .filter(|&c| state.channel_visible(id, c))
            .collect()
    } else {
        vec![view::channel_at(meta, &position)]
    };

    let log_id = egui::Id::new("histogram_log_scale");
    let mut log = ui.data(|data| data.get_temp(log_id)).unwrap_or(false);
    ui.horizontal(|ui| {
        if ui
            .checkbox(&mut log, "Log scale")
            .on_hover_text("Show counts on a logarithmic scale, so that rare values show up")
            .changed()
        {
            ui.data_mut(|data| data.insert_temp(log_id, log));
        }
    });

    if channels.is_empty() {
        return ui::empty(ui, "Every channel is hidden");
    }
    let mut shown = Vec::new();
    let mut pending = false;
    for &channel in &channels {
        let Ok(plane) = meta.plane_index(&view::with_channel(meta, &position, channel)) else {
            continue;
        };
        match state.histogram(id, plane, false) {
            HistogramState::Ready(data) => shown.push(Shown {
                name: display::channel_name(&image, channel),
                color: if rgb {
                    let primary = [[255, 0, 0], [0, 255, 0], [0, 0, 255]][channel.min(2)];
                    ui::series_color(ui, &imager_core::Lut::from_fn("", |_| primary).table)
                } else if count > 1 {
                    ui::series_color(ui, &display::channel_lut(&image, channel).table)
                } else {
                    ui.visuals().text_color()
                },
                data,
            }),
            HistogramState::Pending => pending = true,
            HistogramState::Failed(problem) => {
                ui::problem(ui, &problem);
            }
        }
    }
    if shown.is_empty() {
        if pending {
            ui::weak(ui, "Counting…");
        }
        return;
    }

    let series: Vec<ui::Series<'_>> = shown
        .iter()
        .map(|one| ui::Series {
            histogram: &one.data.histogram,
            color: one.color,
        })
        .collect();
    let plot = ui::histogram_plot(ui, PLOT_HEIGHT, &series, log, None, egui::Sense::hover());
    plot.response.widget_info(|| {
        egui::WidgetInfo::labeled(
            egui::WidgetType::Other,
            true,
            "Histogram of the plane shown",
        )
    });
    hover(ui, &plot, &shown);

    // The ends of the value axis, under the ends of the plot.
    let (low, high) = plot.span;
    let integer = meta.dtype().is_integer();
    ui.horizontal(|ui| {
        ui.label(RichText::new(ui::number_text(low)).weak().small());
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            // Integer bins end one above the largest value they hold.
            let last = if integer { high - 1.0 } else { high };
            ui.label(RichText::new(ui::number_text(last)).weak().small());
            if pending {
                ui.label(RichText::new("Counting…").weak().small());
            }
        });
    });
    ui.add_space(2.0);
    statistics(ui, &shown);

    let sampled = shown.iter().filter_map(|one| one.data.sampled).next();
    if let Some(fraction) = sampled {
        ui.add_space(2.0);
        ui.add(
            egui::Label::new(
                RichText::new(format!(
                    "Estimated from {:.1}% of the plane, which is too large to read whole",
                    fraction * 100.0
                ))
                .weak()
                .small(),
            )
            .wrap(),
        );
    }
}

/// The value under the pointer and how many samples each channel has
/// there, with a line marking the place.
fn hover(ui: &egui::Ui, plot: &ui::Plot, shown: &[Shown]) {
    let Some(pointer) = plot.response.hover_pos() else {
        return;
    };
    let value = plot.value_at(pointer.x);
    ui.painter_at(plot.rect).line_segment(
        [
            egui::pos2(pointer.x, plot.rect.top()),
            egui::pos2(pointer.x, plot.rect.bottom()),
        ],
        egui::Stroke::new(1.0, ui.visuals().weak_text_color()),
    );
    let mut lines = Vec::new();
    for one in shown {
        let histogram = &one.data.histogram;
        let offset = (value - histogram.hist_min) / histogram.bin_width;
        let inside = offset >= 0.0 && (offset as usize) < histogram.counts.len();
        let (range, count) = if inside {
            let bin = offset as usize;
            let start = histogram.bin_start(bin);
            // A bin one value wide is that value; wider ones are a range.
            let range = if histogram.bin_width == 1.0 {
                ui::number_text(start)
            } else {
                format!(
                    "{} to {}",
                    ui::number_text(start),
                    ui::number_text(histogram.bin_start(bin + 1))
                )
            };
            (range, histogram.counts[bin])
        } else {
            (ui::number_text(value), 0)
        };
        lines.push(if shown.len() > 1 {
            format!("{}: value {range}, count {count}", one.name)
        } else {
            format!("Value {range}, count {count}")
        });
    }
    plot.response
        .clone()
        .on_hover_text_at_pointer(lines.join("\n"));
}

/// Count, mean and the rest: a row each, a column per channel.
fn statistics(ui: &mut egui::Ui, shown: &[Shown]) {
    type Pick = fn(&imager_render::Histogram) -> f64;
    let rows: [(&str, Pick); 6] = [
        ("Count", |h| h.count as f64),
        ("Mean", |h| h.mean),
        ("Std dev", |h| h.std_dev),
        ("Min", |h| h.min),
        ("Max", |h| h.max),
        ("Mode", |h| h.mode),
    ];
    egui::Grid::new("histogram_statistics")
        .num_columns(shown.len() + 1)
        .spacing([ui::GAP, 3.0])
        .show(ui, |ui| {
            if shown.len() > 1 {
                // The legend: each column is headed by its channel's
                // colour and name.
                ui.label("");
                for one in shown {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 4.0;
                        let (mark, _) =
                            ui.allocate_exact_size(egui::vec2(8.0, 8.0), egui::Sense::hover());
                        ui.painter().rect_filled(mark, 2.0, one.color);
                        ui.label(RichText::new(&one.name).small());
                    });
                }
                ui.end_row();
            }
            for (name, pick) in rows {
                ui::weak(ui, name);
                for one in shown {
                    let histogram = &one.data.histogram;
                    let mut text = ui::number_text(pick(histogram));
                    if name == "Mode" {
                        text = format!("{text} ({})", histogram.mode_count);
                    }
                    ui.label(text);
                }
                ui.end_row();
            }
        });
}
