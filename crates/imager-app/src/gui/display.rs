//! The Display panel: how the active image is shown, channel by channel.
//!
//! Every control here runs a display command, so what is done in the
//! panel appears in the History panel, can be undone and reads the same
//! in a macro. The one thing that is not a command is hiding a channel:
//! that changes the view, not the image.
//!
//! Dragging a range control would otherwise run a command for every
//! pixel moved. While a control is held the new range is only shown
//! ([`AppState::preview_display_range`]); letting go runs Set Display
//! Range once, with where the drag ended.

use eframe::egui::{self, Color32, RichText};
use imager_command::{Args, DatasetId};
use imager_core::{AxisKind, ColorModel, Image};
use imager_render::ChannelMode;

use super::ui;
use crate::state::{AppState, HistogramState, view};

const PLOT_HEIGHT: f32 = 46.0;

pub fn show(ui: &mut egui::Ui, state: &mut AppState) {
    let Some(id) = state.active().map(|dataset| dataset.id) else {
        return ui::empty(ui, "Open an image to adjust how it is shown");
    };
    let Some(image) = state.displayed(id) else {
        return;
    };
    let meta = image.meta();
    let channels = meta.size(&AxisKind::Channel);
    let rgb = meta.color == ColorModel::Rgb;
    let mode = imager_commands::view::channel_mode(meta);

    if rgb {
        // The three channels are one picture with one range.
        channel(ui, state, id, &image, None);
        return;
    }
    if channels > 1 {
        mode_selector(ui, state, id, mode);
        ui.add_space(2.0);
    }
    for index in 0..channels {
        if index > 0 {
            ui.add_space(2.0);
            ui.separator();
        }
        ui.push_id(index, |ui| channel(ui, state, id, &image, Some(index)));
    }
}

fn mode_selector(ui: &mut egui::Ui, state: &mut AppState, id: DatasetId, mode: ChannelMode) {
    const MODES: [(ChannelMode, &str, &str); 3] = [
        (ChannelMode::Composite, "Composite", "composite"),
        (ChannelMode::Color, "Color", "color"),
        (ChannelMode::Grayscale, "Grayscale", "grayscale"),
    ];
    let current = MODES
        .iter()
        .find(|(m, _, _)| *m == mode)
        .map_or("Color", |(_, name, _)| name);
    let mut chosen = None;
    ui.horizontal(|ui| {
        let label = ui.label("Channels");
        egui::ComboBox::from_id_salt("channel_mode")
            .selected_text(current)
            .width(120.0)
            .show_ui(ui, |ui| {
                for (candidate, name, key) in MODES {
                    if ui.selectable_label(candidate == mode, name).clicked() && candidate != mode {
                        chosen = Some(key);
                    }
                }
            })
            .response
            .labelled_by(label.id)
            .on_hover_text(
                "Composite shows all channels together; color and grayscale show one at a time",
            );
    });
    if let Some(key) = chosen {
        state.start_display("view.set_channel_mode", id, &Args::new().with("mode", key));
    }
}

/// The name a channel goes by in the panels.
pub fn channel_name(image: &Image, channel: usize) -> String {
    let meta = image.meta();
    if meta.color == ColorModel::Rgb {
        return ["Red", "Green", "Blue"].get(channel).map_or_else(
            || format!("Channel {}", channel + 1),
            |name| name.to_string(),
        );
    }
    match meta.channels.get(channel).and_then(|c| c.name.as_ref()) {
        Some(name) if !name.is_empty() => name.clone(),
        _ => format!("Channel {}", channel + 1),
    }
}

/// The lookup table a channel is shown through when shown in colour.
pub fn channel_lut(image: &Image, channel: usize) -> imager_core::Lut {
    let mode = match imager_commands::view::channel_mode(image.meta()) {
        ChannelMode::Composite => ChannelMode::Composite,
        _ => ChannelMode::Color,
    };
    imager_render::channel_lut(image, channel, mode)
}

/// One channel's controls. `channel` is `None` for an RGB image, whose
/// channels are adjusted together.
fn channel(
    ui: &mut egui::Ui,
    state: &mut AppState,
    id: DatasetId,
    image: &Image,
    channel: Option<usize>,
) {
    let meta = image.meta();
    let index = channel.unwrap_or(0);
    let count = meta.size(&AxisKind::Channel);
    let together = view::shown_together(meta);
    let position = state.position(id);
    // What the commands call this channel: counted from 1, 0 for all.
    let number = channel.map_or(0, |c| c as i64 + 1);
    let name = match channel {
        Some(c) => channel_name(image, c),
        None => "RGB".to_string(),
    };

    ui.horizontal(|ui| {
        if let Some(c) = channel {
            ui::lut_strip(ui, &channel_lut(image, c).table, egui::vec2(26.0, 12.0));
            if count > 1 && together {
                // Hiding a channel changes the view, not the image.
                let mut visible = state.channel_visible(id, c);
                let toggle = ui
                    .checkbox(&mut visible, name.as_str())
                    .on_hover_text("Show or hide this channel");
                if toggle.changed() {
                    state.set_channel_visible(id, c, visible);
                }
            } else if count > 1 {
                let selected = view::channel_at(meta, &position) == c;
                let choose = ui
                    .radio(selected, name.as_str())
                    .on_hover_text("Show this channel");
                if choose.clicked() && !selected {
                    state.set_position(id, &view::with_channel(meta, &position, c));
                }
            } else {
                ui.label(name.as_str());
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                lut_choice(ui, state, id, image, c);
            });
        } else {
            ui::section(ui, "Red, green and blue");
        }
    });

    let plane = meta
        .plane_index(&view::with_channel(meta, &position, index))
        .unwrap_or(0);
    let histogram = state.histogram(id, plane, channel.is_none());
    let Some((mut min, mut max)) = state.shown_range(id, index) else {
        // The default range of a plane that is still being read.
        ui::weak(ui, "Reading the plane…");
        return;
    };
    let before = (min, max);
    let integer = meta.dtype().is_integer();
    let snap = |value: f64| if integer { value.round() } else { value };

    // The histogram, with the range drawn over it and draggable on it.
    let color = match channel {
        Some(c) => ui::series_color(ui, &channel_lut(image, c).table),
        None => ui.visuals().text_color(),
    };
    let series: Vec<ui::Series<'_>> = histogram
        .ready()
        .map(|h| ui::Series {
            histogram: &h.histogram,
            color,
        })
        .into_iter()
        .collect();
    // The value axis holds still during a drag, so the handle stays
    // under the pointer.
    let span_id = ui.id().with("span");
    let held: Option<(f64, f64)> = ui.data(|data| data.get_temp(span_id));
    let span = held.unwrap_or((min.min(max), max.max(min)));
    let plot = ui::histogram_plot(
        ui,
        PLOT_HEIGHT,
        &series,
        false,
        Some(span),
        egui::Sense::click_and_drag(),
    );
    let handle_id = ui.id().with("handle");
    let pointer = plot.response.interact_pointer_pos();
    if plot.response.drag_started()
        && let Some(at) = pointer
    {
        let upper = (at.x - plot.x_of(max)).abs() < (at.x - plot.x_of(min)).abs();
        ui.data_mut(|data| {
            data.insert_temp(handle_id, upper);
            data.insert_temp(span_id, plot.span);
        });
    }
    if plot.response.dragged()
        && let Some(at) = pointer
    {
        let value = snap(plot.value_at(at.x));
        if ui.data(|data| data.get_temp(handle_id)).unwrap_or(false) {
            max = value.max(min);
        } else {
            min = value.min(max);
        }
    }
    if !plot.response.dragged() && held.is_some() {
        ui.data_mut(|data| data.remove::<(f64, f64)>(span_id));
    }
    range_marks(ui, &plot, (min, max));
    let what = match channel {
        Some(_) => format!("Histogram of {name} with its display range"),
        None => "Histogram with the display range".to_string(),
    };
    plot.response
        .widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Other, true, &what));
    let plot_dragged = plot.response.dragged();
    match &histogram {
        HistogramState::Failed(problem) => {
            ui::problem(ui, problem);
        }
        HistogramState::Pending => {
            plot.response.on_hover_text("Counting the histogram…");
        }
        HistogramState::Ready(_) => {
            plot.response
                .on_hover_text("Drag the markers to change the display range");
        }
    }

    // The same range as numbers, and the two buttons that choose one.
    let (low, high) = plot.span;
    let speed = if integer {
        ((high - low) / 300.0).max(0.25)
    } else {
        ((high - low) / 300.0).max(f64::MIN_POSITIVE)
    };
    let mut fields_held = false;
    let (mut auto, mut reset) = (false, false);
    ui.horizontal(|ui| {
        let mut field = |ui: &mut egui::Ui, label: &str, value: &mut f64, hint: &str| {
            let text = ui.label(RichText::new(label).weak());
            let mut drag = egui::DragValue::new(value)
                .speed(speed)
                .update_while_editing(false);
            drag = if integer {
                drag.fixed_decimals(0)
            } else {
                drag.max_decimals(4)
            };
            let response = ui
                .add_sized([54.0, ui.spacing().interact_size.y], drag)
                .labelled_by(text.id)
                .on_hover_text(hint);
            fields_held |= response.dragged();
        };
        field(ui, "Min", &mut min, "Sample value shown darkest");
        field(ui, "Max", &mut max, "Sample value shown brightest");
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            reset = ui
                .small_button("Reset")
                .on_hover_text("Show the full range of this plane")
                .clicked();
            auto = ui
                .small_button("Auto")
                .on_hover_text("Pick a range that suits this plane; press again to stretch further")
                .clicked();
        });
    });
    min = snap(min);
    max = snap(max);

    if (min, max) != before {
        state.preview_display_range(id, channel, (min, max));
    }
    let ours = state
        .previewed_range()
        .is_some_and(|(image, which, _)| image == id && which == channel);
    if ours && !plot_dragged && !fields_held {
        state.commit_display_range();
    }
    if auto {
        state.start_display(
            "view.auto_contrast",
            id,
            &Args::new().with("channel", number),
        );
    }
    if reset {
        state.start_display(
            "view.reset_display_range",
            id,
            &Args::new().with("channel", number),
        );
    }
}

/// Draws the display range over a histogram: the part outside it dimmed,
/// and a marker at each end.
fn range_marks(ui: &egui::Ui, plot: &ui::Plot, (min, max): (f64, f64)) {
    let rect = plot.rect;
    let painter = ui.painter_at(rect.expand(1.0));
    let (left, right) = (plot.x_of(min), plot.x_of(max));
    let dim = ui.visuals().extreme_bg_color.gamma_multiply(0.6);
    let outside = |from: f32, to: f32| {
        if to > from {
            painter.rect_filled(
                egui::Rect::from_min_max(
                    egui::pos2(from, rect.top()),
                    egui::pos2(to, rect.bottom()),
                ),
                0.0,
                dim,
            );
        }
    };
    outside(rect.left(), left);
    outside(right, rect.right());
    let ink = ui.visuals().strong_text_color();
    for (x, inward) in [(left, 1.0), (right, -1.0)] {
        painter.line_segment(
            [egui::pos2(x, rect.top()), egui::pos2(x, rect.bottom())],
            egui::Stroke::new(1.5, ink),
        );
        // A flag at the top says which way the range lies, so the two
        // ends are told apart by shape as well as position.
        let flag = vec![
            egui::pos2(x, rect.top()),
            egui::pos2(x + 6.0 * inward, rect.top()),
            egui::pos2(x, rect.top() + 7.0),
        ];
        painter.add(egui::Shape::convex_polygon(
            flag,
            ink,
            egui::Stroke::new(0.0, Color32::TRANSPARENT),
        ));
    }
}

/// The drop-down that chooses a channel's lookup table.
fn lut_choice(
    ui: &mut egui::Ui,
    state: &mut AppState,
    id: DatasetId,
    image: &Image,
    channel: usize,
) {
    let current = channel_lut(image, channel);
    let number = channel as i64 + 1;
    let mut chosen = None;
    let mut invert = false;
    let name = channel_name(image, channel);
    let combo = egui::ComboBox::from_id_salt("lut")
        .selected_text(current.name.as_str())
        .width(112.0)
        .show_ui(ui, |ui| {
            for lut in imager_render::builtin_luts() {
                // A table may share a name with a built-in one without
                // being it (a file's own "Custom" never does).
                let selected = lut.name == current.name && lut.lut().table == current.table;
                if ui.selectable_label(selected, lut.name).clicked() && !selected {
                    chosen = Some(lut.id);
                }
            }
            ui.separator();
            invert = ui
                .button("Invert")
                .on_hover_text("Reverse this lookup table")
                .clicked();
        });
    let info = format!("Lookup table of {name}");
    combo
        .response
        .on_hover_text("Lookup table")
        .widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::ComboBox, true, &info));
    if let Some(lut) = chosen {
        state.start_display(
            "view.set_lut",
            id,
            &Args::new().with("lut", lut).with("channel", number),
        );
    }
    if invert {
        state.start_display("view.invert_lut", id, &Args::new().with("channel", number));
    }
}
