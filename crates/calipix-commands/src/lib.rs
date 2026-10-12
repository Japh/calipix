//! The standard command library: everything the application can do,
//! defined once as commands.

mod convert;
mod file;
mod filter;
mod image;
mod math;
pub mod measure;
pub mod process;
mod regions;
mod select;
mod transform;
pub mod view;

use calipix_command::{DatasetId, Params, Registry, Result};

/// Parameters of commands that act on one image and need nothing else.
#[derive(Params)]
pub(crate) struct InputOnly {
    /// The image to act on.
    #[param(label = "Image")]
    pub input: DatasetId,
}

/// Adds every built-in command to `registry`.
pub fn register(registry: &mut Registry) -> Result<()> {
    file::register(registry)?;
    image::register(registry)?;
    select::register(registry)?;
    regions::register(registry)?;
    convert::register(registry)?;
    filter::register(registry)?;
    math::register(registry)?;
    measure::register(registry)?;
    transform::register(registry)?;
    view::register(registry)?;
    Ok(())
}

/// A registry holding exactly the built-in commands.
pub fn standard_registry() -> Registry {
    let mut registry = Registry::new();
    register(&mut registry).expect("built-in commands are valid and unique");
    registry
}
