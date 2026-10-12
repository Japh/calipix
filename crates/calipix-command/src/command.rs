use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use calipix_core::Image;
use calipix_roi::Roi;
use serde_json::{Map, Value as Json};

use crate::exec::{CancelToken, Progress};
use crate::{
    CommandSpec, DatasetId, Error, ParamSpec, ParamType, PathMode, Result, Value, Workspace,
};

/// Named values: the arguments to a command, or its outputs.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Args(BTreeMap<String, Value>);

/// Values a command returns, keyed by output name.
pub type Outputs = Args;

impl Args {
    pub fn new() -> Args {
        Args::default()
    }

    /// Builder-style insert.
    pub fn with(mut self, name: impl Into<String>, value: impl Into<Value>) -> Args {
        self.insert(name, value);
        self
    }

    pub fn insert(&mut self, name: impl Into<String>, value: impl Into<Value>) {
        self.0.insert(name.into(), value.into());
    }

    pub fn get(&self, name: &str) -> Option<&Value> {
        self.0.get(name)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, &Value)> {
        self.0.iter().map(|(k, v)| (k.as_str(), v))
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn to_json(&self) -> Json {
        Json::Object(
            self.0
                .iter()
                .map(|(k, v)| (k.clone(), v.to_json()))
                .collect::<Map<_, _>>(),
        )
    }
}

/// What a running command can see and do.
pub struct CommandCtx<'a> {
    workspace: &'a mut Workspace,
    progress: &'a dyn Progress,
    cancel: &'a CancelToken,
}

impl<'a> CommandCtx<'a> {
    pub(crate) fn new(
        workspace: &'a mut Workspace,
        progress: &'a dyn Progress,
        cancel: &'a CancelToken,
    ) -> CommandCtx<'a> {
        CommandCtx {
            workspace,
            progress,
            cancel,
        }
    }

    pub fn workspace(&self) -> &Workspace {
        self.workspace
    }

    /// The workspace the command may change. Changes are committed only if
    /// the command returns `Ok`.
    pub fn workspace_mut(&mut self) -> &mut Workspace {
        self.workspace
    }

    pub fn image(&self, id: DatasetId) -> Result<&Image> {
        self.workspace.image(id)
    }

    /// Reports progress as a fraction from 0 to 1 with a short message.
    pub fn progress(&self, fraction: f64, message: &str) {
        self.progress.report(fraction.clamp(0.0, 1.0), message);
    }

    /// Returns [`Error::Cancelled`] if the user asked to stop. Long-running
    /// commands should call this regularly.
    pub fn check_cancelled(&self) -> Result<()> {
        if self.cancel.is_cancelled() {
            Err(Error::Cancelled)
        } else {
            Ok(())
        }
    }
}

/// An operation users can run. See the crate documentation.
pub trait Command: Send + Sync {
    fn spec(&self) -> &CommandSpec;

    /// Runs the command. `args` has been resolved and validated against the
    /// spec: every required parameter is present with the declared type.
    fn run(&self, ctx: &mut CommandCtx<'_>, args: &Args) -> Result<Outputs>;
}

/// A struct of command parameters. Implement it with `#[derive(Params)]`.
pub trait Params: Sized {
    fn specs() -> Vec<ParamSpec>;
    fn from_args(args: &Args) -> Result<Self>;
}

/// A Rust type usable as a field of a [`Params`] struct.
pub trait ParamField: Sized {
    const REQUIRED: bool = true;

    fn param_type() -> ParamType;

    fn from_value(value: &Value) -> Option<Self>;

    /// Extracts the field from an argument that may be absent.
    fn from_arg(value: Option<&Value>, name: &str) -> Result<Self> {
        let value = value.ok_or_else(|| Error::MissingArg {
            command: String::new(),
            param: name.to_string(),
        })?;
        Self::from_value(value).ok_or_else(|| {
            Error::invalid_arg(name, format!("expected {}", Self::param_type().name()))
        })
    }
}

impl ParamField for bool {
    fn param_type() -> ParamType {
        ParamType::Bool
    }

    fn from_value(value: &Value) -> Option<bool> {
        match value {
            Value::Bool(b) => Some(*b),
            _ => None,
        }
    }
}

impl ParamField for i64 {
    fn param_type() -> ParamType {
        ParamType::Int
    }

    fn from_value(value: &Value) -> Option<i64> {
        match value {
            Value::Int(i) => Some(*i),
            _ => None,
        }
    }
}

impl ParamField for f64 {
    fn param_type() -> ParamType {
        ParamType::Float
    }

    fn from_value(value: &Value) -> Option<f64> {
        match value {
            Value::Float(f) => Some(*f),
            Value::Int(i) => Some(*i as f64),
            _ => None,
        }
    }
}

impl ParamField for String {
    fn param_type() -> ParamType {
        ParamType::Str
    }

    fn from_value(value: &Value) -> Option<String> {
        match value {
            Value::Str(s) => Some(s.clone()),
            _ => None,
        }
    }
}

impl ParamField for PathBuf {
    fn param_type() -> ParamType {
        ParamType::Path(PathMode::Open)
    }

    fn from_value(value: &Value) -> Option<PathBuf> {
        match value {
            Value::Path(p) => Some(p.clone()),
            Value::Str(s) => Some(PathBuf::from(s)),
            _ => None,
        }
    }
}

impl ParamField for DatasetId {
    fn param_type() -> ParamType {
        ParamType::Image
    }

    fn from_value(value: &Value) -> Option<DatasetId> {
        match value {
            Value::Image(id) => Some(*id),
            _ => None,
        }
    }
}

impl ParamField for Roi {
    fn param_type() -> ParamType {
        ParamType::Roi
    }

    fn from_value(value: &Value) -> Option<Roi> {
        match value {
            Value::Roi(roi) => Some(roi.clone()),
            _ => None,
        }
    }
}

impl<T: ParamField> ParamField for Option<T> {
    const REQUIRED: bool = false;

    fn param_type() -> ParamType {
        T::param_type()
    }

    fn from_value(value: &Value) -> Option<Option<T>> {
        T::from_value(value).map(Some)
    }

    fn from_arg(value: Option<&Value>, name: &str) -> Result<Option<T>> {
        match value {
            None => Ok(None),
            Some(_) => T::from_arg(value, name).map(Some),
        }
    }
}

impl<T: ParamField> ParamField for Vec<T> {
    fn param_type() -> ParamType {
        ParamType::List(Box::new(T::param_type()))
    }

    fn from_value(value: &Value) -> Option<Vec<T>> {
        match value {
            Value::List(items) => items.iter().map(T::from_value).collect(),
            _ => None,
        }
    }
}

struct TypedCommand<P, F> {
    spec: CommandSpec,
    run: F,
    _params: std::marker::PhantomData<fn() -> P>,
}

impl<P, F> Command for TypedCommand<P, F>
where
    P: Params,
    F: Fn(&mut CommandCtx<'_>, P) -> Result<Outputs> + Send + Sync,
{
    fn spec(&self) -> &CommandSpec {
        &self.spec
    }

    fn run(&self, ctx: &mut CommandCtx<'_>, args: &Args) -> Result<Outputs> {
        (self.run)(ctx, P::from_args(args)?)
    }
}

/// Builds a command from a spec, a [`Params`] struct and a function. The
/// struct's fields become the command's inputs, replacing any already on
/// `spec`.
pub fn typed<P, F>(mut spec: CommandSpec, run: F) -> Arc<dyn Command>
where
    P: Params + 'static,
    F: Fn(&mut CommandCtx<'_>, P) -> Result<Outputs> + Send + Sync + 'static,
{
    spec.inputs = P::specs();
    Arc::new(TypedCommand {
        spec,
        run,
        _params: std::marker::PhantomData,
    })
}
