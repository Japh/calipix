//! Reading and writing image files.
//!
//! Each file format implements [`Format`]. [`open`] picks the format from
//! the file's leading bytes, falling back to its extension; [`save`] picks
//! it from the extension.

mod bmp;
mod gif;
mod jpeg;
mod nrrd;
mod png;
mod tiff;

use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

use calipix_core::Image;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("{0}: unrecognised image format")]
    UnknownFormat(PathBuf),
    #[error("{path}: {message}")]
    Decode { path: PathBuf, message: String },
    #[error("cannot save as {format}: {reason}")]
    Unsupported {
        format: &'static str,
        reason: String,
    },
    #[error(transparent)]
    Core(#[from] calipix_core::Error),
}

impl Error {
    pub(crate) fn io(path: &Path, source: std::io::Error) -> Error {
        Error::Io {
            path: path.to_path_buf(),
            source,
        }
    }
}

/// A file format that can be read and possibly written.
pub trait Format: Send + Sync {
    /// Name shown to users, e.g. "PNG".
    fn name(&self) -> &'static str;

    /// Lower-case file extensions without the dot.
    fn extensions(&self) -> &'static [&'static str];

    /// True if `header`, the first bytes of a file, identifies this format.
    fn matches(&self, header: &[u8]) -> bool;

    fn read(&self, path: &Path) -> Result<Image>;

    fn write(&self, image: &Image, path: &Path) -> Result<()>;
}

/// The formats built into the application.
pub fn formats() -> &'static [&'static dyn Format] {
    &[
        &tiff::Tiff,
        &png::Png,
        &jpeg::Jpeg,
        &gif::Gif,
        &bmp::Bmp,
        &nrrd::Nrrd,
    ]
}

fn by_extension(path: &Path) -> Option<&'static dyn Format> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    formats()
        .iter()
        .copied()
        .find(|f| f.extensions().contains(&ext.as_str()))
}

/// Opens an image file.
pub fn open(path: impl AsRef<Path>) -> Result<Image> {
    let path = path.as_ref();
    let mut header = [0u8; 16];
    let read = File::open(path)
        .and_then(|mut f| f.read(&mut header))
        .map_err(|e| Error::io(path, e))?;
    let format = formats()
        .iter()
        .copied()
        .find(|f| f.matches(&header[..read]))
        .or_else(|| by_extension(path))
        .ok_or_else(|| Error::UnknownFormat(path.to_path_buf()))?;
    format.read(path)
}

/// Saves an image in the format implied by the path's extension.
pub fn save(image: &Image, path: impl AsRef<Path>) -> Result<()> {
    let path = path.as_ref();
    let format = by_extension(path).ok_or_else(|| Error::UnknownFormat(path.to_path_buf()))?;
    format.write(image, path)
}
