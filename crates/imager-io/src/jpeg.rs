use std::path::Path;

use imager_core::Image;

use crate::{Error, Format, Result};

pub(crate) struct Jpeg;

impl Format for Jpeg {
    fn name(&self) -> &'static str {
        "JPEG"
    }

    fn extensions(&self) -> &'static [&'static str] {
        &["jpg", "jpeg"]
    }

    fn matches(&self, _header: &[u8]) -> bool {
        false
    }

    fn read(&self, path: &Path) -> Result<Image> {
        Err(Error::Decode {
            path: path.to_path_buf(),
            message: "JPEG reading is not implemented yet".to_string(),
        })
    }

    fn write(&self, _image: &Image, _path: &Path) -> Result<()> {
        Err(Error::Unsupported {
            format: "JPEG",
            reason: "not implemented yet".to_string(),
        })
    }
}
