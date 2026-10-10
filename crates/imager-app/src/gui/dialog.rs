//! Parameter dialogs. One function draws the dialog of any command from
//! its [`Dialog`] model; no command has dialog code of its own.

use eframe::egui::{self, Key};
use imager_command::{DatasetId, ParamType, Workspace};

use super::{FilePicker, ui};
use crate::state::{AppState, Dialog, Draft, Field};

/// What the person did with the dialog this frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Still filling it in.
    Open,
    /// Pressed OK with valid values.
    Accepted,
    Cancelled,
}

const WIDTH: f32 = 420.0;

pub fn show(
    ctx: &egui::Context,
    dialog: &mut Dialog,
    state: &AppState,
    picker: &mut dyn FilePicker,
) -> Outcome {
    let workspace = &state.session().workspace;
    let mut outcome = Outcome::Open;
    let modal = egui::Modal::new(egui::Id::new("command_dialog")).show(ctx, |ui| {
        ui.set_width(WIDTH);
        ui.heading(&dialog.spec.title);
        if !dialog.spec.summary.is_empty() {
            ui::weak(ui, dialog.spec.summary.clone());
        }
        ui.add_space(ui::GAP);

        let spec = dialog.spec.clone();
        let mut rows = |ui: &mut egui::Ui, advanced: bool, fields: &mut [Field]| {
            egui::Grid::new(("fields", advanced))
                .num_columns(2)
                .spacing([ui::GAP * 1.5, 8.0])
                .show(ui, |ui| {
                    for field in fields.iter_mut().filter(|f| f.param.advanced == advanced) {
                        row(ui, field, workspace, &mut |mode, current| {
                            let mut request = state.path_request(&spec, mode, &Default::default());
                            let current = std::path::Path::new(current);
                            if let Some(dir) = current.parent().filter(|d| d.is_dir()) {
                                request.directory = Some(dir.to_path_buf());
                            }
                            picker.pick(&request)
                        });
                        ui.end_row();
                    }
                });
        };
        rows(ui, false, &mut dialog.fields);
        if dialog.has_advanced() {
            ui.add_space(ui::GAP / 2.0);
            let section = egui::CollapsingHeader::new("More options")
                .id_salt("advanced")
                .open(Some(dialog.show_advanced))
                .show(ui, |ui| rows(ui, true, &mut dialog.fields));
            if section.header_response.clicked() {
                dialog.show_advanced = !dialog.show_advanced;
            }
        }

        ui.add_space(ui::GAP);
        ui.separator();
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

/// One parameter: its label, its control with the unit after it, and under
/// the control whatever is wrong with the value.
fn row(
    ui: &mut egui::Ui,
    field: &mut Field,
    workspace: &Workspace,
    browse: &mut dyn FnMut(imager_command::PathMode, &str) -> Option<std::path::PathBuf>,
) {
    let param = field.param.clone();
    let is_bool = matches!(field.draft, Draft::Bool(_));
    // A checkbox carries its own label.
    let label = ui.label(if is_bool { "" } else { param.label.as_str() });
    let label = if param.doc.is_empty() {
        label
    } else {
        label.on_hover_text(&param.doc)
    };

    ui.vertical(|ui| {
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
            control.on_hover_text(&param.doc);
        }
        if let Some(problem) = field.problem(workspace) {
            ui::problem(ui, &problem);
        }
    });
}

fn control(
    ui: &mut egui::Ui,
    field: &mut Field,
    workspace: &Workspace,
    browse: &mut dyn FnMut(imager_command::PathMode, &str) -> Option<std::path::PathBuf>,
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
                .width(180.0)
                .show_ui(ui, |ui| {
                    for choice in choices {
                        ui.selectable_value(text, choice.clone(), choice);
                    }
                })
                .response
        }
        (Draft::Text(text), ParamType::Path(mode)) => {
            let edit = ui.add(egui::TextEdit::singleline(text).desired_width(230.0));
            if ui.button("Browse…").clicked()
                && let Some(path) = browse(*mode, text)
            {
                *text = path.display().to_string();
            }
            edit
        }
        (Draft::Text(text), ty) => {
            let mut edit = egui::TextEdit::singleline(text).desired_width(230.0);
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
        .width(180.0)
        .show_ui(ui, |ui| {
            for dataset in workspace.datasets() {
                ui.selectable_value(chosen, Some(dataset.id), name(dataset.id));
            }
        })
        .response
}
