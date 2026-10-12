//! Selection commands: choosing the part of an image that later commands
//! act within or measure.

use calipix_command::{
    CommandSpec, DatasetId, Effects, ImageEffect, Outputs, Params, Registry, Result, typed,
};
use calipix_roi::Roi;

use crate::InputOnly;

#[derive(Params)]
struct SetParams {
    /// The image to select within.
    #[param(label = "Image")]
    input: DatasetId,
    /// The selection, for example `rectangle 10 20 30 40` for x, y, width
    /// and height.
    #[param(label = "Selection", no_persist)]
    selection: Roi,
}

fn modifies() -> Effects {
    Effects {
        image: ImageEffect::Modifies,
        ..Effects::default()
    }
}

pub(crate) fn register(registry: &mut Registry) -> Result<()> {
    registry.register(typed(
        CommandSpec::new("select.set", "Specify Selection")
            .summary("Selects a region given by its shape and coordinates.")
            .menu("Edit > Selection", 230.0)
            .effects(modifies()),
        |ctx, p: SetParams| {
            ctx.workspace_mut().set_roi(p.input, Some(p.selection))?;
            Ok(Outputs::new())
        },
    ))?;

    registry.register(typed(
        CommandSpec::new("select.all", "Select All")
            .summary("Selects the whole image.")
            .menu("Edit > Selection", 210.0)
            .shortcut("Ctrl+A")
            .ijm_name("Select All")
            .effects(modifies()),
        |ctx, p: InputOnly| {
            let meta = ctx.image(p.input)?.meta();
            let all = Roi::rectangle(0.0, 0.0, meta.width() as f64, meta.height() as f64);
            ctx.workspace_mut().set_roi(p.input, Some(all))?;
            Ok(Outputs::new())
        },
    ))?;

    registry.register(typed(
        CommandSpec::new("select.none", "Select None")
            .summary("Removes the selection.")
            .menu("Edit > Selection", 220.0)
            .shortcut("Ctrl+Shift+A")
            .ijm_name("Select None")
            .effects(modifies()),
        |ctx, p: InputOnly| {
            ctx.workspace_mut().set_roi(p.input, None)?;
            Ok(Outputs::new())
        },
    ))?;

    Ok(())
}
