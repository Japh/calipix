//! The desktop window. All behaviour lives in [`AppState`]; the modules
//! here only draw it and forward input.
//!
//! - [`layout`]: the dockable workspace (the only user of the docking crate)
//! - [`viewer`]: what an image tab shows (the only user of `calipix-view`)
//! - [`panels`], [`display`], [`histogram`], [`results`], [`plot`],
//!   [`regions`]: the panels around the images
//! - [`tools`]: the tool strip, and the keys that act on a selection
//! - [`overlay`]: what is drawn over an image: its selection and regions
//! - [`menu`], [`palette`], [`shortcuts`]: the three ways to choose a command
//! - [`dialog`]: parameter dialogs generated from command specs
//! - [`status`]: the status bar
//! - [`ui`]: the widgets and measurements all of the above share

mod capture;
pub mod dialog;
pub mod display;
pub mod histogram;
pub mod layout;
pub mod menu;
pub mod overlay;
pub mod palette;
pub mod panels;
pub mod plot;
pub mod regions;
pub mod results;
pub mod shortcuts;
pub mod status;
pub mod tools;
pub mod ui;
pub mod viewer;

use std::path::PathBuf;
use std::sync::mpsc::{Receiver, TryRecvError, channel};

use std::sync::{Arc, Mutex};

use anyhow::{Result, anyhow};
use calipix_command::{Args, DatasetId, PathMode};
use eframe::egui;

use crate::state::{
    AppState, Dialog, Draft, Invocation, Panel, PathRequest, Settings, Status, UiAction, settings,
};
use layout::{Layout, Pane, PaneHost};
use palette::Palette;
use viewer::{Viewer, ViewerResponse};

/// Asks the person for a file or folder.
pub trait FilePicker {
    /// The chosen path, or `None` if they changed their mind.
    fn pick(&mut self, request: &PathRequest) -> Option<PathBuf>;

    /// Starts asking and returns where the answer will arrive, so that
    /// the window keeps drawing while the person chooses. A picker that
    /// answers at once need only implement [`pick`](Self::pick).
    fn begin(&mut self, request: &PathRequest) -> Receiver<Option<PathBuf>> {
        let (sender, answer) = channel();
        // The receiver is right here, so this cannot fail.
        let _ = sender.send(self.pick(request));
        answer
    }
}

/// The operating system's file dialog.
struct NativePicker;

fn native_dialog(request: &PathRequest) -> Option<PathBuf> {
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

impl FilePicker for NativePicker {
    fn pick(&mut self, request: &PathRequest) -> Option<PathBuf> {
        native_dialog(request)
    }

    /// The system's dialog blocks whoever shows it until it is answered,
    /// so it is shown from a thread of its own: the window goes on
    /// drawing, and a dialog that fails takes only that thread with it
    /// (the dropped sender is how the interface finds out).
    fn begin(&mut self, request: &PathRequest) -> Receiver<Option<PathBuf>> {
        let (sender, answer) = channel();
        let request = request.clone();
        let _ = std::thread::Builder::new()
            .name("Calipix file dialog".to_string())
            .spawn(move || {
                let _ = sender.send(native_dialog(&request));
            });
        answer
    }
}

/// What a picked path is for.
enum PickFor {
    /// The one missing parameter of a command, which then runs.
    Command {
        command: String,
        param: String,
        args: Args,
    },
    /// A path field of the open dialog.
    DialogField(String),
}

/// A file dialog that is open, and what its answer is for.
struct Picking {
    purpose: PickFor,
    answer: Receiver<Option<PathBuf>>,
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
    /// A path to ask for once this frame is drawn.
    wanted_pick: Option<(PickFor, PathRequest)>,
    picking: Option<Picking>,
    /// What the viewer reported for the active image last frame.
    view: ViewerResponse,
    /// The active image the tabs were last arranged for.
    shown_active: Option<DatasetId>,
    prepared: bool,
    layout_changed: bool,
    /// Errors the GPU reported since the last frame.
    gpu_errors: Arc<Mutex<Vec<String>>>,
    /// Processor time the last frame took, as the window reports it.
    frame_seconds: Option<f32>,
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
            // Without a GPU the viewer draws through the CPU renderer,
            // which is also how the tests run it.
            viewer: Viewer::new(None, false),
            picker,
            wanted_pick: None,
            picking: None,
            view: ViewerResponse::default(),
            shown_active: None,
            prepared: false,
            layout_changed: false,
            gpu_errors: Arc::default(),
            frame_seconds: None,
        }
    }

    /// Gives the image viewer a GPU to draw with. `force_cpu` keeps it to
    /// the CPU renderer all the same.
    pub fn use_gpu(&mut self, gpu: Option<calipix_view::ViewerGpu>, force_cpu: bool) {
        self.viewer = Viewer::new(gpu, force_cpu);
    }

    /// What the viewer reported for the active image on the last frame.
    pub fn view(&self) -> &ViewerResponse {
        &self.view
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
                // Shown once this frame is drawn; see `file_dialog`.
                let purpose = PickFor::Command {
                    command,
                    param,
                    args,
                };
                self.wanted_pick = Some((purpose, request));
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
        self.palette.open || self.dialog.is_some() || self.about_open || self.picking.is_some()
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
        let gpu_errors = std::mem::take(&mut *self.gpu_errors.lock().expect("errors poisoned"));
        for error in gpu_errors {
            self.state
                .report_problem(format!("The GPU reported a problem: {error}"));
        }

        let dropped: Vec<PathBuf> = ctx.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .map(|file| file.path().to_path_buf())
                .collect()
        });
        self.state.open_files(dropped);

        // Choosing commands: shortcuts, the palette, the menus. While the
        // Results table has the keyboard, its keys come first.
        if !self.modal_open() {
            if self.state.table_was_drawn() {
                results::keys(&ctx, &mut self.state);
            }
            if self.state.regions_were_drawn() {
                regions::keys(&ctx, &mut self.state);
            }
            // Then the keys that act on the selection, where there is
            // one for them to act on.
            tools::keys(&ctx, &mut self.state);
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
        let status = egui::Panel::bottom("status")
            .show(ui, |ui| status::show(ui, &self.state, &self.view))
            .inner;
        if status.cancel {
            self.state.cancel();
        }
        if status.fit {
            self.invoke("view.zoom_fit", &Args::new());
        }

        egui::Panel::left("tools")
            .exact_size(tools::WIDTH)
            .resizable(false)
            .show(ui, |ui| tools::strip(ui, &mut self.state));

        self.apply_ui_actions();
        self.workspace(ui);
        self.dialogs(&ctx);
        self.file_dialog(&ctx);

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
            let active = self.state.active().map(|dataset| dataset.id);
            match action {
                UiAction::Zoom(zoom) => {
                    if let Some(id) = active {
                        self.viewer.zoom(id, zoom);
                    }
                }
                UiAction::Step(delta) => {
                    if let Some(id) = active {
                        self.viewer.step(&mut self.state, id, delta.into());
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
                UiAction::TileImages => {
                    let text = if self.layout.tile_images() {
                        "Images shown side by side"
                    } else {
                        "Show Images Side by Side: the images are already apart"
                    };
                    self.state.status = Status::Done(text.to_string());
                }
                UiAction::OpenPalette => self.palette.open(),
                UiAction::About => self.about_open = true,
                // Entirely the state's business.
                UiAction::Undo
                | UiAction::Redo
                | UiAction::SetTheme(_)
                | UiAction::ClearRecent
                | UiAction::SetTool(_)
                | UiAction::ToggleRegions
                | UiAction::SetRegionLabels(_) => {}
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
            state: &mut self.state,
            viewer: &mut self.viewer,
            view: &mut self.view,
            clicked_image: None,
            chosen: None,
        };
        let events = egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(ui.visuals().extreme_bg_color))
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
        // What the panels' own buttons asked for.
        for (id, args) in self.state.take_requests() {
            self.invoke(&id, &args);
        }
        for panel in Panel::ALL {
            self.state.set_panel_open(panel, self.layout.is_open(panel));
        }
    }

    fn dialogs(&mut self, ctx: &egui::Context) {
        if let Some(dialog) = &mut self.dialog {
            let mut browse = None;
            match dialog::show(ctx, dialog, &self.state, &mut browse) {
                dialog::Outcome::Open => {}
                dialog::Outcome::Accepted => self.accept_dialog(),
                dialog::Outcome::Cancelled => self.dialog = None,
            }
            if let Some((field, request)) = browse {
                self.wanted_pick = Some((PickFor::DialogField(field), request));
            }
        }
        if self.about_open {
            let frame = ui::modal_frame(&ctx.global_style());
            let about = egui::Modal::new(egui::Id::new("about")).frame(frame);
            let renderer = self.viewer.renderer_description();
            let modal = about.show(ctx, |ui| {
                ui.set_width(340.0);
                ui.heading("Calipix");
                ui::weak(ui, concat!("Version ", env!("CARGO_PKG_VERSION")));
                ui.add_space(ui::GAP);
                ui.label("Scientific image processing: a Rust port of ImageJ.");
                ui.label(concat!("Licence: ", env!("CARGO_PKG_LICENSE")));
                ui.hyperlink(env!("CARGO_PKG_REPOSITORY"));
                ui.add_space(ui::GAP / 2.0);
                ui::weak(ui, format!("Images are drawn by: {renderer}"));
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

    /// Opens the file dialog asked for this frame, and takes the answer of
    /// the one that is open.
    ///
    /// The system's dialog runs on its own thread (see [`NativePicker`]),
    /// so this never waits: while the person is choosing, the window keeps
    /// drawing and says what it is waiting for.
    fn file_dialog(&mut self, ctx: &egui::Context) {
        if let Some((purpose, request)) = self.wanted_pick.take()
            && self.picking.is_none()
        {
            let answer = self.picker.begin(&request);
            self.picking = Some(Picking { purpose, answer });
        }
        let Some(picking) = &self.picking else {
            return;
        };
        let answer = match picking.answer.try_recv() {
            Ok(answer) => answer,
            Err(TryRecvError::Empty) => {
                let frame = ui::modal_frame(&ctx.global_style());
                egui::Modal::new(egui::Id::new("file_dialog_wait"))
                    .frame(frame)
                    .show(ctx, |ui| {
                        ui.label("Choose a file in the dialog that has opened.");
                        ui::weak(ui, "It may be behind this window.");
                    });
                ctx.request_repaint_after(std::time::Duration::from_millis(50));
                return;
            }
            Err(TryRecvError::Disconnected) => {
                self.picking = None;
                self.state.report_problem(
                    "The system's file dialog could not be shown, so no file was chosen",
                );
                return;
            }
        };
        let Some(picking) = self.picking.take() else {
            return;
        };
        let Some(path) = answer else {
            return;
        };
        match picking.purpose {
            PickFor::Command {
                command,
                param,
                args,
            } => self.state.picked(&command, &param, &args, path),
            PickFor::DialogField(name) => {
                self.state.remember_dir(&path);
                let field = self.dialog.as_mut().and_then(|d| d.field_mut(&name));
                if let Some(field) = field {
                    field.draft = Draft::Text(path.display().to_string());
                }
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
    state: &'a mut AppState,
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
                    // The active image is underlined, so that it can be told
                    // apart when several images are side by side.
                    Ok(dataset) if workspace.active() == Some(dataset.id) => {
                        egui::RichText::new(&dataset.image.meta().name)
                            .strong()
                            .underline()
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
                let id = DatasetId(id);
                if self.state.session().workspace.get(id).is_err() {
                    return;
                }
                let response = self.viewer.show(ui, self.state, id);
                if response.clicked {
                    self.clicked_image = Some(id);
                }
                if self.state.session().workspace.active() == Some(id) {
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
        let open_id = open.id.clone();
        let mut label = crate::state::menu::label(open);
        if let Some(shortcut) = self.state.shortcut_text(&open_id) {
            label = format!("{label}   {shortcut}");
        }
        ui.vertical_centered(|ui| {
            ui.add_space((area.height() / 2.0 - 40.0).max(0.0));
            let hint = "Drop image files here to open them";
            ui.label(egui::RichText::new(hint).color(ui::ON_IMAGE_BACKGROUND));
            ui.add_space(ui::GAP);
            if ui.button(label).clicked() {
                self.chosen = Some(open_id);
            }
        });
    }
}

struct Window {
    shell: Shell,
    capture: Option<capture::Capture>,
}

impl eframe::App for Window {
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        self.shell.frame_seconds = frame.info().cpu_usage;
        self.shell.ui(ui);
        if let Some(capture) = &mut self.capture
            && capture.drive(ui.ctx(), &mut self.shell)
        {
            self.capture = None;
        }
    }

    fn raw_input_hook(&mut self, _ctx: &egui::Context, raw_input: &mut egui::RawInput) {
        if let Some(capture) = &mut self.capture {
            capture.inject(raw_input);
        }
    }

    fn on_exit(&mut self) {
        self.shell.persist();
    }
}

/// True if the environment variable is set to something.
fn flag(name: &str) -> bool {
    std::env::var_os(name).is_some_and(|value| !value.is_empty())
}

/// How the window's renderer is set up.
///
/// - wgpu's debug layer is left out. In debug builds wgpu asks for it,
///   and the Direct3D 12 one crashes on software adapters on some
///   machines; nothing here relies on it.
/// - `CALIPIX_SOFTWARE_ADAPTER=1` picks a software adapter, to try what a
///   machine without a usable GPU would get. The viewer then draws
///   through the CPU renderer of its own accord.
fn native_options(size: [f32; 2]) -> eframe::NativeOptions {
    use eframe::{egui_wgpu, wgpu};
    let mut options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Calipix")
            .with_inner_size(size)
            .with_min_inner_size([640.0, 420.0])
            .with_drag_and_drop(true),
        renderer: eframe::Renderer::Wgpu,
        ..Default::default()
    };
    if let egui_wgpu::WgpuSetup::CreateNew(setup) = &mut options.wgpu_options.wgpu_setup {
        setup
            .instance_descriptor
            .flags
            .remove(wgpu::InstanceFlags::DEBUG);
        if flag("CALIPIX_SOFTWARE_ADAPTER") {
            setup.native_adapter_selector = Some(Arc::new(|adapters, _surface| {
                adapters
                    .iter()
                    .find(|a| a.get_info().device_type == wgpu::DeviceType::Cpu)
                    .cloned()
                    .ok_or_else(|| "no software adapter is available".to_string())
            }));
        }
    }
    options
}

/// Opens the application window and blocks until it is closed.
pub fn run(files: Vec<PathBuf>) -> Result<()> {
    let capture = capture::Capture::from_env()?;
    let mut registry = calipix_commands::standard_registry();
    if flag("CALIPIX_DEMO_COMMANDS") {
        crate::demo::register(&mut registry)?;
    }
    // A capture must look the same on every machine, so it leaves the
    // person's settings alone unless told where to keep its own.
    let own_settings = std::env::var_os("CALIPIX_CONFIG_DIR").is_some();
    let settings_path = settings::default_path().filter(|_| capture.is_none() || own_settings);
    let (loaded, problem) = match &settings_path {
        Some(path) => Settings::load(path),
        None => (Settings::default(), None),
    };
    let mut state = AppState::build(registry, loaded, settings_path);
    if let Some(problem) = problem {
        state.report_problem(problem);
    }
    state.open_files(files);

    let size = std::env::var("CALIPIX_WINDOW_SIZE")
        .ok()
        .and_then(|text| {
            let (width, height) = text.split_once('x')?;
            Some([width.parse().ok()?, height.parse().ok()?])
        })
        .unwrap_or([1280.0, 840.0]);
    let mut window = Window {
        shell: Shell::new(state),
        capture,
    };
    let force_cpu = flag("CALIPIX_CPU_RENDERER");
    eframe::run_native(
        "calipix",
        native_options(size),
        Box::new(move |cc| {
            // The viewer shares the renderer egui itself draws with.
            let render_state = cc.wgpu_render_state.as_ref();
            if let Some(render_state) = render_state {
                // A GPU error is a problem to report, not a reason to stop.
                let (errors, ctx) = (window.shell.gpu_errors.clone(), cc.egui_ctx.clone());
                render_state
                    .device
                    .on_uncaptured_error(Arc::new(move |error| {
                        if let Ok(mut errors) = errors.lock() {
                            errors.push(error.to_string());
                        }
                        ctx.request_repaint();
                    }));
            }
            let gpu = render_state.map(calipix_view::ViewerGpu::from_render_state);
            window.shell.use_gpu(gpu, force_cpu);
            Ok(Box::new(window))
        }),
    )
    .map_err(|e| anyhow!("could not start the desktop interface: {e}"))
}
