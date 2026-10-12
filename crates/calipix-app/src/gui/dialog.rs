//! Parameter dialogs. One function draws the dialog of any command from
//! its [`Dialog`] model; no command has dialog code of its own.

use std::path::Path;

use calipix_command::{Args, DatasetId, ParamType, PathMode, Workspace};
use eframe::egui::{self, Key};

use super::ui;
use crate::state::{AppState, Dialog, Draft, Field, PathRequest};

/// What the person did with the dialog this frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Still filling it in.
    Open,
    /// Pressed OK with valid values.
    Accepted,
    Cancelled,
}

const WIDTH: f32 = 430.0;

/// Notes that the person wants to browse for the path of the named
/// parameter, starting from the one in its field.
type Browse<'a> = dyn FnMut(PathMode, &str, &str) + 'a;

/// Draws the dialog. If a Browse button was pressed, `browse_for` is set
/// to the parameter it belongs to and what the file dialog should ask.
pub fn show(
    ctx: &egui::Context,
    dialog: &mut Dialog,
    state: &AppState,
    browse_for: &mut Option<(String, PathRequest)>,
) -> Outcome {
    let workspace = &state.session().workspace;
    let mut outcome = Outcome::Open;
    let frame = ui::modal_frame(&ctx.global_style());
    let modal = egui::Modal::new(egui::Id::new("command_dialog"))
        .frame(frame)
        .show(ctx, |ui| {
            ui.set_width(WIDTH);
            ui.heading(&dialog.spec.title);
            if !dialog.spec.summary.is_empty() {
                ui::weak(ui, dialog.spec.summary.clone());
            }
            ui.add_space(ui::GAP);

            let spec = dialog.spec.clone();
            let mut browse = |mode: PathMode, current: &str, param: &str| {
                let mut request = state.path_request(&spec, mode, &Args::new());
                if let Some(folder) = Path::new(current).parent().filter(|d| d.is_dir()) {
                    request.directory = Some(folder.to_path_buf());
                }
                *browse_for = Some((param.to_string(), request));
            };
            // A long dialog scrolls rather than running off the window.
            let room = (ui.ctx().content_rect().height() - 230.0).max(160.0);
            let mut typing_lines = false;
            egui::ScrollArea::vertical()
                .max_height(room)
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    typing_lines |= rows(ui, workspace, &mut dialog.fields, false, &mut browse);
                    if dialog.has_advanced() {
                        ui.add_space(ui::GAP / 2.0);
                        let section = egui::CollapsingHeader::new("More options")
                            .id_salt("advanced")
                            .open(Some(dialog.show_advanced))
                            .show(ui, |ui| {
                                let fields = &mut dialog.fields;
                                typing_lines |= rows(ui, workspace, fields, true, &mut browse);
                            });
                        if section.header_response.clicked() {
                            dialog.show_advanced = !dialog.show_advanced;
                        }
                    }
                });

            ui.add_space(ui::GAP);
            ui.separator();
            ui.add_space(ui::GAP / 2.0);
            let valid = dialog.is_valid(workspace);
            ui.horizontal(|ui| {
                if ui::button(ui, "Reset")
                    .on_hover_text("Restore the default values")
                    .clicked()
                {
                    dialog.reset(workspace);
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let ok = ui::primary_button(ui, "OK", valid)
                        .on_disabled_hover_text("Fix the marked values first");
                    if ok.clicked() {
                        outcome = Outcome::Accepted;
                    }
                    if ui::button(ui, "Cancel").clicked() {
                        outcome = Outcome::Cancelled;
                    }
                });
            });
            // Read after the fields have been drawn, so a number being typed
            // is committed by the same Enter that accepts the dialog.
            // In a field of several lines, Enter starts a new line.
            if valid && !typing_lines && ui.input(|i| i.key_pressed(Key::Enter)) {
                outcome = Outcome::Accepted;
            }
        });
    if modal.should_close() && outcome == Outcome::Open {
        outcome = Outcome::Cancelled;
    }
    outcome
}

/// How the fields of one section are laid out.
enum Block {
    /// One row per parameter: label, then the control with its unit.
    Rows(Vec<usize>),
    /// A long run of checkboxes, two to a row, as ImageJ sets out the
    /// choices of Set Measurements.
    Checks(Vec<usize>),
    /// Text of several lines: its label, then a field as wide as the
    /// dialog under it.
    Wide(usize),
}

/// From how many checkboxes in a row they are set out in two columns.
const CHECK_COLUMNS_FROM: usize = 6;

/// Splits the basic or the advanced fields into blocks, in order.
fn blocks(fields: &[Field], advanced: bool) -> Vec<Block> {
    let mut blocks: Vec<Block> = Vec::new();
    let mut run: Vec<usize> = Vec::new();
    let close = |run: &mut Vec<usize>, blocks: &mut Vec<Block>| {
        if run.len() >= CHECK_COLUMNS_FROM {
            blocks.push(Block::Checks(std::mem::take(run)));
        } else if run.is_empty() {
        } else if let Some(Block::Rows(rows)) = blocks.last_mut() {
            rows.append(run);
        } else {
            blocks.push(Block::Rows(std::mem::take(run)));
        }
    };
    for (index, field) in fields.iter().enumerate() {
        if field.param.advanced != advanced {
            continue;
        }
        if matches!(field.draft, Draft::Bool(_)) {
            run.push(index);
            continue;
        }
        close(&mut run, &mut blocks);
        match blocks.last_mut() {
            _ if field.param.multiline => blocks.push(Block::Wide(index)),
            Some(Block::Rows(rows)) => rows.push(index),
            _ => blocks.push(Block::Rows(vec![index])),
        }
    }
    close(&mut run, &mut blocks);
    blocks
}

/// The basic or the advanced parameters. Whatever is wrong with a value
/// goes on a row of its own directly under the control. Returns true if a
/// field that takes several lines of text has the keyboard.
fn rows(
    ui: &mut egui::Ui,
    workspace: &Workspace,
    fields: &mut [Field],
    advanced: bool,
    browse: &mut Browse<'_>,
) -> bool {
    let mut typing_lines = false;
    for (number, block) in blocks(fields, advanced).into_iter().enumerate() {
        match block {
            Block::Checks(indices) => {
                egui::Grid::new(("checks", advanced, number))
                    .num_columns(2)
                    .min_col_width((WIDTH - ui::GAP * 2.0) / 2.0)
                    .spacing([ui::GAP * 2.0, 5.0])
                    .show(ui, |ui| {
                        for (place, index) in indices.into_iter().enumerate() {
                            let field = &mut fields[index];
                            let doc = field.param.doc.clone();
                            let response = control(ui, field, workspace, browse);
                            if !doc.is_empty() {
                                response.on_hover_text(doc);
                            }
                            if place % 2 == 1 {
                                ui.end_row();
                            }
                        }
                    });
                ui.add_space(4.0);
            }
            Block::Wide(index) => {
                let field = &mut fields[index];
                let doc = field.param.doc.clone();
                let label = ui.label(field.param.label.as_str());
                let control = control(ui, field, workspace, browse).labelled_by(label.id);
                typing_lines |= control.has_focus();
                if !doc.is_empty() {
                    label.on_hover_text(&doc);
                }
                if let Some(problem) = field.problem(workspace) {
                    ui::problem(ui, &problem);
                }
                ui.add_space(4.0);
            }
            Block::Rows(indices) => {
                egui::Grid::new(("fields", advanced, number))
                    .num_columns(2)
                    .spacing([ui::GAP * 1.5, 7.0])
                    .show(ui, |ui| {
                        for index in indices {
                            let field = &mut fields[index];
                            row(ui, field, workspace, browse);
                        }
                    });
            }
        }
    }
    typing_lines
}

/// One parameter: its label, its control and unit, and under them what is
/// wrong with the value, if anything.
fn row(ui: &mut egui::Ui, field: &mut Field, workspace: &Workspace, browse: &mut Browse<'_>) {
    let param = field.param.clone();
    // A checkbox carries its own label.
    let is_bool = matches!(field.draft, Draft::Bool(_));
    let text = if is_bool { "" } else { param.label.as_str() };
    // A label is never broken over lines; the column makes room.
    let label = ui.add(egui::Label::new(text).extend());
    let control = ui
        .horizontal(|ui| {
            let control = control(ui, field, workspace, browse);
            if let Some(unit) = &param.unit {
                ui::weak(ui, unit.clone());
            }
            control
        })
        .inner
        .labelled_by(label.id);
    if !param.doc.is_empty() {
        label.on_hover_text(&param.doc);
        control.on_hover_text(&param.doc);
    }
    ui.end_row();

    if let Some(problem) = field.problem(workspace) {
        ui.label("");
        ui::problem(ui, &problem);
        ui.end_row();
    }
}

fn control(
    ui: &mut egui::Ui,
    field: &mut Field,
    workspace: &Workspace,
    browse: &mut Browse<'_>,
) -> egui::Response {
    let param = &field.param;
    let range = |integer| ui::NumberRange {
        min: param.min,
        max: param.max,
        step: param.step,
        integer,
    };
    match (&mut field.draft, &param.ty) {
        (Draft::Bool(on), _) => ui.checkbox(on, param.label.as_str()),
        (Draft::Int(value), _) => {
            let mut number = *value as f64;
            let response = ui::number(ui, &mut number, range(true));
            *value = number.round() as i64;
            response
        }
        (Draft::Float(value), _) => ui::number(ui, value, range(false)),
        (Draft::Image(chosen), _) => image_choice(ui, &param.name, chosen, workspace),
        (Draft::Text(text), ParamType::Choice(choices)) => {
            // What is shown is each choice's label; what is kept is the
            // choice itself.
            egui::ComboBox::from_id_salt(&param.name)
                .selected_text(param.choice_label(text))
                .width(ui::FIELD_WIDTH)
                .show_ui(ui, |ui| {
                    for choice in choices {
                        ui.selectable_value(text, choice.clone(), param.choice_label(choice));
                    }
                })
                .response
        }
        (Draft::Text(text), ParamType::Path(mode)) => {
            // The field and its button together are as wide as other fields.
            let button = 76.0;
            let width = ui::FIELD_WIDTH - button - ui.spacing().item_spacing.x;
            let edit = ui.add(egui::TextEdit::singleline(text).desired_width(width));
            let browse_button = egui::Button::new("Browse…").min_size(egui::vec2(button, 0.0));
            if ui.add(browse_button).clicked() {
                browse(*mode, text, &param.name);
            }
            edit
        }
        // A number that may be left out: an empty field means it is.
        (Draft::Text(text), ParamType::Int | ParamType::Float) => ui.add(
            egui::TextEdit::singleline(text)
                .desired_width(ui::NUMBER_WIDTH)
                .hint_text("Not set"),
        ),
        (Draft::Text(text), ParamType::Str) if param.multiline => {
            let lines = text.lines().count().clamp(3, 9);
            ui.add(
                egui::TextEdit::multiline(text)
                    .font(egui::TextStyle::Monospace)
                    .desired_width(f32::INFINITY)
                    .desired_rows(lines),
            )
        }
        (Draft::Text(text), ty) => {
            let mut edit = egui::TextEdit::singleline(text).desired_width(ui::FIELD_WIDTH);
            if matches!(ty, ParamType::List(_)) {
                edit = edit.hint_text("Values separated by commas");
            }
            ui.add(edit)
        }
    }
}

/// A drop-down of the open images. Images with the same name are told
/// apart by their number.
fn image_choice(
    ui: &mut egui::Ui,
    salt: &str,
    chosen: &mut Option<DatasetId>,
    workspace: &Workspace,
) -> egui::Response {
    let name = |id: DatasetId| {
        let Ok(dataset) = workspace.get(id) else {
            return "Closed image".to_string();
        };
        let name = &dataset.image.meta().name;
        let twins = workspace
            .datasets()
            .filter(|other| &other.image.meta().name == name)
            .count();
        if twins > 1 {
            format!("{name} ({id})")
        } else {
            name.clone()
        }
    };
    egui::ComboBox::from_id_salt(salt)
        .selected_text(chosen.map_or_else(|| "No image".to_string(), name))
        .width(ui::FIELD_WIDTH)
        .show_ui(ui, |ui| {
            for dataset in workspace.datasets() {
                ui.selectable_value(chosen, Some(dataset.id), name(dataset.id));
            }
        })
        .response
}
