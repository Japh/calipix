//! The desktop window. All behaviour lives in [`AppState`]; this module
//! only draws it and forwards input.

use std::path::PathBuf;

use anyhow::{Result, anyhow};
use eframe::egui;
use imager_command::{Args, MenuItem};
use imager_core::Image;

use crate::state::{AppState, Status};

/// Opens the application window and blocks until it is closed.
pub fn run(files: Vec<PathBuf>) -> Result<()> {
    let mut state = AppState::new();
    for file in files {
        state.open(file);
    }
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("imager")
            .with_inner_size([1100.0, 760.0])
            .with_drag_and_drop(true),
        ..Default::default()
    };
    eframe::run_native(
        "imager",
        options,
        Box::new(move |_cc| Ok(Box::new(Gui::new(state)))),
    )
    .map_err(|e| anyhow!("could not start the desktop interface: {e}"))
}

/// Unattended capture of the window, for checking the interface without a
/// person at the screen. Enabled by setting `IMAGER_CAPTURE` to a PNG path;
/// `IMAGER_CAPTURE_RUN` may name a command to run first. The window closes
/// once the picture is saved.
struct Capture {
    path: PathBuf,
    run_first: Option<String>,
    frame: u32,
}

impl Capture {
    fn from_env() -> Option<Capture> {
        Some(Capture {
            path: std::env::var_os("IMAGER_CAPTURE")?.into(),
            run_first: std::env::var("IMAGER_CAPTURE_RUN").ok(),
            frame: 0,
        })
    }
}

struct Gui {
    state: AppState,
    capture: Option<Capture>,
    /// The image currently uploaded for display and its texture, or why it
    /// could not be shown.
    shown: Option<(Image, std::result::Result<egui::TextureHandle, String>)>,
}

impl Gui {
    fn new(state: AppState) -> Gui {
        Gui {
            state,
            capture: Capture::from_env(),
            shown: None,
        }
    }

    /// Makes sure the texture matches the active image. Images are
    /// immutable snapshots, so a pointer comparison detects every change.
    fn sync_texture(&mut self, ctx: &egui::Context) {
        let Some(active) = self.state.active().map(|d| d.image.clone()) else {
            self.shown = None;
            return;
        };
        if self
            .shown
            .as_ref()
            .is_some_and(|(image, _)| image.ptr_eq(&active))
        {
            return;
        }
        let texture = upload(ctx, &active);
        self.shown = Some((active, texture));
    }

    fn drive_capture(&mut self, ctx: &egui::Context) {
        let Some(capture) = &mut self.capture else {
            return;
        };
        capture.frame += 1;
        ctx.request_repaint();
        match capture.frame {
            10 => {
                if let Some(id) = capture.run_first.take() {
                    self.state.run(&id, &Args::new());
                }
            }
            30 => ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(Default::default())),
            _ => {}
        }
        let shot = ctx.input(|i| {
            i.events.iter().find_map(|e| match e {
                egui::Event::Screenshot { image, .. } => Some(image.clone()),
                _ => None,
            })
        });
        if let Some(shot) = shot {
            let path = self.capture.take().expect("checked above").path;
            if let Err(e) = save_screenshot(&shot, &path) {
                eprintln!("error: could not save capture: {e}");
            }
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }

    fn menu_bar(&mut self, ui: &mut egui::Ui) {
        let mut clicked = None;
        egui::MenuBar::new().ui(ui, |ui| {
            let items = self.state.menu().to_vec();
            for (index, item) in items.iter().enumerate() {
                menu_item(ui, &self.state, item, &mut clicked);
                // Edit sits after the first menu (File), as users expect.
                if index == 0 {
                    self.edit_menu(ui);
                }
            }
        });
        if let Some(id) = clicked {
            self.state.run(&id, &Args::new());
        }
    }

    fn edit_menu(&mut self, ui: &mut egui::Ui) {
        ui.menu_button("Edit", |ui| {
            if ui
                .add_enabled(
                    self.state.can_undo(),
                    egui::Button::new("Undo").shortcut_text("Ctrl+Z"),
                )
                .clicked()
            {
                self.state.undo();
            }
            if ui
                .add_enabled(
                    self.state.can_redo(),
                    egui::Button::new("Redo").shortcut_text("Ctrl+Shift+Z"),
                )
                .clicked()
            {
                self.state.redo();
            }
        });
    }

    fn shortcuts(&mut self, ui: &mut egui::Ui) {
        use egui::{Key, Modifiers};
        let redo = ui.input_mut(|i| i.consume_key(Modifiers::COMMAND | Modifiers::SHIFT, Key::Z));
        let undo = ui.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::Z));
        if redo && self.state.can_redo() {
            self.state.redo();
        } else if undo && self.state.can_undo() {
            self.state.undo();
        }
    }

    fn status_bar(&self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            match &self.state.status {
                Status::Idle => {}
                Status::Done(message) => {
                    ui.label(message);
                }
                Status::Failed(message) => {
                    ui.colored_label(ui.visuals().error_fg_color, message);
                }
            }
            if let Some(dataset) = self.state.active() {
                let meta = dataset.image.meta();
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(format!(
                        "{}   {} x {}   {}   {} plane(s)",
                        meta.name,
                        meta.width(),
                        meta.height(),
                        meta.dtype(),
                        meta.plane_count()
                    ));
                });
            }
        });
    }

    fn viewer(&self, ui: &mut egui::Ui) {
        match &self.shown {
            None => {
                ui.centered_and_justified(|ui| {
                    ui.label("Drop an image file here to open it");
                });
            }
            Some((_, Err(problem))) => {
                ui.centered_and_justified(|ui| {
                    ui.colored_label(ui.visuals().error_fg_color, problem);
                });
            }
            Some((_, Ok(texture))) => {
                let available = ui.available_size();
                let size = texture.size_vec2();
                let scale = (available.x / size.x).min(available.y / size.y);
                ui.centered_and_justified(|ui| {
                    ui.add(
                        egui::Image::new(egui::load::SizedTexture::new(texture.id(), size))
                            .fit_to_exact_size(size * scale),
                    );
                });
            }
        }
    }
}

fn menu_item(ui: &mut egui::Ui, state: &AppState, item: &MenuItem, clicked: &mut Option<String>) {
    match item {
        MenuItem::Submenu { title, items } => {
            ui.menu_button(title, |ui| {
                for item in items {
                    menu_item(ui, state, item, clicked);
                }
            });
        }
        MenuItem::Command { id, title } => {
            let unavailable = state.unavailable_reason(id);
            let response = ui.add_enabled(unavailable.is_none(), egui::Button::new(title));
            if let Some(reason) = unavailable {
                response.on_disabled_hover_text(reason);
            } else if response.clicked() {
                *clicked = Some(id.clone());
            }
        }
    }
}

fn save_screenshot(shot: &egui::ColorImage, path: &std::path::Path) -> Result<()> {
    use imager_core::{Buffer, ColorModel, DType, ImageMeta};
    let [width, height] = shot.size;
    let mut meta = ImageMeta::xyczt(DType::U8, width, height, 3, 1, 1)?;
    meta.color = ColorModel::Rgb;
    let planes = (0..3)
        .map(|c| {
            Buffer::from_vec(
                shot.pixels
                    .iter()
                    .map(|p| p.to_array()[c])
                    .collect::<Vec<u8>>(),
            )
        })
        .collect();
    imager_io::save(&Image::from_planes(meta, planes)?, path)?;
    Ok(())
}

/// Renders the first plane of `image` on the CPU and uploads it.
fn upload(ctx: &egui::Context, image: &Image) -> std::result::Result<egui::TextureHandle, String> {
    let meta = image.meta();
    let limit = ctx.input(|i| i.max_texture_side);
    if meta.width() > limit || meta.height() > limit {
        return Err(format!(
            "{} is {} x {}, larger than this build can display ({limit} pixels a side)",
            meta.name,
            meta.width(),
            meta.height()
        ));
    }
    let position = vec![0; meta.ndim() - 2];
    let rendered = imager_render::render_plane(image, &position).map_err(|e| e.to_string())?;
    let pixels =
        egui::ColorImage::from_rgba_unmultiplied([rendered.width, rendered.height], &rendered.data);
    // Nearest-neighbour sampling: scientific images must show real pixels.
    Ok(ctx.load_texture("active-image", pixels, egui::TextureOptions::NEAREST))
}

impl eframe::App for Gui {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let dropped: Vec<PathBuf> = ui.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .map(|f| f.path().to_path_buf())
                .collect()
        });
        for path in dropped {
            self.state.open(path);
        }
        self.shortcuts(ui);
        self.drive_capture(ui.ctx());

        egui::Panel::top("menu").show(ui, |ui| self.menu_bar(ui));
        egui::Panel::bottom("status").show(ui, |ui| self.status_bar(ui));
        self.sync_texture(ui.ctx());
        egui::CentralPanel::default().show(ui, |ui| self.viewer(ui));
    }
}
