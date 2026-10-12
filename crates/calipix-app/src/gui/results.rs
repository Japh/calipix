//! The Results panel: the session's results tables, one at a time, as the
//! text ImageJ would show and save.
//!
//! Only the rows in view are laid out, so a table of any length scrolls
//! the same. The headings and the row numbers stay put while the rest
//! scrolls under them.

use calipix_command::Args;
use eframe::egui::{self, Align, Color32, Layout, Rect, RichText, Sense, UiBuilder, pos2, vec2};

use super::ui;
use crate::state::{AppState, RowClick, TableText};

/// Space either side of a cell's text.
const PAD: f32 = 9.0;

pub fn show(ui: &mut egui::Ui, state: &mut AppState) {
    ui::panel_frame().show(ui, |ui| {
        let panel = ui.max_rect();
        let names = state.table_names();
        let (Some(shown), Some(text)) = (state.shown_table(), state.table_text()) else {
            state.set_table_attention(false);
            return ui::empty(
                ui,
                "Measurements are listed here: Analyze > Measure adds a row",
            );
        };
        toolbar(ui, state, &names, &shown, &text);
        ui.add_space(3.0);
        if text
            .headings
            .iter()
            .all(|heading| heading.trim().is_empty())
            && text.is_empty()
        {
            ui::empty(ui, "This table is empty");
        } else {
            rows(ui, state, &text);
        }
        // A press anywhere else gives the keyboard back to the image.
        let pressed_outside = ui.input(|input| {
            input.pointer.any_pressed()
                && input
                    .pointer
                    .interact_pos()
                    .is_some_and(|at| !panel.contains(at))
        });
        if pressed_outside {
            state.set_table_attention(false);
        }
    });
}

/// A button that chooses a command for the table on show.
fn command_button(
    ui: &mut egui::Ui,
    state: &mut AppState,
    label: &str,
    hint: &str,
    command: &str,
    table: &str,
) {
    let unavailable = state.unavailable_reason(command);
    let button = ui
        .add_enabled(unavailable.is_none(), egui::Button::new(label))
        .on_hover_text(hint);
    if let Some(reason) = unavailable {
        button.on_disabled_hover_text(reason);
    } else if button.clicked() {
        state.request(command, Args::new().with("table", table));
    }
}

/// A drop-down that chooses between `names`, or just the name if there is
/// nothing to choose between. Returns the name chosen, if one was.
pub fn switcher(ui: &mut egui::Ui, id: &str, names: &[String], shown: &str) -> Option<String> {
    if names.len() < 2 {
        ui.label(RichText::new(shown).strong());
        return None;
    }
    let mut chosen = None;
    // Wide enough for the name on show, without growing past the panel.
    let width = (shown.chars().count() as f32 * 7.5 + 36.0).clamp(120.0, 320.0);
    egui::ComboBox::from_id_salt(id)
        .selected_text(RichText::new(shown).strong())
        .width(width)
        .show_ui(ui, |ui| {
            for name in names {
                if ui.selectable_label(name == shown, name).clicked() && name != shown {
                    chosen = Some(name.clone());
                }
            }
        })
        .response
        .on_hover_text(format!("{} tables. Choose the one to show", names.len()));
    chosen
}

fn toolbar(
    ui: &mut egui::Ui,
    state: &mut AppState,
    names: &[String],
    shown: &str,
    text: &TableText,
) {
    ui.horizontal(|ui| {
        if let Some(name) = switcher(ui, "results_table_choice", names, shown) {
            state.show_table(&name);
        }
        let picked = state.selected_rows().len();
        let count = match text.len() {
            1 => "1 row".to_string(),
            rows => format!("{rows} rows"),
        };
        ui::weak(
            ui,
            if picked > 0 {
                format!("{count}, {picked} selected")
            } else {
                count
            },
        );
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            // Right to left: the last one here is the leftmost.
            command_button(
                ui,
                state,
                "Clear",
                "Remove this table. Undo brings it back",
                "measure.clear_results",
                shown,
            );
            let delete = ui
                .add_enabled(picked > 0, egui::Button::new("Delete Rows"))
                .on_hover_text("Remove the selected rows (Delete)")
                .on_disabled_hover_text("Click a row to select it first");
            if delete.clicked() {
                state.delete_selected_rows();
            }
            command_button(
                ui,
                state,
                "Summarize",
                "Add the mean, standard deviation, minimum and maximum of each column",
                "measure.summarize",
                shown,
            );
            ui.separator();
            command_button(
                ui,
                state,
                "Save As…",
                "Save this table as comma-separated (.csv) or tab-separated text",
                "measure.save_results",
                shown,
            );
            let what = if picked > 0 {
                "Copy the selected rows"
            } else {
                "Copy the whole table"
            };
            if ui.button("Copy").on_hover_text(what).clicked() {
                copy(ui.ctx(), state);
            }
        });
    });
}

/// Puts the selected rows, or the whole table, on the clipboard.
pub fn copy(ctx: &egui::Context, state: &mut AppState) {
    if let Some(text) = state.copy_table() {
        ctx.copy_text(text);
    }
}

/// The keys that act on the table while it has the keyboard's attention.
/// Call before shortcuts are looked up: Ctrl+A here selects rows, not the
/// image.
pub fn keys(ctx: &egui::Context, state: &mut AppState) {
    if !state.table_has_attention() || ctx.text_edit_focused() {
        return;
    }
    use egui::{Key, Modifiers};
    let (delete, all, copied, escape) = ctx.input_mut(|input| {
        let delete = input.consume_key(Modifiers::NONE, Key::Delete)
            || input.consume_key(Modifiers::NONE, Key::Backspace);
        // Select None, while the table has the keyboard, is about its
        // rows; taken before Ctrl+A, which would match it too.
        let none = input.consume_key(Modifiers::COMMAND | Modifiers::SHIFT, Key::A);
        let all = input.consume_key(Modifiers::COMMAND, Key::A);
        // The window system turns Ctrl+C into a copy event.
        let copy_event = input.events.iter().any(|e| matches!(e, egui::Event::Copy));
        let copied = input.consume_key(Modifiers::COMMAND, Key::C) || copy_event;
        let escape = input.consume_key(Modifiers::NONE, Key::Escape) || none;
        (delete, all, copied, escape)
    });
    if delete {
        state.delete_selected_rows();
    }
    if all {
        state.select_all_rows();
    }
    if copied {
        copy(ctx, state);
    }
    if escape {
        state.clear_row_selection();
    }
}

/// One cell: text in a rectangle, numbers against its right edge.
fn cell(ui: &mut egui::Ui, id: (usize, usize), rect: Rect, text: RichText, right: bool) {
    let layout = if right {
        Layout::right_to_left(Align::Center)
    } else {
        Layout::left_to_right(Align::Center)
    };
    let builder = UiBuilder::new()
        .id_salt(id)
        .max_rect(rect.shrink2(vec2(PAD, 0.0)))
        .layout(layout);
    let mut cell = ui.new_child(builder);
    cell.add(egui::Label::new(text).selectable(false).truncate());
}

fn rows(ui: &mut egui::Ui, state: &mut AppState, text: &TableText) {
    let font = egui::TextStyle::Body.resolve(ui.style());
    let row_height = (ui.text_style_height(&egui::TextStyle::Body) + 5.0).round();
    let measure = |text: &str| {
        ui.painter()
            .layout_no_wrap(text.to_string(), font.clone(), Color32::PLACEHOLDER)
            .size()
            .x
    };
    let widths: Vec<f32> = text
        .headings
        .iter()
        .zip(&text.widest)
        .map(|(heading, widest)| (measure(heading).max(measure(widest)) + 2.0 * PAD + 3.0).ceil())
        .collect();
    let total: f32 = widths.iter().sum();
    let count = text.len();
    let visuals = ui.visuals().clone();
    let panel_fill = visuals.panel_fill;
    let line = visuals.widgets.noninteractive.bg_stroke;
    let mut clicked = None;

    egui::ScrollArea::both()
        .id_salt("results_rows")
        .auto_shrink(false)
        .show_viewport(ui, |ui, viewport| {
            let size = vec2(total.max(viewport.width()), row_height * (count + 1) as f32);
            let (area, _) = ui.allocate_exact_size(size, Sense::hover());
            let origin = area.min;
            // Where the pinned headings and row numbers sit this frame.
            let (pin_x, pin_y) = (origin.x + viewport.left(), origin.y + viewport.top());
            let first = (viewport.top() / row_height).floor().max(0.0) as usize;
            let last = ((viewport.bottom() / row_height).ceil() as usize)
                .saturating_sub(1)
                .min(count);
            // The left edge of each column that scrolls.
            let mut lefts = Vec::with_capacity(widths.len());
            let mut x = origin.x;
            for width in &widths {
                lefts.push(x);
                x += width;
            }
            let in_view = |column: usize| {
                lefts[column] + widths[column] > pin_x + widths[0]
                    && lefts[column] < origin.x + viewport.right()
            };
            let painter = ui.painter().clone();
            let modifiers = ui.input(|input| input.modifiers);

            for row in first..last {
                let top = origin.y + row_height * (row + 1) as f32;
                let band =
                    Rect::from_min_size(pos2(pin_x, top), vec2(viewport.width(), row_height));
                // The part of the row not under the headings takes clicks.
                let live = band.intersect(Rect::everything_below(pin_y + row_height));
                let response = ui.interact(live, ui.id().with(("row", row)), Sense::click());
                let selected = state.selected_rows().contains(&row);
                response.widget_info(|| {
                    egui::WidgetInfo::selected(
                        egui::WidgetType::SelectableLabel,
                        true,
                        selected,
                        format!("Row {}", row + 1),
                    )
                });
                if response.clicked() {
                    clicked = Some((
                        row,
                        RowClick {
                            extend: modifiers.shift,
                            toggle: modifiers.command,
                        },
                    ));
                }
                let fill = if selected {
                    Some(visuals.selection.bg_fill)
                } else if response.hovered() {
                    Some(visuals.widgets.hovered.weak_bg_fill)
                } else if row % 2 == 1 {
                    Some(visuals.faint_bg_color)
                } else {
                    None
                };
                if let Some(fill) = fill {
                    painter.rect_filled(band, 0.0, fill);
                }
                let color = if selected {
                    visuals.strong_text_color()
                } else {
                    visuals.text_color()
                };
                for (column, value) in text.row(row).enumerate().skip(1) {
                    if column >= widths.len() || !in_view(column) {
                        continue;
                    }
                    let rect = Rect::from_min_size(
                        pos2(lefts[column], top),
                        vec2(widths[column], row_height),
                    );
                    let value = RichText::new(value).color(color);
                    cell(ui, (row + 1, column), rect, value, text.numeric[column]);
                }
            }

            // The row numbers, over whatever has scrolled under them.
            let numbers = Rect::from_min_size(
                pos2(pin_x, origin.y),
                vec2(widths[0], size.y.max(viewport.height())),
            );
            painter.rect_filled(numbers, 0.0, panel_fill);
            for row in first..last {
                let top = origin.y + row_height * (row + 1) as f32;
                let rect = Rect::from_min_size(pos2(pin_x, top), vec2(widths[0], row_height));
                if state.selected_rows().contains(&row) {
                    painter.rect_filled(rect, 0.0, visuals.selection.bg_fill);
                }
                let number = RichText::new((row + 1).to_string()).weak();
                cell(ui, (row + 1, 0), rect, number, true);
            }
            painter.vline(numbers.right(), numbers.y_range(), line);

            // The headings, over everything.
            let band = Rect::from_min_size(pos2(pin_x, pin_y), vec2(viewport.width(), row_height));
            painter.rect_filled(band, 0.0, panel_fill);
            for (column, heading) in text.headings.iter().enumerate().skip(1) {
                if !in_view(column) {
                    continue;
                }
                let rect = Rect::from_min_size(
                    pos2(lefts[column], pin_y),
                    vec2(widths[column], row_height),
                );
                let heading = RichText::new(heading).strong();
                cell(ui, (0, column), rect, heading, text.numeric[column]);
            }
            let corner = Rect::from_min_size(pos2(pin_x, pin_y), vec2(widths[0], row_height));
            painter.rect_filled(corner, 0.0, panel_fill);
            painter.hline(band.x_range(), band.bottom(), line);
        });

    state.note_table_drawn();
    if let Some((row, click)) = clicked {
        state.click_row(row, click);
    }
}
