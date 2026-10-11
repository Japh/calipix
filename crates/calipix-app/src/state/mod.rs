//! Application state behind the desktop interface. It has no dependency on
//! the GUI toolkit, so everything the interface can do is testable here.

pub mod action;
pub mod dialog;
pub mod histogram;
pub mod menu;
pub mod palette;
mod panel;
pub mod settings;
pub mod shortcut;
mod task;
pub mod view;

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use calipix_command::{
    Args, CommandSpec, Dataset, DatasetId, EffectsPolicy, Error, Executor, HistoryEntry, Outputs,
    ParamType, PathMode, Registry, Session, Value, Workspace,
};
use calipix_commands::view::DISPLAY_COMMANDS;
use calipix_core::{ColorModel, Image};

pub use action::{ActionQueue, Theme, UiAction, Zoom};
pub use dialog::{Dialog, Draft, Field};
pub use histogram::{HistogramKey, HistogramState, Histograms, PlaneHistogram};
pub use menu::{MenuCommand, MenuEntry};
pub use panel::{Dock, Panel};
pub use settings::{SavedLayout, Settings};
pub use shortcut::{KeyPress, Shortcut};
pub use task::{Running, Waker};
pub use view::ImageViewState;

use task::{Finished, Task};
use view::RangeGesture;

/// Outcome of the most recent action, shown in the status bar.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Status {
    #[default]
    Idle,
    /// What happened, or why nothing did.
    Done(String),
    /// What failed, on what, and why.
    Failed(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LogLevel {
    Info,
    Error,
}

/// One line of the Log panel.
#[derive(Clone, Debug)]
pub struct LogEntry {
    pub time: chrono::DateTime<chrono::Local>,
    pub level: LogLevel,
    pub text: String,
}

/// How a menu item shows that its command switches something.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mark {
    /// Independently on or off, like a panel being shown.
    Toggle(bool),
    /// One of several alternatives, like the theme in use.
    Choice(bool),
}

/// What the system's file dialog should ask for.
#[derive(Clone, Debug, PartialEq)]
pub struct PathRequest {
    /// Title of the dialog window.
    pub title: String,
    pub mode: PathMode,
    /// Folder to start in.
    pub directory: Option<PathBuf>,
    /// File name to suggest when saving.
    pub file_name: Option<String>,
    /// File kinds to offer, as (name, extensions without the dot).
    pub filters: Vec<(String, Vec<String>)>,
}

/// What has to happen next for a command the person chose.
#[derive(Clone, Debug, PartialEq)]
pub enum Invocation {
    /// The command is running or has already run.
    Started,
    /// The command cannot run now; the status bar says why.
    Unavailable(String),
    /// Values are needed: show this dialog, then [`AppState::accept`] it.
    Dialog(Box<Dialog>),
    /// Only a file path is needed: show the system's file dialog, then
    /// pass the answer to [`AppState::picked`].
    PickPath {
        request: PathRequest,
        command: String,
        param: String,
        args: Args,
    },
}

pub struct AppState {
    executor: Executor,
    session: Session,
    /// Filled by application commands when they run.
    queue: ActionQueue,
    /// The application commands by id.
    app_actions: BTreeMap<String, UiAction>,
    /// Every declared shortcut, most modifiers first.
    shortcuts: Vec<(Shortcut, String)>,
    menu: Vec<MenuEntry>,
    settings: Settings,
    settings_path: Option<PathBuf>,
    settings_dirty: bool,
    closed_panels: BTreeSet<Panel>,
    pub status: Status,
    log: Vec<LogEntry>,
    task: Option<Task>,
    /// Commands waiting for the running one to finish.
    pending: VecDeque<(String, Args)>,
    /// Applied actions the interface has yet to act on.
    ui_actions: Vec<UiAction>,
    waker: Option<Waker>,
    /// How each open image is being looked at.
    views: BTreeMap<DatasetId, ImageViewState>,
    /// A number per open image that changes whenever its samples may have.
    samples: BTreeMap<DatasetId, u64>,
    next_version: u64,
    gesture: Option<RangeGesture>,
    histograms: Histograms,
}

/// One line of the History panel.
#[derive(Clone, Debug, PartialEq)]
pub struct HistoryRow {
    pub title: String,
    /// The arguments the command ran with, in words.
    pub detail: String,
    pub duration: std::time::Duration,
    /// Undone, and so not in effect.
    pub undone: bool,
    /// The last step in effect: where undo is.
    pub current: bool,
    /// Whether the step changed an image, so that undo can stop on it.
    pub changed: bool,
}

impl Default for AppState {
    fn default() -> AppState {
        AppState::new()
    }
}

impl AppState {
    /// The standard commands, default settings, and nothing saved to disk.
    pub fn new() -> AppState {
        AppState::build(
            calipix_commands::standard_registry(),
            Settings::default(),
            None,
        )
    }

    /// The application as a person starts it: settings are read from
    /// `settings_path` and written back there as they change.
    pub fn with_settings_file(settings_path: PathBuf) -> AppState {
        let (settings, problem) = Settings::load(&settings_path);
        let mut state = AppState::build(
            calipix_commands::standard_registry(),
            settings,
            Some(settings_path),
        );
        if let Some(problem) = problem {
            state.report_problem(problem);
        }
        state
    }

    /// Builds the state around `registry`, to which the application
    /// commands are added.
    pub fn build(
        mut registry: Registry,
        settings: Settings,
        settings_path: Option<PathBuf>,
    ) -> AppState {
        let queue = ActionQueue::default();
        let app_actions: BTreeMap<String, UiAction> = action::register(&mut registry, &queue)
            .expect("application commands are valid and unique")
            .into_iter()
            .collect();

        let declared = registry
            .specs()
            .filter_map(|spec| Some((spec.shortcut.as_deref()?, spec.id.as_str())));
        let extra = action::EXTRA_SHORTCUTS.iter().map(|&(id, text)| (text, id));
        let mut shortcuts: Vec<(Shortcut, String)> = declared
            .chain(extra)
            // Specs are validated on registration, so these all parse.
            .filter_map(|(text, id)| Some((Shortcut::parse(text).ok()?, id.to_string())))
            .collect();
        shortcuts.sort_by_key(|(shortcut, _)| std::cmp::Reverse(shortcut.specificity()));

        let menu = menu::build(&registry, &settings.recent);
        AppState {
            // The person at the keyboard is trusted with their own files.
            executor: Executor::new(Arc::new(registry), EffectsPolicy::allow_all()),
            session: Session::new(),
            queue,
            app_actions,
            shortcuts,
            menu,
            settings,
            settings_path,
            settings_dirty: false,
            closed_panels: BTreeSet::new(),
            status: Status::Idle,
            log: Vec::new(),
            task: None,
            pending: VecDeque::new(),
            ui_actions: Vec::new(),
            waker: None,
            views: BTreeMap::new(),
            samples: BTreeMap::new(),
            next_version: 0,
            gesture: None,
            histograms: Histograms::default(),
        }
    }

    pub fn registry(&self) -> &Registry {
        self.executor.registry()
    }

    pub fn spec(&self, id: &str) -> Option<&CommandSpec> {
        self.registry().get(id).ok().map(|c| c.spec())
    }

    pub fn session(&self) -> &Session {
        &self.session
    }

    pub fn menu(&self) -> &[MenuEntry] {
        &self.menu
    }

    pub fn settings(&self) -> &Settings {
        &self.settings
    }

    pub fn log(&self) -> &[LogEntry] {
        &self.log
    }

    /// The image commands act on and the viewer shows.
    pub fn active(&self) -> Option<&Dataset> {
        let id = self.session.workspace.active()?;
        self.session.workspace.get(id).ok()
    }

    /// Makes an open image the active one, as when its tab is chosen.
    pub fn set_active(&mut self, id: DatasetId) {
        // An id that is no longer open is a stale click; nothing to do.
        let _ = self.session.workspace.set_active(id);
    }

    /// Sets what the worker thread calls when progress or a result is
    /// ready, so the interface can redraw.
    pub fn set_waker(&mut self, waker: Waker) {
        self.histograms.set_waker(waker.clone());
        self.waker = Some(waker);
    }

    // ---- Messages ----

    fn note(&mut self, text: impl Into<String>) {
        self.status = Status::Done(text.into());
    }

    /// Shows a problem in the status bar and records it in the Log.
    pub fn report_problem(&mut self, text: impl Into<String>) {
        self.fail(text);
    }

    fn fail(&mut self, text: impl Into<String>) {
        let text = text.into();
        self.log_line(LogLevel::Error, text.clone());
        self.status = Status::Failed(text);
    }

    fn log_line(&mut self, level: LogLevel, text: String) {
        self.log.push(LogEntry {
            time: chrono::Local::now(),
            level,
            text,
        });
    }

    // ---- What can run ----

    fn title(&self, id: &str) -> String {
        self.spec(id)
            .map_or_else(|| id.to_string(), |spec| spec.title.clone())
    }

    /// Why a command cannot be run right now, or `None` if it can.
    pub fn unavailable_reason(&self, id: &str) -> Option<String> {
        self.unavailable_with(id, &Args::new())
    }

    /// As [`unavailable_reason`](Self::unavailable_reason), for a command
    /// that will be given `args`.
    pub fn unavailable_with(&self, id: &str, args: &Args) -> Option<String> {
        let spec = match self.registry().get(id) {
            Ok(command) => command.spec(),
            Err(e) => return Some(e.to_string()),
        };
        let history = self.session.history();
        let busy = || {
            let task = self.task.as_ref()?;
            Some(format!("Wait for {} to finish", task.spec.title))
        };
        match self.app_actions.get(id) {
            Some(UiAction::Undo) if !history.can_undo() => Some("Nothing to undo".to_string()),
            Some(UiAction::Redo) if !history.can_redo() => Some("Nothing to redo".to_string()),
            // These change the session a running command will replace.
            Some(UiAction::Undo | UiAction::Redo) => busy(),
            Some(UiAction::Zoom(_) | UiAction::Step(_)) if self.active().is_none() => {
                Some("Open an image first".to_string())
            }
            Some(UiAction::Step(_)) if self.active().is_some_and(|d| !has_planes(&d.image)) => {
                Some("The image has a single plane".to_string())
            }
            Some(UiAction::TileImages) if self.session.workspace.len() < 2 => {
                Some("Open at least two images".to_string())
            }
            Some(UiAction::ClearRecent) if self.settings.recent.is_empty() => {
                Some("No recent files".to_string())
            }
            Some(_) => None,
            None => busy().or_else(|| {
                let needs_image = spec
                    .inputs
                    .iter()
                    .any(|p| p.ty == ParamType::Image && p.required && args.get(&p.name).is_none());
                (needs_image && self.active().is_none()).then(|| "Open an image first".to_string())
            }),
        }
    }

    /// The on/off mark a command's menu item carries, if it has one.
    pub fn mark(&self, id: &str) -> Option<Mark> {
        match self.app_actions.get(id)? {
            UiAction::TogglePanel(panel) => Some(Mark::Toggle(self.panel_open(*panel))),
            UiAction::SetTheme(theme) => Some(Mark::Choice(self.settings.theme == *theme)),
            _ => None,
        }
    }

    // ---- Shortcuts and search ----

    /// The command a key press runs, if any.
    pub fn shortcut_command(&self, press: &KeyPress) -> Option<&str> {
        self.shortcuts
            .iter()
            .find(|(shortcut, _)| shortcut.matches(press))
            .map(|(_, id)| id.as_str())
    }

    /// The command's shortcut as shown in menus and the palette.
    pub fn shortcut_text(&self, id: &str) -> Option<String> {
        // The shortcut on the spec is the one shown; extras come after it.
        let declared = Shortcut::parse(self.spec(id)?.shortcut.as_deref()?).ok()?;
        Some(declared.display(cfg!(target_os = "macos")))
    }

    /// Commands matching a palette query, best first.
    pub fn search(&self, query: &str) -> Vec<&CommandSpec> {
        palette::search(self.registry().specs(), query)
    }

    // ---- Running commands ----

    /// Begins running a command the way a menu item does: straight away if
    /// it needs nothing from the person, otherwise by asking for the
    /// values it needs. `preset` holds arguments already decided.
    pub fn invoke(&mut self, id: &str, preset: &Args) -> Invocation {
        if let Some(reason) = self.unavailable_with(id, preset) {
            self.note(format!("{}: {reason}", self.title(id)));
            return Invocation::Unavailable(reason);
        }
        let spec = self.spec(id).expect("availability was checked").clone();
        let preset = &self.with_view_args(&spec, preset);
        let asked: Vec<_> = menu::dialog_params(&spec)
            .into_iter()
            .filter(|p| preset.get(&p.name).is_none())
            .collect();
        match asked.as_slice() {
            [] => {
                self.start(id, preset);
                Invocation::Started
            }
            [param] if matches!(param.ty, ParamType::Path(_)) => {
                let ParamType::Path(mode) = param.ty else {
                    unreachable!("guarded above");
                };
                Invocation::PickPath {
                    request: self.path_request(&spec, mode, preset),
                    command: spec.id.clone(),
                    param: param.name.clone(),
                    args: preset.clone(),
                }
            }
            _ => {
                let initial = self.with_suggestions(&spec, preset);
                Invocation::Dialog(Box::new(Dialog::new(
                    &spec,
                    &self.session.workspace,
                    &initial,
                    self.settings.remembered.get(id),
                )))
            }
        }
    }

    /// Describes the file dialog for a path parameter of `spec`.
    pub fn path_request(&self, spec: &CommandSpec, mode: PathMode, args: &Args) -> PathRequest {
        let opens_image =
            mode == PathMode::Open && spec.effects.image == calipix_command::ImageEffect::Creates;
        let mut filters = Vec::new();
        if opens_image {
            let formats = calipix_io::formats();
            let extensions = |f: &&dyn calipix_io::Format| -> Vec<String> {
                f.extensions().iter().map(|e| e.to_string()).collect()
            };
            filters.push((
                "Images".to_string(),
                formats.iter().flat_map(extensions).collect(),
            ));
            filters.extend(
                formats
                    .iter()
                    .map(|f| (f.name().to_string(), extensions(f))),
            );
        }
        // When saving, suggest the name of the image being saved.
        let image = spec
            .inputs
            .iter()
            .filter(|p| p.ty == ParamType::Image)
            .find_map(|p| match args.get(&p.name) {
                Some(Value::Image(id)) => self.session.workspace.get(*id).ok(),
                _ => self.active(),
            });
        PathRequest {
            title: spec.title.clone(),
            mode,
            directory: self.settings.last_dir.clone(),
            file_name: image
                .filter(|_| mode == PathMode::Save)
                .map(|dataset| dataset.image.meta().name.clone()),
            filters,
        }
    }

    /// Runs a command with the path the person picked for `param`.
    pub fn picked(&mut self, command: &str, param: &str, args: &Args, path: PathBuf) {
        self.remember_dir(&path);
        self.start(command, &args.clone().with(param, path));
    }

    /// Notes the folder of a chosen path as the place to browse from next.
    pub fn remember_dir(&mut self, path: &Path) {
        if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
            self.settings.last_dir = Some(dir.to_path_buf());
            self.settings_dirty = true;
        }
    }

    /// Runs the command of a dialog with the values entered, remembering
    /// them for next time. Returns false, doing nothing, if they are not
    /// all valid.
    pub fn accept(&mut self, dialog: &Dialog) -> bool {
        let Some(args) = dialog.args(&self.session.workspace) else {
            return false;
        };
        self.settings.remembered.insert(
            dialog.spec.id.clone(),
            dialog.remember(&self.session.workspace),
        );
        self.settings_dirty = true;
        self.start(&dialog.spec.id, &args);
        true
    }

    /// Starts a command with the given arguments. Application commands
    /// take effect at once; the rest run on a worker thread against a copy
    /// of the session, which replaces the session if they succeed. See
    /// [`poll`](Self::poll).
    pub fn start(&mut self, id: &str, args: &Args) {
        if let Some(reason) = self.unavailable_with(id, args) {
            self.note(format!("{}: {reason}", self.title(id)));
            return;
        }
        let spec = self.spec(id).expect("availability was checked").clone();
        if self.app_actions.contains_key(id) {
            if let Err(e) = self.executor.run(&mut self.session, id, args) {
                self.fail(format!("{}: {e}", spec.title));
            }
            self.apply_actions();
            return;
        }
        let spawned = Task::spawn(
            self.executor.clone(),
            self.session.clone(),
            &spec,
            args.clone(),
            self.waker.clone(),
        );
        match spawned {
            Ok(task) => self.task = Some(task),
            Err(e) => self.fail(format!(
                "{}: could not start a worker thread: {e}",
                spec.title
            )),
        }
    }

    /// Takes the result of the running command if it has finished, and
    /// starts the next waiting one. Call once per frame.
    pub fn poll(&mut self) {
        if let Some(outcome) = self.task.as_ref().and_then(Task::poll) {
            self.finish(outcome);
        }
        self.start_pending();
    }

    /// Blocks until nothing is running or waiting.
    pub fn wait(&mut self) {
        loop {
            self.start_pending();
            let Some(task) = &self.task else { return };
            let outcome = task.wait();
            self.finish(outcome);
        }
    }

    /// Runs a command to completion. For tests and start-up; the interface
    /// uses [`start`](Self::start) so that it stays responsive.
    pub fn run(&mut self, id: &str, args: &Args) {
        self.wait();
        self.start(id, args);
        self.wait();
    }

    /// The running command, for the progress display.
    pub fn running(&self) -> Option<Running> {
        self.task.as_ref().map(Task::running)
    }

    /// Abandons the running command. It is asked to stop, and whatever it
    /// returns is thrown away, so the session is as if it never ran.
    pub fn cancel(&mut self) {
        if let Some(task) = self.task.take() {
            task.cancel();
            self.pending.clear();
            self.note(format!("{} cancelled", task.spec.title));
        }
    }

    fn start_pending(&mut self) {
        while self.task.is_none() {
            let Some((id, args)) = self.pending.pop_front() else {
                return;
            };
            self.start(&id, &args);
        }
    }

    fn finish(&mut self, outcome: Result<Finished, String>) {
        let Some(task) = self.task.take() else { return };
        // Whatever became of the command, a dragged range it was to make
        // permanent is no longer pending.
        if self.gesture.as_ref().is_some_and(|g| g.committing) {
            self.gesture = None;
        }
        let title = &task.spec.title;
        let finished = match outcome {
            Ok(finished) => finished,
            Err(problem) => return self.fail(format!("{title}: {problem}")),
        };
        let outputs = match finished.result {
            Ok(outputs) => outputs,
            Err(Error::Cancelled) => return self.note(format!("{title} cancelled")),
            Err(e) => return self.fail(format!("{title}: {e}")),
        };

        // Commit. If the person chose another tab during the run and the
        // command did not itself choose an image, their choice stands.
        let chosen = self.session.workspace.active();
        let before = std::mem::replace(&mut self.session, finished.session).workspace;
        let display_only = DISPLAY_COMMANDS.contains(&task.spec.id.as_str());
        self.note_changes(&before, display_only);
        self.note_auto_contrast(&task);
        if chosen != task.base_active
            && self.session.workspace.active() == task.base_active
            && let Some(id) = chosen
        {
            self.set_active(id);
        }

        for param in &task.spec.inputs {
            if let (ParamType::Path(PathMode::Open | PathMode::Save), Some(Value::Path(path))) =
                (&param.ty, task.args.get(&param.name))
            {
                self.settings.add_recent(path);
                self.settings_dirty = true;
                self.rebuild_menu();
            }
        }
        match describe_outputs(&outputs) {
            Some(results) => {
                self.log_line(LogLevel::Info, format!("{title}: {results}"));
                self.queue.push(UiAction::ShowPanel(Panel::Log));
                self.apply_actions();
                self.note(format!("{title} finished; its results are in the Log"));
            }
            None => self.note(format!("{title} finished")),
        }
    }

    // ---- Files ----

    /// Opens an image file and waits for it.
    pub fn open(&mut self, path: PathBuf) {
        self.run("file.open", &Args::new().with("path", path));
    }

    /// Opens image files one after another without blocking.
    pub fn open_files(&mut self, paths: impl IntoIterator<Item = PathBuf>) {
        for path in paths {
            self.pending
                .push_back(("file.open".to_string(), Args::new().with("path", path)));
        }
        self.start_pending();
    }

    fn rebuild_menu(&mut self) {
        self.menu = menu::build(self.executor.registry(), &self.settings.recent);
    }

    // ---- Looking at images ----

    /// How an open image is being looked at.
    pub fn view(&self, id: DatasetId) -> ImageViewState {
        self.views.get(&id).cloned().unwrap_or_default()
    }

    /// The plane of an image on screen: one index per axis beyond X and Y.
    pub fn position(&self, id: DatasetId) -> Vec<usize> {
        match self.session.workspace.image(id) {
            Ok(image) => self.view(id).position_in(image.meta()),
            Err(_) => Vec::new(),
        }
    }

    /// Moves an image's view to another plane. Indices are kept within
    /// the image.
    pub fn set_position(&mut self, id: DatasetId, position: &[usize]) {
        let Ok(image) = self.session.workspace.image(id) else {
            return;
        };
        let wanted = ImageViewState {
            position: position.to_vec(),
            ..ImageViewState::default()
        }
        .position_in(image.meta());
        self.views.entry(id).or_default().position = wanted;
    }

    pub fn channel_visible(&self, id: DatasetId, channel: usize) -> bool {
        self.views
            .get(&id)
            .is_none_or(|view| view.is_visible(channel))
    }

    /// Shows or hides a channel of a composite. This is not a change to
    /// the image, so it is not recorded or undone.
    pub fn set_channel_visible(&mut self, id: DatasetId, channel: usize, visible: bool) {
        if self.session.workspace.get(id).is_err() {
            return;
        }
        let list = &mut self.views.entry(id).or_default().visible;
        if list.len() <= channel {
            list.resize(channel + 1, true);
        }
        list[channel] = visible;
    }

    /// A number that changes whenever the samples of an open image may
    /// have changed. Changes to how the image is displayed leave it alone,
    /// which is how the viewer and the histograms know what they can keep.
    pub fn samples_version(&self, id: DatasetId) -> u64 {
        self.samples.get(&id).copied().unwrap_or(0)
    }

    /// The image to draw for a dataset: the open image, or while a display
    /// range is being dragged, that image with the dragged range.
    pub fn displayed(&self, id: DatasetId) -> Option<Image> {
        let image = self.session.workspace.image(id).ok()?;
        match &self.gesture {
            Some(gesture) if gesture.id == id && gesture.base.ptr_eq(image) => {
                Some(gesture.shown.clone())
            }
            _ => Some(image.clone()),
        }
    }

    /// The arguments that point a display command at what is on screen:
    /// the image, and where the command has such parameters, the channel
    /// (every channel when they are shown together), slice and frame in
    /// view and how often Auto Contrast has been pressed.
    pub fn display_args(&self, command: &str, input: DatasetId) -> Args {
        let mut args = Args::new().with("input", input);
        let (Some(spec), Ok(image)) = (self.spec(command), self.session.workspace.image(input))
        else {
            return args;
        };
        let meta = image.meta();
        let position = self.position(input);
        let (slice, frame) = view::slice_and_frame(meta, &position);
        for param in &spec.inputs {
            let value = match param.name.as_str() {
                "slice" => slice,
                "frame" => frame,
                "channel" if view::shown_together(meta) => 0,
                "channel" => view::channel_at(meta, &position) as i64 + 1,
                "press" => match self.views.get(&input).and_then(|v| v.auto.as_ref()) {
                    Some((last, press)) if last.ptr_eq(image) => i64::from(*press) + 1,
                    _ => 1,
                },
                _ => continue,
            };
            args.insert(param.name.clone(), value);
        }
        args
    }

    /// `preset` with what [`display_args`](Self::display_args) knows
    /// filled in, for a display command chosen from a menu.
    fn with_view_args(&self, spec: &CommandSpec, preset: &Args) -> Args {
        if !DISPLAY_COMMANDS.contains(&spec.id.as_str()) {
            return preset.clone();
        }
        let input = match preset.get("input") {
            Some(Value::Image(id)) => Some(*id),
            _ => self.session.workspace.active(),
        };
        let Some(input) = input else {
            return preset.clone();
        };
        let mut args = preset.clone();
        for (name, value) in self.display_args(&spec.id, input).iter() {
            if name != "input" && args.get(name).is_none() {
                args.insert(name, value.clone());
            }
        }
        args
    }

    /// `preset` with starting values for a dialog that are better than a
    /// command's defaults: Set Display Range begins at the range in use.
    fn with_suggestions(&mut self, spec: &CommandSpec, preset: &Args) -> Args {
        let mut args = preset.clone();
        if spec.id != "view.set_display_range" {
            return args;
        }
        let input = match preset.get("input") {
            Some(Value::Image(id)) => Some(*id),
            _ => self.session.workspace.active(),
        };
        let channel = match preset.get("channel") {
            Some(Value::Int(c)) if *c >= 1 => *c as usize - 1,
            _ => 0,
        };
        if let Some((min, max)) = input.and_then(|id| self.shown_range(id, channel)) {
            for (name, value) in [("min", min), ("max", max)] {
                if args.get(name).is_none() {
                    args.insert(name, value);
                }
            }
        }
        args
    }

    /// The display range a channel is shown with on the plane in view:
    /// the one being dragged, the one the image stores, or the default
    /// for its pixel type. `None` while the default is still being found
    /// (it is the smallest and largest sample of the plane).
    pub fn shown_range(&mut self, id: DatasetId, channel: usize) -> Option<(f64, f64)> {
        let image = self.displayed(id)?;
        let meta = image.meta();
        let rgb = meta.color == ColorModel::Rgb;
        let is_u8 = meta.dtype() == calipix_core::DType::U8;
        let stored = meta.channels.get(channel).and_then(|c| c.display_range);
        match stored {
            // As in ImageJ, (0, 0) on anything but 8-bit means the default.
            Some(range) if is_u8 || rgb || range != (0.0, 0.0) => return Some(range),
            _ if rgb => return Some(meta.dtype().integer_range().unwrap_or((0.0, 1.0))),
            _ if is_u8 => return Some((0.0, 255.0)),
            _ => {}
        }
        let position = view::with_channel(meta, &self.position(id), channel);
        let plane = meta.plane_index(&position).ok()?;
        let state = self.histogram(id, plane, false);
        let histogram = &state.ready()?.histogram;
        Some((histogram.min, histogram.max))
    }

    /// Runs a display command on what is on screen, with `more` arguments
    /// taking precedence. This is what the Display panel's controls do.
    pub fn start_display(&mut self, command: &str, input: DatasetId, more: &Args) {
        let mut args = self.display_args(command, input);
        for (name, value) in more.iter() {
            args.insert(name, value.clone());
        }
        self.start(command, &args);
    }

    /// The histogram of one plane of an open image, counted on first use
    /// and kept until the image's samples change. `gray` asks for the grey
    /// values of an RGB image rather than one of its channels.
    pub fn histogram(&mut self, id: DatasetId, plane: usize, gray: bool) -> HistogramState {
        let Ok(image) = self.session.workspace.image(id) else {
            return HistogramState::Failed("This image is no longer open".to_string());
        };
        let key = HistogramKey {
            image: id,
            plane,
            gray: gray && image.meta().color == ColorModel::Rgb,
        };
        let version = self.samples.get(&id).copied().unwrap_or(0);
        self.histograms.get(key, image, version)
    }

    /// How many histograms have been counted, to check that they are kept.
    pub fn histograms_computed(&self) -> u64 {
        self.histograms.computed()
    }

    /// Blocks until no histogram is being counted. For tests.
    pub fn wait_for_histograms(&mut self) {
        self.histograms.wait();
    }

    // ---- Dragging a display range ----

    /// Shows an image with another display range without recording
    /// anything: the live half of dragging a range control. `channel` is
    /// the channel to change, or `None` for all of them. Call
    /// [`commit_display_range`](Self::commit_display_range) when the drag
    /// ends.
    pub fn preview_display_range(
        &mut self,
        id: DatasetId,
        channel: Option<usize>,
        range: (f64, f64),
    ) {
        let Ok(base) = self.session.workspace.image(id).cloned() else {
            return;
        };
        if !range.0.is_finite() || !range.1.is_finite() {
            return;
        }
        let mut meta = base.meta().clone();
        let count = meta.size(&calipix_core::AxisKind::Channel);
        if meta.channels.len() != count {
            meta.channels = vec![Default::default(); count];
        }
        for (index, entry) in meta.channels.iter_mut().enumerate() {
            if channel.is_none_or(|c| c == index) {
                entry.display_range = Some(range);
            }
        }
        let Ok(shown) = base.with_meta(meta) else {
            return;
        };
        self.gesture = Some(RangeGesture {
            id,
            channel,
            base,
            shown,
            range,
            committing: false,
        });
    }

    /// The range being dragged, with its image and channel.
    pub fn previewed_range(&self) -> Option<(DatasetId, Option<usize>, (f64, f64))> {
        let gesture = self.gesture.as_ref()?;
        Some((gesture.id, gesture.channel, gesture.range))
    }

    /// Ends a drag by running Set Display Range once with where it ended,
    /// so that the whole gesture is one step in the history. Returns false
    /// if there was nothing to make permanent.
    pub fn commit_display_range(&mut self) -> bool {
        let Some(gesture) = &mut self.gesture else {
            return false;
        };
        if gesture.committing {
            return false;
        }
        let (id, channel, range) = (gesture.id, gesture.channel, gesture.range);
        let current = self.session.workspace.image(id).ok();
        // The image changed under the drag, or it ended where it began.
        let stale = current.is_none_or(|image| !image.ptr_eq(&gesture.base));
        let unchanged = gesture.shown.meta() == gesture.base.meta();
        let args = Args::new()
            .with("input", id)
            .with("min", range.0)
            .with("max", range.1)
            .with("channel", channel.map_or(0, |c| c as i64 + 1));
        let command = "view.set_display_range";
        if stale || unchanged {
            self.gesture = None;
            return false;
        }
        gesture.committing = true;
        // If something else is running this says so and starts nothing.
        self.start(command, &args);
        let started = self
            .task
            .as_ref()
            .is_some_and(|task| task.spec.id == command);
        if !started {
            self.gesture = None;
        }
        started
    }

    /// Abandons a drag, putting the display back as it was.
    pub fn cancel_display_range(&mut self) {
        if self.gesture.as_ref().is_some_and(|g| !g.committing) {
            self.gesture = None;
        }
    }

    /// Brings the per-image records in line with the images now open,
    /// given the workspace as it was before a change. `display_only` says
    /// that the change cannot have touched a sample.
    fn note_changes(&mut self, before: &Workspace, display_only: bool) {
        let workspace = &self.session.workspace;
        for dataset in workspace.datasets() {
            let unchanged = match before.image(dataset.id) {
                Ok(old) => old.ptr_eq(&dataset.image) || display_only,
                Err(_) => false,
            };
            if !unchanged || !self.samples.contains_key(&dataset.id) {
                self.next_version += 1;
                self.samples.insert(dataset.id, self.next_version);
            }
        }
        let open = |id: DatasetId| workspace.get(id).is_ok();
        self.views.retain(|id, _| open(*id));
        self.samples.retain(|id, _| open(*id));
        self.histograms.retain(open);
        if self.gesture.as_ref().is_some_and(|g| !open(g.id)) {
            self.gesture = None;
        }
    }

    /// Remembers what Auto Contrast just produced, so that pressing it
    /// again goes a step further.
    fn note_auto_contrast(&mut self, task: &Task) {
        if task.spec.id != "view.auto_contrast" {
            return;
        }
        let id = match task.args.get("input") {
            Some(Value::Image(id)) => Some(*id),
            _ => task.base_active,
        };
        let Some((id, image)) =
            id.and_then(|id| Some((id, self.session.workspace.image(id).ok()?.clone())))
        else {
            return;
        };
        let press = match task.args.get("press") {
            Some(Value::Int(press)) => (*press).clamp(1, i64::from(u32::MAX)) as u32,
            _ => 1,
        };
        // Where ImageJ finds no range it resets and starts counting again.
        let meta = image.meta();
        let position = self.position(id);
        let channel = match task.args.get("channel") {
            Some(Value::Int(c)) if *c >= 1 => *c as usize - 1,
            _ => 0,
        };
        let found = meta
            .plane_index(&view::with_channel(meta, &position, channel))
            .ok()
            .and_then(|plane| {
                let state = self.histogram(id, plane, true);
                Some(calipix_render::auto_range(&state.ready()?.histogram, press).is_some())
            })
            .unwrap_or(true);
        self.views.entry(id).or_default().auto = found.then_some((image, press));
    }

    // ---- History ----

    /// The History panel's lines, oldest first.
    pub fn history_rows(&self) -> Vec<HistoryRow> {
        let history = self.session.history();
        let entries = history.entries();
        let applied = history.applied().len();
        entries
            .iter()
            .enumerate()
            .map(|(index, entry)| HistoryRow {
                title: entry.title.clone(),
                detail: self.describe_args(entries, index),
                duration: entry.duration,
                undone: index >= applied,
                current: index + 1 == applied,
                changed: entry.changed_workspace(),
            })
            .collect()
    }

    /// The arguments of a history entry as `name value` pairs. Images are
    /// called what they were called when the command ran, whether or not
    /// they are still open.
    fn describe_args(&self, entries: &[HistoryEntry], index: usize) -> String {
        let name_then = |id: DatasetId| {
            // The step itself knows the image if it changed it; otherwise
            // the nearest earlier step that touched it does.
            let own = entries[index].changes.iter().find(|c| c.id == id);
            let named = own
                .and_then(|c| c.before.as_ref().or(c.after.as_ref()))
                .or_else(|| {
                    entries[..index].iter().rev().find_map(|entry| {
                        let change = entry.changes.iter().find(|c| c.id == id)?;
                        change.after.as_ref().or(change.before.as_ref())
                    })
                });
            match named {
                Some(dataset) => dataset.image.meta().name.clone(),
                None => self.session.workspace.get(id).map_or_else(
                    |_| format!("image {id}"),
                    |dataset| dataset.image.meta().name.clone(),
                ),
            }
        };
        let entry = &entries[index];
        let spec = self.spec(&entry.command);
        // In the order the command declares its parameters, leaving out
        // those left at their defaults: what is listed is what was chosen.
        let mut names: Vec<&str> = match spec {
            Some(spec) => spec.inputs.iter().map(|p| p.name.as_str()).collect(),
            None => Vec::new(),
        };
        for (name, _) in entry.args.iter() {
            if !names.contains(&name) {
                names.push(name);
            }
        }
        let images = entry
            .args
            .iter()
            .filter(|(_, value)| matches!(value, Value::Image(_)))
            .count();
        names
            .into_iter()
            .filter_map(|name| {
                let value = entry.args.get(name)?;
                let default = spec
                    .and_then(|s| s.param(name))
                    .and_then(|p| p.default.as_ref());
                if default == Some(value) {
                    return None;
                }
                Some(match value {
                    // A command's one image needs no label.
                    Value::Image(id) if images == 1 => name_then(*id),
                    Value::Image(id) => format!("{} {}", name.replace('_', " "), name_then(*id)),
                    other => format!("{} {}", name.replace('_', " "), other.to_text()),
                })
            })
            .collect::<Vec<_>>()
            .join(", ")
    }

    /// Undoes or redoes until step `index` (from 0) is the last one in
    /// effect, as clicking it in the History panel does. Undo and redo
    /// move between steps that changed an image, so the position ends on
    /// the nearest such step.
    pub fn go_to_history(&mut self, index: usize) {
        let target = index + 1;
        let applied = |state: &AppState| state.session.history().applied().len();
        if let Some(task) = &self.task {
            self.note(format!("History: Wait for {} to finish", task.spec.title));
            return;
        }
        while applied(self) > target && self.can_undo() {
            // Undoing the next step back would pass the one clicked.
            let entries = self.session.history().applied();
            let next = entries.iter().rposition(HistoryEntry::changed_workspace);
            if next.is_none_or(|next| next < target) {
                break;
            }
            self.start("edit.undo", &Args::new());
        }
        while applied(self) < target && self.can_redo() {
            // Likewise, redoing must not pass the step clicked.
            let from = applied(self);
            let undone = &self.session.history().entries()[from..];
            let next = undone.iter().position(HistoryEntry::changed_workspace);
            if next.is_none_or(|next| from + next >= target) {
                break;
            }
            self.start("edit.redo", &Args::new());
        }
    }

    // ---- Undo ----

    pub fn can_undo(&self) -> bool {
        self.session.history().can_undo()
    }

    pub fn can_redo(&self) -> bool {
        self.session.history().can_redo()
    }

    pub fn undo(&mut self) {
        self.run("edit.undo", &Args::new());
    }

    pub fn redo(&mut self) {
        self.run("edit.redo", &Args::new());
    }

    // ---- Interface state ----

    pub fn panel_open(&self, panel: Panel) -> bool {
        !self.closed_panels.contains(&panel)
    }

    /// Records that a panel was shown or hidden by other means than its
    /// command, such as the close button on its tab.
    pub fn set_panel_open(&mut self, panel: Panel, open: bool) {
        if open {
            self.closed_panels.remove(&panel);
        } else {
            self.closed_panels.insert(panel);
        }
    }

    /// Applies the actions queued by application commands to this state,
    /// and keeps them for the interface to act on too.
    fn apply_actions(&mut self) {
        for action in self.queue.take() {
            match action {
                UiAction::Undo | UiAction::Redo => {
                    let before = self.session.workspace.clone();
                    let was = self.session.history().applied().len();
                    let result = if action == UiAction::Undo {
                        self.executor.undo(&mut self.session)
                    } else {
                        self.executor.redo(&mut self.session)
                    };
                    let (done, name) = match action {
                        UiAction::Undo => ("Undid", "Undo"),
                        _ => ("Redid", "Redo"),
                    };
                    match result {
                        Ok(title) => self.note(format!("{done} {title}")),
                        Err(e) => self.fail(format!("{name}: {e}")),
                    }
                    // The steps crossed decide whether samples changed.
                    let history = self.session.history();
                    let now = history.applied().len();
                    let crossed = &history.entries()[was.min(now)..was.max(now)];
                    let display_only = crossed
                        .iter()
                        .filter(|entry| entry.changed_workspace())
                        .all(|entry| DISPLAY_COMMANDS.contains(&entry.command.as_str()));
                    self.note_changes(&before, display_only);
                }
                UiAction::TogglePanel(panel) => {
                    self.set_panel_open(panel, !self.panel_open(panel));
                }
                UiAction::ShowPanel(panel) => self.set_panel_open(panel, true),
                UiAction::ResetLayout => {
                    self.closed_panels.clear();
                    self.note("Layout reset");
                }
                UiAction::SetTheme(theme) => {
                    self.settings.theme = theme;
                    self.settings_dirty = true;
                }
                UiAction::ClearRecent => {
                    self.settings.recent.clear();
                    self.settings_dirty = true;
                    self.rebuild_menu();
                    self.note("Recent files cleared");
                }
                UiAction::Zoom(_)
                | UiAction::Step(_)
                | UiAction::TileImages
                | UiAction::OpenPalette
                | UiAction::About => {}
            }
            self.ui_actions.push(action);
        }
    }

    /// The actions applied since the last call, for the interface to carry
    /// out its side of: zooming the viewer, rearranging panels and so on.
    pub fn take_ui_actions(&mut self) -> Vec<UiAction> {
        std::mem::take(&mut self.ui_actions)
    }

    // ---- Settings ----

    /// Stores the workspace layout to be saved with the settings.
    pub fn set_saved_layout(&mut self, layout: Option<SavedLayout>) {
        if self.settings.layout != layout {
            self.settings.layout = layout;
            self.settings_dirty = true;
        }
    }

    /// Writes the settings to disk if they changed since the last call.
    pub fn save_settings(&mut self) {
        if !std::mem::take(&mut self.settings_dirty) {
            return;
        }
        let Some(path) = self.settings_path.clone() else {
            return;
        };
        if let Err(e) = self.settings.save(&path) {
            self.fail(format!(
                "Could not save settings to {}: {e}",
                path.display()
            ));
        }
    }
}

/// The values a command returned, as text for the Log. Images are left
/// out: they show up as tabs.
fn describe_outputs(outputs: &Outputs) -> Option<String> {
    let parts: Vec<String> = outputs
        .iter()
        .filter(|(_, value)| !matches!(value, Value::Image(_)))
        .map(|(name, value)| format!("{} {}", name.replace('_', " "), value.to_text()))
        .collect();
    (!parts.is_empty()).then(|| parts.join(", "))
}

/// True if the image has planes to step through besides its channels.
fn has_planes(image: &Image) -> bool {
    let meta = image.meta();
    meta.axes()
        .iter()
        .zip(meta.shape())
        .skip(2)
        .any(|(axis, &size)| axis.kind != calipix_core::AxisKind::Channel && size > 1)
}
