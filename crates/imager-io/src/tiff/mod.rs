//! TIFF, read and written to match ImageJ 1.x.
//!
//! # Reading
//!
//! Opening a file reads its directories and metadata only; planes are read
//! on demand and kept in the shared plane cache.
//!
//! - Files written by ImageJ (ImageDescription starting `ImageJ=`,
//!   uncompressed) are read the way ImageJ reads them: plane `k` is at
//!   `first strip offset + k * plane size`, whatever the directories say.
//!   This covers ImageJ's stacks of 4 GiB and more, which have a single
//!   directory.
//! - Any other TIFF is one image per directory, decoded through the `tiff`
//!   crate: strips or tiles; uncompressed, LZW, Deflate or PackBits; with
//!   or without a predictor; classic or BigTIFF; either byte order. A file
//!   whose directories differ in size or type opens as its first image
//!   only, as in ImageJ.
//! - A single-file OME-TIFF takes its dimensions, plane order, physical
//!   sizes and channel names from the OME-XML of its first image.
//!
//! Samples are never altered: WhiteIsZero data and palette images keep
//! their stored values and get an inverting or palette lookup table. RGB
//! becomes three channels marked [`ColorModel::Rgb`](imager_core::ColorModel)
//! (alpha is discarded); other multi-sample pixels become channels. ImageJ's
//! "16-bit signed" files load as `i16` with their true values.
//!
//! # Properties
//!
//! Metadata with no place in [`ImageMeta`](imager_core::ImageMeta) is kept
//! in its `properties` and written back by [`Format::write`]:
//!
//! | Key | Meaning |
//! |---|---|
//! | `Info` | ImageJ's "Info" image property (Image > Show Info) |
//! | `imagej.label.N` | Label of stack slice `N` (0-based, in ImageJ's stack order) |
//! | `imagej.roi` | The active selection in ImageJ's `.roi` encoding, as hex |
//! | `imagej.overlay.N` | Overlay element `N` in `.roi` encoding, as hex |
//! | `imagej.KEY` | Entry `KEY` of the ImageJ description that is not image structure: `cf`, `c0`..`c4`, `vunit`, `zeroclip` (density calibration), `fps`, `loop`, `inverty`, `info`, `mode` (only `grayscale` with stored lookup tables), and any entry we do not know |
//! | any other key | An ImageJ image property (the `prop` metadata block), for example `CurrentLUT` |
//! | `tiff.description` | ImageDescription of a file not written by ImageJ (read only) |
//! | `tiff.software`, `tiff.datetime`, `tiff.artist`, `tiff.host_computer` | The corresponding TIFF tags (read only) |
//!
//! # Writing
//!
//! Files are little-endian, uncompressed and in ImageJ's layout, with its
//! description and metadata tags, so ImageJ opens them as it would its own.
//! `u8`, `u16`, `f32` and 8-bit RGB images are exactly ImageJ's types; `i16`
//! is written as ImageJ's signed 16-bit (unsigned plus a calibration
//! function). Other real types use the same layout with a SampleFormat tag,
//! which ImageJ reads only in part. A file that would reach 4 GiB is written
//! with a single directory if it is one of ImageJ's types (as ImageJ does),
//! and as BigTIFF otherwise.

mod ifd;
mod imagej;
mod ome;
mod reader;
mod source;
#[cfg(test)]
mod tests;
mod writer;

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;
use std::sync::Arc;

use imager_core::Image;

use crate::{Error, Format, Result};
use ifd::{FileStorage, TiffError};
use writer::WriteError;

pub(crate) struct Tiff;

impl Format for Tiff {
    fn name(&self) -> &'static str {
        "TIFF"
    }

    fn extensions(&self) -> &'static [&'static str] {
        &["tif", "tiff"]
    }

    fn matches(&self, header: &[u8]) -> bool {
        ifd::is_tiff_header(header)
    }

    /// Opens the file lazily; see the module documentation for what is
    /// understood.
    fn read(&self, path: &Path) -> Result<Image> {
        let storage = File::open(path)
            .and_then(FileStorage::new)
            .map_err(|e| Error::io(path, e))?;
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        reader::open(Arc::new(storage), &name).map_err(|e| match e {
            TiffError::Io(source) => Error::io(path, source),
            other => Error::Decode {
                path: path.to_path_buf(),
                message: other.to_string(),
            },
        })
    }

    /// Writes plane by plane into a temporary file beside `path` and
    /// renames it into place, so a failed save leaves no partial file and
    /// an image can be saved over the file it is still being read from.
    fn write(&self, image: &Image, path: &Path) -> Result<()> {
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        let temporary = path.with_file_name(format!(".{name}.{}.tmp", std::process::id()));
        let result = write_file(image, &temporary)
            .and_then(|()| std::fs::rename(&temporary, path).map_err(|e| Error::io(path, e)));
        if result.is_err() {
            let _ = std::fs::remove_file(&temporary);
        }
        result
    }
}

fn write_file(image: &Image, path: &Path) -> Result<()> {
    let file = File::create(path).map_err(|e| Error::io(path, e))?;
    let mut out = BufWriter::with_capacity(1 << 20, file);
    writer::write(image, &mut out, writer::Options::default())
        .and_then(|()| out.flush().map_err(WriteError::Io))
        .map_err(|e| match e {
            WriteError::Unsupported(reason) => Error::Unsupported {
                format: "TIFF",
                reason,
            },
            WriteError::Io(source) => Error::io(path, source),
            WriteError::Core(source) => Error::Core(source),
        })
}
