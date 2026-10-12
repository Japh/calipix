use std::time::Duration;

use calipix_core::Table;

use crate::{Args, Dataset, DatasetId, Error, Outputs, Result, Workspace};

/// How one dataset differed across a command run. `None` means the dataset
/// did not exist on that side.
#[derive(Clone, Debug)]
pub struct Change {
    pub id: DatasetId,
    pub before: Option<Dataset>,
    pub after: Option<Dataset>,
}

/// How one results table differed across a command run.
#[derive(Clone, Debug)]
pub struct TableChange {
    pub name: String,
    pub before: Option<Table>,
    pub after: Option<Table>,
}

/// Record of one command run: enough to undo it, redo it, and reproduce it.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct HistoryEntry {
    pub command: String,
    pub title: String,
    /// The fully resolved arguments the command ran with.
    pub args: Args,
    pub outputs: Outputs,
    pub changes: Vec<Change>,
    pub table_changes: Vec<TableChange>,
    pub active_before: Option<DatasetId>,
    pub active_after: Option<DatasetId>,
    pub duration: Duration,
}

impl HistoryEntry {
    pub(crate) fn new(
        command: String,
        title: String,
        args: Args,
        outputs: Outputs,
        before: &Workspace,
        after: &Workspace,
        duration: Duration,
    ) -> HistoryEntry {
        HistoryEntry {
            command,
            title,
            args,
            outputs,
            changes: diff(before, after),
            table_changes: diff_tables(before, after),
            active_before: before.active(),
            active_after: after.active(),
            duration,
        }
    }

    /// True if the run changed any open image, selection or table.
    pub fn changed_workspace(&self) -> bool {
        !self.changes.is_empty() || !self.table_changes.is_empty()
    }
}

fn diff_tables(before: &Workspace, after: &Workspace) -> Vec<TableChange> {
    let mut names: Vec<&str> = before.table_names().chain(after.table_names()).collect();
    names.sort_unstable();
    names.dedup();
    names
        .into_iter()
        .filter_map(|name| {
            let (b, a) = (before.table(name), after.table(name));
            let unchanged = match (b, a) {
                (Some(b), Some(a)) => b.ptr_eq(a) || b == a,
                (None, None) => true,
                _ => false,
            };
            (!unchanged).then(|| TableChange {
                name: name.to_string(),
                before: b.cloned(),
                after: a.cloned(),
            })
        })
        .collect()
}

fn diff(before: &Workspace, after: &Workspace) -> Vec<Change> {
    let mut ids: Vec<DatasetId> = before.ids().chain(after.ids()).collect();
    ids.sort_unstable();
    ids.dedup();
    ids.into_iter()
        .filter_map(|id| {
            let (b, a) = (before.dataset(id), after.dataset(id));
            let unchanged = match (b, a) {
                (Some(b), Some(a)) => b.same_as(a),
                (None, None) => true,
                _ => false,
            };
            (!unchanged).then(|| Change {
                id,
                before: b.cloned(),
                after: a.cloned(),
            })
        })
        .collect()
}

/// The sequence of commands run in a session, with an undo position.
///
/// Because images are immutable and share unchanged planes, keeping the
/// before and after state of every step costs only the planes that differ.
#[derive(Clone, Debug, Default)]
pub struct History {
    entries: Vec<HistoryEntry>,
    /// Entries before this index are applied; those from it on are undone.
    cursor: usize,
}

impl History {
    pub fn new() -> History {
        History::default()
    }

    /// Records a run, discarding any undone entries.
    pub(crate) fn push(&mut self, entry: HistoryEntry) {
        self.entries.truncate(self.cursor);
        self.entries.push(entry);
        self.cursor = self.entries.len();
    }

    /// Every recorded entry, including undone ones.
    pub fn entries(&self) -> &[HistoryEntry] {
        &self.entries
    }

    /// Entries currently in effect, oldest first. This is what a recorded
    /// macro replays.
    pub fn applied(&self) -> &[HistoryEntry] {
        &self.entries[..self.cursor]
    }

    pub fn can_undo(&self) -> bool {
        self.applied().iter().any(HistoryEntry::changed_workspace)
    }

    pub fn can_redo(&self) -> bool {
        self.entries[self.cursor..]
            .iter()
            .any(HistoryEntry::changed_workspace)
    }

    /// Reverts the most recent applied entry that changed the workspace,
    /// stepping over entries that only read from it.
    pub(crate) fn undo(&mut self, workspace: &mut Workspace) -> Result<&HistoryEntry> {
        let index = self
            .applied()
            .iter()
            .rposition(HistoryEntry::changed_workspace)
            .ok_or(Error::NothingToUndo)?;
        let entry = &self.entries[index];
        for change in &entry.changes {
            workspace.restore(change.id, change.before.clone());
        }
        for change in &entry.table_changes {
            workspace.restore_table(&change.name, change.before.clone());
        }
        workspace.restore_active(entry.active_before);
        self.cursor = index;
        Ok(entry)
    }

    /// Re-applies the next undone entry that changed the workspace.
    pub(crate) fn redo(&mut self, workspace: &mut Workspace) -> Result<&HistoryEntry> {
        let index = self.cursor
            + self.entries[self.cursor..]
                .iter()
                .position(HistoryEntry::changed_workspace)
                .ok_or(Error::NothingToRedo)?;
        let entry = &self.entries[index];
        for change in &entry.changes {
            workspace.restore(change.id, change.after.clone());
        }
        for change in &entry.table_changes {
            workspace.restore_table(&change.name, change.after.clone());
        }
        workspace.restore_active(entry.active_after);
        self.cursor = index + 1;
        Ok(entry)
    }
}
