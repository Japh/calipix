//! The desktop window. All behaviour lives in [`AppState`]; the modules
//! here only draw it and forward input.
//!
//! - [`layout`]: the dockable workspace (the only user of the docking crate)
//! - [`viewer`]: what an image tab shows
//! - [`panels`]: the Info, History and Log panels
//! - [`menu`], [`palette`], [`shortcuts`]: the three ways to choose a command
//! - [`dialog`]: parameter dialogs generated from command specs
//! - [`status`]: the status bar
//! - [`ui`]: the widgets and measurements all of the above share

mod capture;
pub mod dialog;
pub mod layout;
pub mod menu;
pub mod palette;
pub mod panels;
pub mod shortcuts;
pub mod status;
pub mod ui;
pub mod viewer;

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Result, anyhow};
use eframe::egui;
use imager_command::{Args, DatasetId, PathMode};

use crate::state::{
    AppState, Dialog, Invocation, Panel, PathRequest, Settings, UiAction, settings,
};
use layout::{Layout, Pane, PaneHost};
use palette::Palette;
use viewer::{Viewer, ViewerResponse};

/// Asks the person for a file or folder.
pub trait FilePicker {
    /// The chosen path, or `None` if they changed their mind.
    fn pick(&mut self, request: &PathRequest) -> Option<PathBuf>;
}

/// The operating system's file dialog.
struct NativePicker;

impl FilePicker for NativePicker {
    fn pick(&mut self, request: &PathRequest) -> Option<PathBuf> {
        let mut dialog = rfd::FileDialog::new().set_title(&request.title);
        if let Some(directory) = request.directory.as_deref().filter(|d| d.is_dir()) {
            dialog = dialog.set_directory(directory);
        }
        if let Some(name) = &request.file_name {
            dialog = dialog.set_file_name(name);
        }
        for (name, extensions) in &request.filters {
            dialog = dialog.add_filter(name, extensions);
        }
        match request.mode {
            PathMode::Open => dialog.pick_file(),
            PathMode::Save => dialog.save_file(),
            PathMode::Directory => dialog.pick_folder(),
        }
    }
}

/// A command waiting for the person to pick a path.
struct PendingPick {
    request: PathRequest,
    command: String,
    param: String,
    args: Args,
}

/// The whole interface inside the window. It draws into any `egui::Ui`,
/// which is how tests drive it without a window.
pub struct Shell {
    pub state: AppState,
    pub layout: Layout,
    pub palette: Palette,
    /// The parameter dialog on screen, if any.
    pub dialog: Option<Dialog>,
    pub about_open: bool,
    viewer: Viewer,
    picker: Box<dyn FilePicker>,
    pending_pick: Option<PendingPick>,
    /// What the viewer reported for the active image last frame.
    view: ViewerResponse,
    /// The active image the tabs were last arranged for.
    shown_active: Option<DatasetId>,
    prepared: bool,
    layout_changed: bool,
}

impl Shell {
    pub fn new(state: AppState) -> Shell {
        Shell::with_picker(state, Box::new(NativePicker))
    }

    /// As [`new`](Self::new), with something else standing in for the
    /// operating system's file dialog.
    pub fn with_picker(mut state: AppState, picker: Box<dyn FilePicker>) -> Shell {
        // A layout that does not load is not worth a message: the default
        // is right there.
        let saved = state.settings().layout.as_ref();
        let layout = saved.and_then(Layout::restore).unwrap_or_default();
        for panel in Panel::ALL {
            state.set_panel_open(panel, layout.is_open(panel));
        }
        Shell {
            state,
            layout,
            palette: Palette::default(),
            dialog: None,
            about_open: false,
            viewer: Viewer::new(),
            picker,
            pending_pick: None,
            view: ViewerResponse::default(),
            shown_active: None,
            prepared: false,
            layout_changed: false,
        }
    }

    /// Chooses a command as a menu item does: it runs at once, or asks for
    /// what it needs first.
    pub fn invoke(&mut self, id: &str, args: &Args) {
        match self.state.invoke(id, args) {
            Invocation::Started | Invocation::Unavailable(_) => {}
            Invocation::Dialog(dialog) => self.dialog = Some(*dialog),
            Invocation::PickPath {
                request,
                command,
                param,
                args,
            } => {
                // Shown once this frame is drawn; see `ui`.
                self.pending_pick = Some(PendingPick {
                    request,
                    command,
                    param,
                    args,
                });
            }
        }
    }

    /// Presses OK on the open dialog. Does nothing if its values are not
    /// all valid.
    pub fn accept_dialog(&mut self) {
        if let Some(dialog) = &self.dialog
            && self.state.accept(dialog)
        {
            self.dialog = None;
        }
    }

    fn modal_open(&self) -> bool {
        self.palette.open || self.dialog.is_some() || self.about_open
    }

    /// Draws one frame of the interface and handles its input.
    pub fn ui(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        if !self.prepared {
            self.prepared = true;
            ui::apply_style(&ctx);
            let waker = ctx.clone();
            self.state
                .set_waker(Arc::new(move || waker.request_repaint()));
        }
        ui::apply_theme(&ctx, self.state.settings().theme);
        self.state.poll();

        let dropped: Vec<PathBuf> = ctx.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .map(|file| file.path().to_path_buf())
                .collect()
        });
        self.state.open_files(dropped);

        // Choosing commands: shortcuts, the palette, the menus.
        if !self.modal_open() {
            for id in shortcuts::pressed(&ctx, &self.state) {
                self.invoke(&id, &Args::new());
            }
        }
        if let Some(id) = self.palette.show(&ctx, &self.state) {
            self.invoke(&id, &Args::new());
        }
        let chosen = egui::Panel::top("menu")
            .show(ui, |ui| menu::show(ui, &self.state))
            .inner;
        if let Some((id, args)) = chosen {
            self.invoke(&id, &args);
        }
        let cancel = egui::Panel::bottom("status")
            .show(ui, |ui| status::show(ui, &self.state, &self.view))
            .inner;
        if cancel {
            self.state.cancel();
        }

        self.apply_ui_actions();
        self.workspace(ui);
        self.dialogs(&ctx);

        // The file dialog blocks until it is answered, so it comes last,
        // when the frame it interrupts is complete.
        if let Some(pick) = self.pending_pick.take()
            && let Some(path) = self.picker.pick(&pick.request)
        {
            self.state
                .picked(&pick.command, &pick.param, &pick.args, path);
        }

        self.remember_layout(&ctx);
        self.state.save_settings();
        if self.state.running().is_some() {
            // Keeps the progress display moving and shows it when due.
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
    }

    /// Carries out the interface's side of the application commands that
    /// ran since the last frame.
    fn apply_ui_actions(&mut self) {
        for action in self.state.take_ui_actions() {
            match action {
                UiAction::Zoom(zoom) => {
                    if let Some(dataset) = self.state.active() {
                        self.viewer.zoom(dataset.id, zoom);
                    }
                }
                UiAction::TogglePanel(panel) => {
                    self.layout.set_open(panel, self.state.panel_open(panel));
                }
                UiAction::ShowPanel(panel) => {
                    self.layout.set_open(panel, true);
                    self.layout.focus(Pane::Panel(panel));
                }
                UiAction::ResetLayout => {
                    self.layout = Layout::new();
                    self.shown_active = None;
                }
                UiAction::OpenPalette => self.palette.open(),
                UiAction::About => self.about_open = true,
                // Entirely the state's business.
                UiAction::Undo | UiAction::Redo | UiAction::SetTheme(_) | UiAction::ClearRecent => {
                }
            }
            self.layout_changed = true;
        }
    }

    /// Draws the docked workspace and applies what was done to its tabs.
    fn workspace(&mut self, ui: &mut egui::Ui) {
        let open: Vec<DatasetId> = self
            .state
            .session()
            .workspace
            .datasets()
            .map(|dataset| dataset.id)
            .collect();
        self.layout.sync_images(&open);
        self.viewer.retain(|id| open.contains(&id));
        // The active image changes when a command opens or closes one, or
        // is undone; its tab follows.
        let active = self.state.session().workspace.active();
        if active != self.shown_active {
            self.shown_active = active;
            if let Some(id) = active {
                self.layout.focus(Pane::Image(id.0));
            }
        }

        self.view = ViewerResponse::default();
        let mut host = Host {
            state: &self.state,
            viewer: &mut self.viewer,
            view: &mut self.view,
            clicked_image: None,
            chosen: None,
        };
        let events = egui::CentralPanel::default()
            .frame(egui::Frame::new())
            .show(ui, |ui| self.layout.ui(ui, &mut host))
            .inner;
        let (clicked_image, chosen) = (host.clicked_image, host.chosen);

        if let Some(id) = events.chosen_image.or(clicked_image) {
            self.state.set_active(id);
        }
        for id in events.close_images {
            self.invoke("file.close", &Args::new().with("input", id));
        }
        if let Some(id) = chosen {
            self.invoke(&id, &Args::new());
        }
        for panel in Panel::ALL {
            self.state.set_panel_open(panel, self.layout.is_open(panel));
        }
    }

    fn dialogs(&mut self, ctx: &egui::Context) {
        if let Some(dialog) = &mut self.dialog {
            match dialog::show(ctx, dialog, &self.state, self.picker.as_mut()) {
                dialog::Outcome::Open => {}
                dialog::Outcome::Accepted => self.accept_dialog(),
                dialog::Outcome::Cancelled => self.dialog = None,
            }
        }
        if self.about_open {
            let modal = egui::Modal::new(egui::Id::new("about")).show(ctx, |ui| {
                ui.set_width(320.0);
                ui.heading("imager");
                ui::weak(ui, concat!("Version ", env!("CARGO_PKG_VERSION")));
                ui.add_space(ui::GAP);
                ui.label("Scientific image processing: a Rust port of ImageJ.");
                ui.label(concat!("Licence: ", env!("CARGO_PKG_LICENSE")));
                ui.hyperlink(env!("CARGO_PKG_REPOSITORY"));
                ui.add_space(ui::GAP);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui::primary_button(ui, "Close", true).clicked()
                })
                .inner
            });
            if modal.inner || modal.should_close() {
                self.about_open = false;
            }
        }
    }

    /// Notes the layout for the settings file once the person has finished
    /// changing it. Dragging and resizing end with a button release.
    fn remember_layout(&mut self, ctx: &egui::Context) {
        if ctx.input(|i| i.pointer.any_released()) {
            self.layout_changed = true;
        }
        if std::mem::take(&mut self.layout_changed) {
            self.state.set_saved_layout(self.layout.save());
        }
    }

    /// Saves the layout and settings. Call when the window closes.
    pub fn persist(&mut self) {
        self.state.set_saved_layout(self.layout.save());
        self.state.save_settings();
    }
}

/// Lends the layout what it needs to fill its tabs.
struct Host<'a> {
    state: &'a AppState,
    viewer: &'a mut Viewer,
    view: &'a mut ViewerResponse,
    /// An image whose view was clicked.
    clicked_image: Option<DatasetId>,
    /// A command chosen from the welcome tab.
    chosen: Option<String>,
}

impl PaneHost for Host<'_> {
    fn title(&mut self, pane: Pane) -> egui::WidgetText {
        match pane {
            Pane::Welcome => "Welcome".into(),
            Pane::Panel(panel) => panel.title().into(),
            Pane::Image(id) => {
                let workspace = &self.state.session().workspace;
                match workspace.get(DatasetId(id)) {
                    // The active image stands out by weight, not colour.
                    Ok(dataset) if workspace.active() == Some(dataset.id) => {
                        egui::RichText::new(&dataset.image.meta().name)
                            .strong()
                            .into()
                    }
                    Ok(dataset) => dataset.image.meta().name.as_str().into(),
                    Err(_) => "Closed image".into(),
                }
            }
        }
    }

    fn show(&mut self, ui: &mut egui::Ui, pane: Pane) {
        match pane {
            Pane::Welcome => self.welcome(ui),
            Pane::Panel(panel) => panels::show(ui, panel, self.state),
            Pane::Image(id) => {
                let workspace = &self.state.session().workspace;
                let Ok(dataset) = workspace.get(DatasetId(id)) else {
                    return;
                };
                let response = self.viewer.show(ui, dataset);
                if response.clicked {
                    self.clicked_image = Some(dataset.id);
                }
                if workspace.active() == Some(dataset.id) {
                    *self.view = response;
                }
            }
        }
    }
}

impl Host<'_> {
    /// What the centre shows while no image is open.
    fn welcome(&mut self, ui: &mut egui::Ui) {
        let area = ui.available_rect_before_wrap();
        ui.painter().rect_filled(area, 0.0, ui::IMAGE_BACKGROUND);
        let Some(open) = self.state.spec("file.open") else {
            return;
        };
        ui.vertical_centered(|ui| {
            ui.add_space((area.height() / 2.0 - 40.0).max(0.0));
            let hint = "Drop image files here to open them";
            ui.label(egui::RichText::new(hint).color(ui::ON_IMAGE_BACKGROUND));
            ui.add_space(ui::GAP);
            let mut label = crate::state::menu::label(open);
            if let Some(shortcut) = self.state.shortcut_text(&open.id) {
                label = format!("{label}   {shortcut}");
            }
            if ui.button(label).clicked() {
                self.chosen = Some(open.id.clone());
            }
        });
    }
}

struct Window {
    shell: Shell,
    capture: Option<capture::Capture>,
}

impl eframe::App for Window {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.shell.ui(ui);
        if let Some(capture) = &mut self.capture
            && capture.drive(ui.ctx(), &mut self.shell)
        {
            self.capture = None;
        }
    }

    fn on_exit(&mut self) {
        self.shell.persist();
    }
}

/// Opens the application window and blocks until it is closed.
pub fn run(files: Vec<PathBuf>) -> Result<()> {
    let capture = capture::Capture::from_env()?;
    let mut registry = imager_commands::standard_registry();
    if std::env::var_os("IMAGER_DEMO_COMMANDS").is_some() {
        crate::demo::register(&mut registry)?;
    }
    // A capture must look the same on every machine, so it leaves the
    // person's settings alone.
    let settings_path = settings::default_path().filter(|_| capture.is_none());
    let (loaded, problem) = match &settings_path {
        Some(path) => Settings::load(path),
        None => (Settings::default(), None),
    };
    let mut state = AppState::build(registry, loaded, settings_path);
    if let Some(problem) = problem {
        state.report_problem(problem);
    }
    state.open_files(files);

    let size = std::env::var("IMAGER_WINDOW_SIZE")
        .ok()
        .and_then(|text| {
            let (width, height) = text.split_once('x')?;
            Some([width.parse().ok()?, height.parse().ok()?])
        })
        .unwrap_or([1180.0, 780.0]);
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("imager")
            .with_inner_size(size)
            .with_min_inner_size([640.0, 420.0])
            .with_drag_and_drop(true),
        ..Default::default()
    };
    let window = Window {
        shell: Shell::new(state),
        capture,
    };
    eframe::run_native("imager", options, Box::new(move |_cc| Ok(Box::new(window))))
        .map_err(|e| anyhow!("could not start the desktop interface: {e}"))
}
