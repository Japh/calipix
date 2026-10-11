use std::collections::BTreeMap;
use std::path::PathBuf;

use calipix_core::Image;

use crate::{DatasetId, Error, Result};

/// An open image together with what the application knows about it.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct Dataset {
    pub id: DatasetId,
    pub image: Image,
    /// File the image was opened from or last saved to.
    pub path: Option<PathBuf>,
}

impl Dataset {
    /// True if both hold the same image contents. The file path is not
    /// compared: saving is not something undo should revert.
    pub(crate) fn same_as(&self, other: &Dataset) -> bool {
        self.id == other.id && self.image.ptr_eq(&other.image)
    }
}

/// The set of open images. Cloning is cheap (images are shared), which is
/// what lets the executor run a command against a scratch copy and commit
/// or discard the result as a whole.
#[derive(Clone, Debug, Default)]
pub struct Workspace {
    datasets: BTreeMap<DatasetId, Dataset>,
    next_id: u64,
    active: Option<DatasetId>,
}

impl Workspace {
    pub fn new() -> Workspace {
        Workspace::default()
    }

    /// Adds an image and makes it the active one.
    pub fn insert(&mut self, image: Image, path: Option<PathBuf>) -> DatasetId {
        let id = DatasetId(self.next_id);
        self.next_id += 1;
        self.datasets.insert(id, Dataset { id, image, path });
        self.active = Some(id);
        id
    }

    pub fn get(&self, id: DatasetId) -> Result<&Dataset> {
        self.datasets.get(&id).ok_or(Error::NoDataset(id))
    }

    pub fn image(&self, id: DatasetId) -> Result<&Image> {
        self.get(id).map(|d| &d.image)
    }

    /// Replaces the pixels and metadata of an open image.
    pub fn set_image(&mut self, id: DatasetId, image: Image) -> Result<()> {
        self.datasets
            .get_mut(&id)
            .ok_or(Error::NoDataset(id))?
            .image = image;
        Ok(())
    }

    pub fn set_path(&mut self, id: DatasetId, path: Option<PathBuf>) -> Result<()> {
        self.datasets.get_mut(&id).ok_or(Error::NoDataset(id))?.path = path;
        Ok(())
    }

    /// Closes an image. If it was active, the most recently opened remaining
    /// image becomes active.
    pub fn remove(&mut self, id: DatasetId) -> Result<Dataset> {
        let removed = self.datasets.remove(&id).ok_or(Error::NoDataset(id))?;
        if self.active == Some(id) {
            self.active = self.datasets.keys().next_back().copied();
        }
        Ok(removed)
    }

    pub fn datasets(&self) -> impl Iterator<Item = &Dataset> {
        self.datasets.values()
    }

    pub fn len(&self) -> usize {
        self.datasets.len()
    }

    pub fn is_empty(&self) -> bool {
        self.datasets.is_empty()
    }

    /// The image commands act on when none is named.
    pub fn active(&self) -> Option<DatasetId> {
        self.active
    }

    pub fn set_active(&mut self, id: DatasetId) -> Result<()> {
        self.get(id)?;
        self.active = Some(id);
        Ok(())
    }

    /// Puts a dataset's contents back as they were, or removes it, without
    /// touching id allocation. Used by undo and redo. A dataset that is
    /// still open keeps its current file path.
    pub(crate) fn restore(&mut self, id: DatasetId, state: Option<Dataset>) {
        match (state, self.datasets.get_mut(&id)) {
            (Some(dataset), Some(current)) => current.image = dataset.image,
            (Some(dataset), None) => {
                self.datasets.insert(id, dataset);
            }
            (None, _) => {
                self.datasets.remove(&id);
            }
        }
    }

    pub(crate) fn restore_active(&mut self, active: Option<DatasetId>) {
        self.active = active.filter(|id| self.datasets.contains_key(id));
    }

    pub(crate) fn dataset(&self, id: DatasetId) -> Option<&Dataset> {
        self.datasets.get(&id)
    }

    pub(crate) fn ids(&self) -> impl Iterator<Item = DatasetId> + '_ {
        self.datasets.keys().copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image() -> Image {
        Image::from_vec(1, 1, vec![0u8]).unwrap()
    }

    #[test]
    fn insert_activates_and_ids_are_never_reused() {
        let mut ws = Workspace::new();
        let a = ws.insert(image(), None);
        let b = ws.insert(image(), None);
        assert_eq!(ws.active(), Some(b));
        ws.remove(b).unwrap();
        assert_eq!(ws.active(), Some(a));
        let c = ws.insert(image(), None);
        assert_ne!(c, b);
        assert!(ws.get(b).is_err());
        assert!(ws.set_active(b).is_err());
        ws.remove(a).unwrap();
        ws.remove(c).unwrap();
        assert_eq!(ws.active(), None);
        assert!(ws.is_empty());
    }
}
