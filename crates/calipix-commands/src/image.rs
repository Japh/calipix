use calipix_command::{
    CommandSpec, Effects, ImageEffect, Outputs, ParamType, Params, Registry, Result, typed,
};
use calipix_core::AxisKind;

use crate::InputOnly;
use crate::process::PlaneScope;

#[derive(Params)]
struct InvertParams {
    #[param(flatten)]
    scope: PlaneScope,
}

pub(crate) fn register(registry: &mut Registry) -> Result<()> {
    registry.register(typed(
        CommandSpec::new("image.info", "Image Info")
            .summary("Reports an image's size, pixel type and axes.")
            .menu("Image", 10.0)
            .output("name", ParamType::Str)
            .output("width", ParamType::Int)
            .output("height", ParamType::Int)
            .output("channels", ParamType::Int)
            .output("slices", ParamType::Int)
            .output("frames", ParamType::Int)
            .output("planes", ParamType::Int)
            .output("pixel_type", ParamType::Str)
            .output("in_memory", ParamType::Bool),
        |ctx, p: InputOnly| {
            let image = ctx.image(p.input)?;
            let meta = image.meta();
            Ok(Outputs::new()
                .with("name", meta.name.clone())
                .with("width", meta.width())
                .with("height", meta.height())
                .with("channels", meta.size(&AxisKind::Channel))
                .with("slices", meta.size(&AxisKind::Z))
                .with("frames", meta.size(&AxisKind::Time))
                .with("planes", meta.plane_count())
                .with("pixel_type", meta.dtype().name())
                .with("in_memory", image.is_in_memory()))
        },
    ))?;

    registry.register(typed(
        CommandSpec::new("image.invert", "Invert")
            .summary("Inverts an image's intensities, turning dark to bright and bright to dark.")
            .doc(
                "8-bit and RGB images are inverted over 0 to 255. Other integer images are \
                 inverted over each plane's own minimum and maximum, and float images over \
                 their display range (or each plane's minimum and maximum if none is set).",
            )
            .menu("Process", 10.0)
            .ijm_name("Invert")
            .effects(Effects {
                image: ImageEffect::Modifies,
                ..Effects::default()
            }),
        |ctx, p: InvertParams| {
            let meta = ctx.image(p.scope.input)?.meta().clone();
            p.scope.apply(ctx, "Inverting", |plane, info| {
                let range = meta
                    .channels
                    .get(info.channel)
                    .and_then(|c| c.display_range);
                calipix_ops::invert_plane(plane, range)
            })
        },
    ))?;

    Ok(())
}
