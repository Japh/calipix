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
            rows(ui, workspace, &mut dialog.fields, false, &mut browse);
            if dialog.has_advanced() {
                ui.add_space(ui::GAP / 2.0);
                let section = egui::CollapsingHeader::new("More options")
                    .id_salt("advanced")
                    .open(Some(dialog.show_advanced))
                    .show(ui, |ui| {
                        rows(ui, workspace, &mut dialog.fields, true, &mut browse);
                    });
                if section.header_response.clicked() {
                    dialog.show_advanced = !dialog.show_advanced;
                }
            }

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
            if valid && ui.input(|i| i.key_pressed(Key::Enter)) {
                outcome = Outcome::Accepted;
            }
        });
    if modal.should_close() && outcome == Outcome::Open {
        outcome = Outcome::Cancelled;
    }
    outcome
}

/// The basic or the advanced parameters, one row each: label, then the
/// control with its unit. Whatever is wrong with a value goes on a row of
/// its own directly under the control.
fn rows(
    ui: &mut egui::Ui,
    workspace: &Workspace,
    fields: &mut [Field],
    advanced: bool,
    browse: &mut Browse<'_>,
) {
    egui::Grid::new(("fields", advanced))
        .num_columns(2)
        .spacing([ui::GAP * 1.5, 7.0])
        .show(ui, |ui| {
            for field in fields.iter_mut().filter(|f| f.param.advanced == advanced) {
                let param = field.param.clone();
                // A checkbox carries its own label.
                let is_bool = matches!(field.draft, Draft::Bool(_));
                let label = ui.label(if is_bool { "" } else { param.label.as_str() });
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
        });
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
            egui::ComboBox::from_id_salt(&param.name)
                .selected_text(text.as_str())
                .width(ui::FIELD_WIDTH)
                .show_ui(ui, |ui| {
                    for choice in choices {
                        ui.selectable_value(text, choice.clone(), choice);
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
