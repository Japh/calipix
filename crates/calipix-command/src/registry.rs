use std::collections::BTreeMap;
use std::sync::Arc;

use crate::{Command, CommandSpec, Error, Result};

/// All commands known to the application, keyed by id.
#[derive(Default)]
pub struct Registry {
    commands: BTreeMap<String, Arc<dyn Command>>,
}

impl Registry {
    pub fn new() -> Registry {
        Registry::default()
    }

    /// Adds a command. Fails if its definition is invalid or its id is
    /// already taken.
    pub fn register(&mut self, command: Arc<dyn Command>) -> Result<()> {
        let spec = command.spec();
        spec.validate()?;
        if self.commands.contains_key(&spec.id) {
            return Err(Error::DuplicateCommand(spec.id.clone()));
        }
        self.commands.insert(spec.id.clone(), command);
        Ok(())
    }

    pub fn get(&self, id: &str) -> Result<&Arc<dyn Command>> {
        self.commands
            .get(id)
            .ok_or_else(|| Error::UnknownCommand(id.to_string()))
    }

    /// The command ImageJ calls `name`, as used by `run("name")` in macros.
    pub fn by_ijm_name(&self, name: &str) -> Option<&Arc<dyn Command>> {
        self.commands
            .values()
            .find(|c| c.spec().ijm_name.as_deref() == Some(name))
    }

    /// Specs of all commands, ordered by id.
    pub fn specs(&self) -> impl Iterator<Item = &CommandSpec> {
        self.commands.values().map(|c| c.spec())
    }

    pub fn len(&self) -> usize {
        self.commands.len()
    }

    pub fn is_empty(&self) -> bool {
        self.commands.is_empty()
    }
}
