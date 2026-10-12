use std::path::PathBuf;

use calipix_command::{
    CommandSpec, DatasetId, Effects, Error, ImageEffect, Outputs, ParamType, Params, Registry,
    Result, typed,
};

use crate::InputOnly;

#[derive(Params)]
struct OpenParams {
    /// The image file to open.
    #[param(label = "File", no_persist)]
    path: PathBuf,
}

#[derive(Params)]
struct SaveAsParams {
    /// The image to save.
    #[param(label = "Image")]
    input: DatasetId,
    /// Where to save it. The extension chooses the file format.
    #[param(label = "File", path = "save", no_persist)]
    path: PathBuf,
}

pub(crate) fn register(registry: &mut Registry) -> Result<()> {
    registry.register(typed(
        CommandSpec::new("file.open", "Open")
            .summary("Opens an image file.")
            .menu("File", 10.0)
            .shortcut("Ctrl+O")
            .output("image", ParamType::Image)
            .effects(Effects {
                image: ImageEffect::Creates,
                reads_fs: true,
                ..Effects::default()
            }),
        |ctx, p: OpenParams| {
            let image = calipix_io::open(&p.path).map_err(Error::failed)?;
            let id = ctx.workspace_mut().insert(image, Some(p.path));
            Ok(Outputs::new().with("image", id))
        },
    ))?;

    registry.register(typed(
        CommandSpec::new("file.save", "Save")
            .summary("Saves an image to the file it was opened from or last saved to.")
            .menu("File", 110.0)
            .shortcut("Ctrl+S")
            .effects(Effects {
                writes_fs: true,
                ..Effects::default()
            }),
        |ctx, p: InputOnly| {
            let dataset = ctx.workspace().get(p.input)?;
            let Some(path) = dataset.path.clone() else {
                return Err(Error::failed(
                    "this image has not been saved to a file yet; use Save As",
                ));
            };
            calipix_io::save(&dataset.image, &path).map_err(Error::failed)?;
            Ok(Outputs::new())
        },
    ))?;

    registry.register(typed(
        CommandSpec::new("file.save_as", "Save As")
            .summary("Saves an image to a file in the format given by its extension.")
            .menu("File", 120.0)
            .shortcut("Ctrl+Shift+S")
            .effects(Effects {
                writes_fs: true,
                ..Effects::default()
            }),
        |ctx, p: SaveAsParams| {
            calipix_io::save(ctx.image(p.input)?, &p.path).map_err(Error::failed)?;
            ctx.workspace_mut().set_path(p.input, Some(p.path))?;
            Ok(Outputs::new())
        },
    ))?;

    registry.register(typed(
        CommandSpec::new("file.close", "Close")
            .summary("Closes an image without saving it.")
            .menu("File", 210.0)
            .shortcut("Ctrl+W")
            .ijm_name("Close")
            .effects(Effects {
                image: ImageEffect::Modifies,
                ..Effects::default()
            }),
        |ctx, p: InputOnly| {
            ctx.workspace_mut().remove(p.input)?;
            Ok(Outputs::new())
        },
    ))?;

    Ok(())
}
