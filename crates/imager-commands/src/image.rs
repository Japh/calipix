use imager_command::{
    CommandSpec, Effects, ImageEffect, Outputs, ParamType, Registry, Result, typed,
};
use imager_core::{AxisKind, Image};

use crate::InputOnly;

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
            .menu("Image > Adjust", 10.0)
            .ijm_name("Invert")
            .effects(Effects {
                image: ImageEffect::Modifies,
                ..Effects::default()
            }),
        |ctx, p: InputOnly| {
            let image = ctx.image(p.input)?.clone();
            let meta = image.meta().clone();
            let count = meta.plane_count();
            let mut planes = Vec::with_capacity(count);
            for plane in 0..count {
                ctx.check_cancelled()?;
                ctx.progress(plane as f64 / count as f64, "Inverting");
                let range = meta
                    .channels
                    .get(meta.plane_channel(plane)?)
                    .and_then(|c| c.display_range);
                planes.push(imager_ops::invert_plane(&image.read_plane(plane)?, range)?);
            }
            ctx.workspace_mut()
                .set_image(p.input, Image::from_planes(meta, planes)?)?;
            Ok(Outputs::new())
        },
    ))?;

    Ok(())
}
