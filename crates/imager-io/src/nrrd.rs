use std::path::Path;

use imager_core::Image;

use crate::{Error, Format, Result};

pub(crate) struct Nrrd;

impl Format for Nrrd {
    fn name(&self) -> &'static str {
        "NRRD"
    }

    fn extensions(&self) -> &'static [&'static str] {
        &["nrrd", "nhdr"]
    }

    fn matches(&self, _header: &[u8]) -> bool {
        false
    }

    fn read(&self, path: &Path) -> Result<Image> {
        Err(Error::Decode {
            path: path.to_path_buf(),
            message: "NRRD reading is not implemented yet".to_string(),
        })
    }

    fn write(&self, _image: &Image, _path: &Path) -> Result<()> {
        Err(Error::Unsupported {
            format: "NRRD",
            reason: "not implemented yet".to_string(),
        })
    }
}
