use std::path::Path;

use imager_core::Image;

use crate::{Error, Format, Result};

pub(crate) struct Tiff;

impl Format for Tiff {
    fn name(&self) -> &'static str {
        "TIFF"
    }

    fn extensions(&self) -> &'static [&'static str] {
        &["tif", "tiff"]
    }

    fn matches(&self, _header: &[u8]) -> bool {
        false
    }

    fn read(&self, path: &Path) -> Result<Image> {
        Err(Error::Decode {
            path: path.to_path_buf(),
            message: "TIFF reading is not implemented yet".to_string(),
        })
    }

    fn write(&self, _image: &Image, _path: &Path) -> Result<()> {
        Err(Error::Unsupported {
            format: "TIFF",
            reason: "not implemented yet".to_string(),
        })
    }
}
