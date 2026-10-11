//! The command system: one typed, schema-described definition per command.
//!
//! A [`CommandSpec`] says what a command is called, where it sits in the
//! menus, what parameters it takes and what effects it has. Every frontend
//! (menus, dialogs, command line, macros, plugins, agents) is derived from
//! that one description, and every run goes through the [`Executor`], which
//! resolves and validates arguments, enforces the effects policy, commits
//! results atomically and records history for undo and provenance.

// Lets `#[derive(Params)]` refer to this crate by name from inside it.
extern crate self as calipix_command;

mod command;
pub mod conformance;
mod error;
mod exec;
mod history;
mod menu;
mod registry;
mod spec;
mod value;
mod workspace;

pub use calipix_command_derive::Params;
pub use command::{Args, Command, CommandCtx, Outputs, ParamField, Params, typed};
pub use error::{Error, Result};
pub use exec::{CancelToken, EffectsPolicy, Executor, Progress, RunOptions, Session};
pub use history::{Change, History, HistoryEntry};
pub use menu::{MenuItem, menu_tree};
pub use registry::Registry;
pub use spec::{CommandSpec, Effects, Flags, ImageEffect, MenuPath, OutputSpec, ParamSpec};
pub use value::{DatasetId, ParamType, PathMode, Value};
pub use workspace::{Dataset, Workspace};
