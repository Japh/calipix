//! Application state behind the desktop interface. It has no dependency on
//! the GUI toolkit, so everything the interface can do is testable here.

pub mod action;
pub mod dialog;
pub mod menu;
pub mod palette;
mod panel;
pub mod settings;
pub mod shortcut;
mod task;

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use imager_command::{
    Args, CommandSpec, Dataset, DatasetId, EffectsPolicy, Error, Executor, Outputs, ParamType,
    PathMode, Registry, Session, Value,
};

pub use action::{ActionQueue, Theme, UiAction, Zoom};
pub use dialog::{Dialog, Draft, Field};
pub use menu::{MenuCommand, MenuEntry};
pub use panel::{Dock, Panel};
pub use settings::{SavedLayout, Settings};
pub use shortcut::{KeyPress, Shortcut};
pub use task::{Running, Waker};

use task::{Finished, Task};

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
            imager_commands::standard_registry(),
            Settings::default(),
            None,
        )
    }

    /// The application as a person starts it: settings are read from
    /// `settings_path` and written back there as they change.
    pub fn with_settings_file(settings_path: PathBuf) -> AppState {
        let (settings, problem) = Settings::load(&settings_path);
        let mut state = AppState::build(
            imager_commands::standard_registry(),
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
            Some(UiAction::Zoom(_)) if self.active().is_none() => {
                Some("Open an image first".to_string())
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
            _ => Invocation::Dialog(Box::new(Dialog::new(
                &spec,
                &self.session.workspace,
                preset,
                self.settings.remembered.get(id),
            ))),
        }
    }

    /// Describes the file dialog for a path parameter of `spec`.
    pub fn path_request(&self, spec: &CommandSpec, mode: PathMode, args: &Args) -> PathRequest {
        let opens_image =
            mode == PathMode::Open && spec.effects.image == imager_command::ImageEffect::Creates;
        let mut filters = Vec::new();
        if opens_image {
            let formats = imager_io::formats();
            let extensions = |f: &&dyn imager_io::Format| -> Vec<String> {
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
        self.session = finished.session;
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
                UiAction::Undo => match self.executor.undo(&mut self.session) {
                    Ok(title) => self.note(format!("Undid {title}")),
                    Err(e) => self.fail(format!("Undo: {e}")),
                },
                UiAction::Redo => match self.executor.redo(&mut self.session) {
                    Ok(title) => self.note(format!("Redid {title}")),
                    Err(e) => self.fail(format!("Redo: {e}")),
                },
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
                UiAction::Zoom(_) | UiAction::OpenPalette | UiAction::About => {}
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
