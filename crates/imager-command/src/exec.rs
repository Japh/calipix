use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use crate::{
    Args, CommandCtx, CommandSpec, Error, History, HistoryEntry, Outputs, ParamType, Registry,
    Result, Value, Workspace,
};

/// Receives progress reports from a running command.
pub trait Progress: Send + Sync {
    /// `fraction` runs from 0 to 1.
    fn report(&self, fraction: f64, message: &str);
}

struct NoProgress;

impl Progress for NoProgress {
    fn report(&self, _fraction: f64, _message: &str) {}
}

/// Shared flag for asking a running command to stop.
#[derive(Clone, Debug, Default)]
pub struct CancelToken(Arc<AtomicBool>);

impl CancelToken {
    pub fn new() -> CancelToken {
        CancelToken::default()
    }

    pub fn cancel(&self) {
        self.0.store(true, Ordering::Relaxed);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}

/// Per-run hooks supplied by the frontend.
pub struct RunOptions<'a> {
    pub progress: &'a dyn Progress,
    pub cancel: CancelToken,
}

impl Default for RunOptions<'_> {
    fn default() -> Self {
        RunOptions {
            progress: &NoProgress,
            cancel: CancelToken::new(),
        }
    }
}

/// Which side effects commands are allowed to have. A frontend acting for
/// an untrusted caller (such as an agent) starts from
/// [`read_only`](Self::read_only) and widens it on user approval.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EffectsPolicy {
    pub allow_fs_read: bool,
    pub allow_fs_write: bool,
    pub allow_network: bool,
}

impl EffectsPolicy {
    pub fn allow_all() -> EffectsPolicy {
        EffectsPolicy {
            allow_fs_read: true,
            allow_fs_write: true,
            allow_network: true,
        }
    }

    /// Reading files is allowed; writing files and network access are not.
    pub fn read_only() -> EffectsPolicy {
        EffectsPolicy {
            allow_fs_read: true,
            allow_fs_write: false,
            allow_network: false,
        }
    }

    fn check(&self, spec: &CommandSpec) -> Result<()> {
        let denied = |what: &str| Err(Error::Denied(format!("`{}` {what}", spec.id)));
        if spec.effects.reads_fs && !self.allow_fs_read {
            return denied("reads files");
        }
        if spec.effects.writes_fs && !self.allow_fs_write {
            return denied("writes files");
        }
        if spec.effects.network && !self.allow_network {
            return denied("uses the network");
        }
        Ok(())
    }
}

/// The state commands operate on: open images and the history of what was
/// done to them.
#[derive(Clone, Debug, Default)]
pub struct Session {
    pub workspace: Workspace,
    history: History,
}

impl Session {
    pub fn new() -> Session {
        Session::default()
    }

    pub fn history(&self) -> &History {
        &self.history
    }
}

/// Runs commands from a [`Registry`] against a [`Session`].
#[derive(Clone)]
pub struct Executor {
    registry: Arc<Registry>,
    pub policy: EffectsPolicy,
}

impl Executor {
    pub fn new(registry: Arc<Registry>, policy: EffectsPolicy) -> Executor {
        Executor { registry, policy }
    }

    pub fn registry(&self) -> &Registry {
        &self.registry
    }

    /// Fills in and validates the arguments for a command. Each parameter
    /// takes, in order of preference: the explicit argument, the active
    /// image (for image parameters), then the spec's default.
    pub fn resolve(
        &self,
        spec: &CommandSpec,
        workspace: &Workspace,
        explicit: &Args,
    ) -> Result<Args> {
        for (name, _) in explicit.iter() {
            if spec.param(name).is_none() {
                return Err(Error::UnknownArg {
                    command: spec.id.clone(),
                    param: name.to_string(),
                });
            }
        }
        let mut resolved = Args::new();
        for param in &spec.inputs {
            let value = explicit
                .get(&param.name)
                .cloned()
                .or_else(|| {
                    (param.ty == ParamType::Image)
                        .then(|| workspace.active().map(Value::Image))
                        .flatten()
                })
                .or_else(|| param.default.clone());
            match value {
                Some(value) => {
                    let value = param.check(value)?;
                    if let Value::Image(id) = value {
                        workspace.get(id)?;
                    }
                    resolved.insert(param.name.clone(), value);
                }
                None if param.required => {
                    return Err(Error::MissingArg {
                        command: spec.id.clone(),
                        param: param.name.clone(),
                    });
                }
                None => {}
            }
        }
        Ok(resolved)
    }

    /// Names of required parameters that [`resolve`](Self::resolve) could
    /// not fill from `explicit`, the active image or defaults. An
    /// interactive frontend asks the user for these.
    pub fn unresolved(
        &self,
        spec: &CommandSpec,
        workspace: &Workspace,
        explicit: &Args,
    ) -> Vec<String> {
        spec.inputs
            .iter()
            .filter(|p| {
                p.required
                    && explicit.get(&p.name).is_none()
                    && p.default.is_none()
                    && !(p.ty == ParamType::Image && workspace.active().is_some())
            })
            .map(|p| p.name.clone())
            .collect()
    }

    pub fn run(&self, session: &mut Session, id: &str, args: &Args) -> Result<Outputs> {
        self.run_with(session, id, args, &RunOptions::default())
    }

    /// Runs a command. Its changes to the workspace are committed only if
    /// it succeeds; on error or cancellation the session is left untouched.
    pub fn run_with(
        &self,
        session: &mut Session,
        id: &str,
        args: &Args,
        options: &RunOptions<'_>,
    ) -> Result<Outputs> {
        let command = self.registry.get(id)?;
        let spec = command.spec();
        let args = self.resolve(spec, &session.workspace, args)?;
        self.policy.check(spec)?;

        let mut scratch = session.workspace.clone();
        let started = Instant::now();
        let outputs = {
            let mut ctx = CommandCtx::new(&mut scratch, options.progress, &options.cancel);
            command.run(&mut ctx, &args)?
        };
        check_outputs(spec, &outputs)?;

        if spec.flags.recordable {
            session.history.push(HistoryEntry::new(
                spec.id.clone(),
                spec.title.clone(),
                args,
                outputs.clone(),
                &session.workspace,
                &scratch,
                started.elapsed(),
            ));
        }
        session.workspace = scratch;
        Ok(outputs)
    }

    /// Reverts the most recent change to the workspace and returns the
    /// title of the command that made it.
    pub fn undo(&self, session: &mut Session) -> Result<String> {
        let entry = session.history.undo(&mut session.workspace)?;
        Ok(entry.title.clone())
    }

    /// Re-applies the most recently undone change.
    pub fn redo(&self, session: &mut Session) -> Result<String> {
        let entry = session.history.redo(&mut session.workspace)?;
        Ok(entry.title.clone())
    }
}

/// A command returning values that do not match its declared outputs is a
/// bug in the command; catch it here rather than in every frontend.
fn check_outputs(spec: &CommandSpec, outputs: &Outputs) -> Result<()> {
    let fail = |reason: String| {
        Err(Error::Failed(format!(
            "`{}` returned invalid outputs: {reason}",
            spec.id
        )))
    };
    for (name, value) in outputs.iter() {
        let Some(declared) = spec.outputs.iter().find(|o| o.name == name) else {
            return fail(format!("undeclared output `{name}`"));
        };
        if let Err(reason) = value.clone().coerce(&declared.ty) {
            return fail(format!("output `{name}`: {reason}"));
        }
    }
    Ok(())
}
