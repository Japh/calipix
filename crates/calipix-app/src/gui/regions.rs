//! The Regions panel: the selections saved with the active image.
//!
//! This one list stands where ImageJ has two, the ROI Manager and the
//! overlay. A region is added from the selection, drawn over the image,
//! saved with it, and measured from here. Every button runs a command, so
//! what is done here is in History and can be undone.
//!
//! Only the rows in view are laid out, so a list of any length scrolls
//! the same.

use calipix_command::Args;
use eframe::egui::{self, Align2, Rect, Sense, pos2, vec2};

use super::{overlay, ui};
use crate::state::canvas::kind_title;
use crate::state::regions::region_title;
use crate::state::{AppState, RegionLabels, RowClick};

pub fn show(ui: &mut egui::Ui, state: &mut AppState) {
    ui::panel_frame().show(ui, |ui| {
        let panel = ui.max_rect();
        if state.active().is_none() {
            state.set_regions_attention(false);
            return ui::empty(ui, "Open an image to keep regions with it");
        }
        toolbar(ui, state);
        ui.add_space(3.0);
        if state.regions().is_empty() {
            ui::empty(
                ui,
                "Regions of the image are listed here: draw a selection and press Add",
            );
        } else {
            rows(ui, state);
        }
        state.note_regions_drawn();
        // A press anywhere else gives the keyboard back to the image.
        let pressed_outside = ui.input(|input| {
            input.pointer.any_pressed()
                && input
                    .pointer
                    .interact_pos()
                    .is_some_and(|at| !panel.contains(at))
        });
        if pressed_outside {
            state.set_regions_attention(false);
        }
    });
}

/// A button that chooses a command with `args`, disabled with the reason
/// while the command cannot run.
fn command_button(
    ui: &mut egui::Ui,
    state: &mut AppState,
    label: &str,
    hint: &str,
    command: &str,
    args: Args,
) {
    let unavailable = state.unavailable_with(command, &args);
    let button = ui
        .add_enabled(unavailable.is_none(), egui::Button::new(label))
        .on_hover_text(hint);
    if let Some(reason) = unavailable {
        button.on_disabled_hover_text(reason);
    } else if button.clicked() {
        state.request(command, args);
    }
}

fn toolbar(ui: &mut egui::Ui, state: &mut AppState) {
    let Some(id) = state.active().map(|dataset| dataset.id) else {
        return;
    };
    let input = || Args::new().with("input", id);
    let picked: Vec<usize> = state.selected_regions(id).iter().copied().collect();
    let count = state.regions().len();
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 5.0;
        let add_hint = match state.shortcut_text("regions.add") {
            Some(shortcut) => format!("Keep the selection as a region ({shortcut})"),
            None => "Keep the selection as a region".to_string(),
        };
        command_button(ui, state, "Add", &add_hint, "regions.add", input());

        let delete = ui
            .add_enabled(!picked.is_empty(), egui::Button::new("Delete"))
            .on_hover_text("Remove the selected regions (Delete)")
            .on_disabled_hover_text("Click a region to select it first");
        if delete.clicked() {
            state.delete_selected_regions();
        }

        let rename = ui
            .add_enabled(picked.len() == 1, egui::Button::new("Rename"))
            .on_hover_text("Give the selected region a name (F2, or double-click it)")
            .on_disabled_hover_text("Select one region first");
        if rename.clicked() {
            state.begin_rename(picked[0]);
        }

        let what = if picked.is_empty() {
            "Measure every region into the Results table"
        } else {
            "Measure the selected regions into the Results table"
        };
        command_button(
            ui,
            state,
            "Measure",
            what,
            "regions.measure",
            state.region_args(),
        );

        ui.menu_button("More", |ui| {
            ui.set_min_width(170.0);
            let update = match picked.as_slice() {
                [one] => input().with("region", *one + 1),
                _ => input(),
            };
            let item = |ui: &mut egui::Ui,
                        state: &mut AppState,
                        label: &str,
                        command: &str,
                        args: Args,
                        enabled: bool| {
                let unavailable = state
                    .unavailable_with(command, &args)
                    .or((!enabled).then(|| "Select one region first".to_string()));
                let button = ui.add_enabled(unavailable.is_none(), egui::Button::new(label));
                if let Some(reason) = unavailable {
                    button.on_disabled_hover_text(reason);
                } else if button.clicked() {
                    state.request(command, args);
                }
            };
            item(
                ui,
                state,
                "Update from Selection",
                "regions.update",
                update,
                picked.len() == 1,
            );
            let chosen = state.region_args();
            item(ui, state, "Set Color…", "regions.set_color", chosen, true);
            ui.separator();
            item(ui, state, "Open…", "regions.open", input(), true);
            let chosen = state.region_args();
            item(ui, state, "Save…", "regions.save", chosen, true);
            ui.separator();
            item(ui, state, "Clear", "regions.clear", input(), true);
        })
        .response
        .on_hover_text("Update, colour, open, save and clear regions");
    });
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 5.0;
        let mut shown = state.regions_shown();
        if ui
            .checkbox(&mut shown, "Show")
            .on_hover_text("Draw the regions on the image")
            .changed()
        {
            state.request("view.toggle_regions", Args::new());
        }
        let labels = state.region_labels();
        let name = |labels: RegionLabels| match labels {
            RegionLabels::None => "No labels",
            RegionLabels::Numbers => "Numbers",
            RegionLabels::Names => "Names",
        };
        egui::ComboBox::from_id_salt("region_labels")
            .selected_text(name(labels))
            .width(92.0)
            .show_ui(ui, |ui| {
                for choice in RegionLabels::ALL {
                    if ui
                        .selectable_label(choice == labels, name(choice))
                        .clicked()
                        && choice != labels
                    {
                        let command = format!("view.region_labels_{}", choice.key());
                        state.request(&command, Args::new());
                    }
                }
            })
            .response
            .on_hover_text("How regions are labelled on the image");
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let total = match count {
                1 => "1 region".to_string(),
                count => format!("{count} regions"),
            };
            ui::weak(
                ui,
                if picked.is_empty() {
                    total
                } else {
                    format!("{total}, {} selected", picked.len())
                },
            );
        });
    });
}

fn rows(ui: &mut egui::Ui, state: &mut AppState) {
    let Some(id) = state.active().map(|dataset| dataset.id) else {
        return;
    };
    let count = state.regions().len();
    let row_height = (ui.text_style_height(&egui::TextStyle::Body) + 6.0).round();
    let font = egui::TextStyle::Body.resolve(ui.style());
    let small = egui::TextStyle::Small.resolve(ui.style());
    let visuals = ui.visuals().clone();
    let modifiers = ui.input(|input| input.modifiers);
    // The number column is as wide as the largest number.
    let digits = count.to_string().len() as f32;
    let number_width = 14.0 + 7.5 * digits;
    let mut clicked = None;
    let mut rename = None;
    let mut end_rename = None;

    ui.spacing_mut().item_spacing.y = 0.0;
    egui::ScrollArea::vertical()
        .id_salt("region_rows")
        .auto_shrink(false)
        .show_rows(ui, row_height, count, |ui, range| {
            for index in range {
                let entry = &state.regions()[index];
                let title = region_title(entry, index);
                let named = entry
                    .props
                    .name
                    .as_ref()
                    .is_some_and(|n| !n.trim().is_empty());
                let kind = kind_title(&entry.roi);
                let color = overlay::region_color(entry);
                let selected = state.selected_regions(id).contains(&index);
                let (rect, response) =
                    ui.allocate_exact_size(vec2(ui.available_width(), row_height), Sense::click());
                response.widget_info(|| {
                    egui::WidgetInfo::selected(
                        egui::WidgetType::SelectableLabel,
                        true,
                        selected,
                        title.as_str(),
                    )
                });
                if response.double_clicked() {
                    rename = Some(index);
                } else if response.clicked() {
                    clicked = Some((
                        index,
                        RowClick {
                            extend: modifiers.shift,
                            toggle: modifiers.command,
                        },
                    ));
                }
                if !ui.is_rect_visible(rect) {
                    continue;
                }
                let painter = ui.painter_at(rect);
                let fill = if selected {
                    Some(visuals.selection.bg_fill)
                } else if response.hovered() {
                    Some(visuals.widgets.hovered.weak_bg_fill)
                } else if index % 2 == 1 {
                    Some(visuals.faint_bg_color)
                } else {
                    None
                };
                if let Some(fill) = fill {
                    painter.rect_filled(rect, 2.0, fill);
                }
                let text_color = if selected {
                    visuals.strong_text_color()
                } else {
                    visuals.text_color()
                };
                let middle = rect.center().y;
                painter.text(
                    pos2(rect.left() + number_width - 6.0, middle),
                    Align2::RIGHT_CENTER,
                    (index + 1).to_string(),
                    small.clone(),
                    visuals.weak_text_color(),
                );
                // The colour the region is drawn in, edged so that it
                // shows on either theme.
                let swatch = Rect::from_center_size(
                    pos2(rect.left() + number_width + 6.0, middle),
                    vec2(11.0, 11.0),
                );
                painter.rect_filled(swatch, 2.0, color);
                painter.rect_stroke(
                    swatch,
                    2.0,
                    visuals.widgets.noninteractive.fg_stroke,
                    egui::StrokeKind::Inside,
                );
                let kind_galley = painter.layout_no_wrap(
                    kind.to_string(),
                    small.clone(),
                    visuals.weak_text_color(),
                );
                let kind_left = rect.right() - kind_galley.size().x - 8.0;
                let name_left = swatch.right() + 8.0;
                let name_rect = Rect::from_min_max(
                    pos2(name_left, rect.top()),
                    pos2((kind_left - 6.0).max(name_left + 30.0), rect.bottom()),
                );

                let renaming = state
                    .renaming()
                    .is_some_and(|renaming| renaming.index == index);
                if renaming && let Some(renaming) = state.renaming() {
                    let field = egui::TextEdit::singleline(&mut renaming.text)
                        .hint_text(format!("Region {}", index + 1))
                        .margin(vec2(4.0, 1.0));
                    let edit = ui.put(name_rect.shrink2(vec2(0.0, 1.0)), field);
                    if std::mem::take(&mut renaming.fresh) {
                        edit.request_focus();
                    } else if edit.lost_focus() {
                        // Enter and a click elsewhere keep the name;
                        // Escape gives it up.
                        let escaped = ui.input(|input| input.key_pressed(egui::Key::Escape));
                        end_rename = Some(!escaped);
                    }
                    continue;
                }
                painter.galley(
                    pos2(kind_left, middle - kind_galley.size().y / 2.0),
                    kind_galley,
                    visuals.weak_text_color(),
                );
                // A region without a name is listed by its number, more
                // quietly, unless the row is picked out.
                let name_color = if named || selected {
                    text_color
                } else {
                    visuals.weak_text_color()
                };
                painter.with_clip_rect(name_rect).text(
                    pos2(name_left, middle),
                    Align2::LEFT_CENTER,
                    &title,
                    font.clone(),
                    name_color,
                );
            }
        });

    if let Some(keep) = end_rename {
        state.end_rename(keep);
    }
    if let Some(index) = rename {
        state.set_regions_attention(true);
        state.begin_rename(index);
    } else if let Some((index, click)) = clicked {
        state.click_region(index, click);
    }
}

/// The keys that act on the list while it has the keyboard's attention.
/// Call before shortcuts are looked up: Ctrl+A here picks regions, not
/// the whole image.
pub fn keys(ctx: &egui::Context, state: &mut AppState) {
    if !state.regions_have_attention() || ctx.text_edit_focused() {
        return;
    }
    use egui::{Key, Modifiers};
    let (delete, none, all, escape, rename) = ctx.input_mut(|input| {
        let delete = input.consume_key(Modifiers::NONE, Key::Delete)
            || input.consume_key(Modifiers::NONE, Key::Backspace);
        // The longer shortcut first, or Ctrl+A would take it.
        let none = input.consume_key(Modifiers::COMMAND | Modifiers::SHIFT, Key::A);
        let all = input.consume_key(Modifiers::COMMAND, Key::A);
        let escape = input.consume_key(Modifiers::NONE, Key::Escape);
        let rename = input.consume_key(Modifiers::NONE, Key::F2);
        (delete, none, all, escape, rename)
    });
    let Some(id) = state.active().map(|dataset| dataset.id) else {
        return;
    };
    if delete {
        state.delete_selected_regions();
    }
    if all {
        state.select_all_regions();
    }
    if none || escape {
        state.clear_region_selection();
    }
    if rename && let Some(&index) = state.selected_regions(id).first() {
        state.begin_rename(index);
    }
}
