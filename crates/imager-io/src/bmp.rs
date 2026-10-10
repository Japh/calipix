use std::path::Path;

use imager_core::Image;

use crate::{Error, Format, Result};

pub(crate) struct Bmp;

impl Format for Bmp {
    fn name(&self) -> &'static str {
        "BMP"
    }

    fn extensions(&self) -> &'static [&'static str] {
        &["bmp"]
    }

    fn matches(&self, _header: &[u8]) -> bool {
        false
    }

    fn read(&self, path: &Path) -> Result<Image> {
        Err(Error::Decode {
            path: path.to_path_buf(),
            message: "BMP reading is not implemented yet".to_string(),
        })
    }

    fn write(&self, _image: &Image, _path: &Path) -> Result<()> {
        Err(Error::Unsupported {
            format: "BMP",
            reason: "not implemented yet".to_string(),
        })
    }
}
