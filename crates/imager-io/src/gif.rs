use std::path::Path;

use imager_core::Image;

use crate::{Error, Format, Result};

pub(crate) struct Gif;

impl Format for Gif {
    fn name(&self) -> &'static str {
        "GIF"
    }

    fn extensions(&self) -> &'static [&'static str] {
        &["gif"]
    }

    fn matches(&self, _header: &[u8]) -> bool {
        false
    }

    fn read(&self, path: &Path) -> Result<Image> {
        Err(Error::Decode {
            path: path.to_path_buf(),
            message: "GIF reading is not implemented yet".to_string(),
        })
    }

    fn write(&self, _image: &Image, _path: &Path) -> Result<()> {
        Err(Error::Unsupported {
            format: "GIF",
            reason: "not implemented yet".to_string(),
        })
    }
}
