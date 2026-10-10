//! Application state behind the desktop interface. It has no dependency on
//! the GUI toolkit, so everything the interface can do is testable here.

use std::path::PathBuf;
use std::sync::Arc;

use imager_command::{
    Args, Dataset, EffectsPolicy, Executor, MenuItem, ParamType, Session, menu_tree,
};

/// Outcome of the most recent action, shown in the status bar.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Status {
    #[default]
    Idle,
    Done(String),
    Failed(String),
}

pub struct AppState {
    executor: Executor,
    session: Session,
    menu: Vec<MenuItem>,
    pub status: Status,
}

impl Default for AppState {
    fn default() -> AppState {
        AppState::new()
    }
}

impl AppState {
    pub fn new() -> AppState {
        let registry = Arc::new(imager_commands::standard_registry());
        let menu = menu_tree(&registry);
        AppState {
            // The person at the keyboard is trusted with their own files.
            executor: Executor::new(registry, EffectsPolicy::allow_all()),
            session: Session::new(),
            menu,
            status: Status::Idle,
        }
    }

    pub fn session(&self) -> &Session {
        &self.session
    }

    pub fn menu(&self) -> &[MenuItem] {
        &self.menu
    }

    /// The image commands act on and the viewer shows.
    pub fn active(&self) -> Option<&Dataset> {
        let id = self.session.workspace.active()?;
        self.session.workspace.get(id).ok()
    }

    /// Why a command cannot be run right now from a menu, or `None` if it
    /// can. Commands that need values from the user are unavailable until
    /// parameter dialogs exist.
    pub fn unavailable_reason(&self, id: &str) -> Option<String> {
        let spec = match self.executor.registry().get(id) {
            Ok(command) => command.spec(),
            Err(e) => return Some(e.to_string()),
        };
        let missing = self
            .executor
            .unresolved(spec, &self.session.workspace, &Args::new());
        if missing.is_empty() {
            return None;
        }
        let needs_image = missing
            .iter()
            .any(|name| spec.param(name).is_some_and(|p| p.ty == ParamType::Image));
        if needs_image {
            return Some("Open an image first".to_string());
        }
        let labels: Vec<_> = missing
            .iter()
            .filter_map(|name| spec.param(name))
            .map(|p| p.label.as_str())
            .collect();
        Some(format!("Needs: {}", labels.join(", ")))
    }

    /// Runs a command with the given arguments and records the outcome.
    pub fn run(&mut self, id: &str, args: &Args) {
        let title = self
            .executor
            .registry()
            .get(id)
            .map(|c| c.spec().title.trim_end_matches("...").to_string())
            .unwrap_or_else(|_| id.to_string());
        self.status = match self.executor.run(&mut self.session, id, args) {
            Ok(_) => Status::Done(title),
            Err(e) => Status::Failed(format!("{title}: {e}")),
        };
    }

    pub fn open(&mut self, path: PathBuf) {
        self.run("file.open", &Args::new().with("path", path));
    }

    pub fn can_undo(&self) -> bool {
        self.session.history().can_undo()
    }

    pub fn can_redo(&self) -> bool {
        self.session.history().can_redo()
    }

    pub fn undo(&mut self) {
        self.status = match self.executor.undo(&mut self.session) {
            Ok(title) => Status::Done(format!("Undid {}", title.trim_end_matches("..."))),
            Err(e) => Status::Failed(e.to_string()),
        };
    }

    pub fn redo(&mut self) {
        self.status = match self.executor.redo(&mut self.session) {
            Ok(title) => Status::Done(format!("Redid {}", title.trim_end_matches("..."))),
            Err(e) => Status::Failed(e.to_string()),
        };
    }
}
