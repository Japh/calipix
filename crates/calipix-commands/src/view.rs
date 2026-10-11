//! Display commands: display range, lookup table and channel mode.
//!
//! Everything here changes how an image is shown and nothing else. A
//! command replaces the image's metadata ([`Image::with_meta`]) and leaves
//! its samples exactly as they were, shared with the image before it, so
//! undo costs nothing and "looking never changes data" holds by
//! construction.
//!
//! Planes are addressed the way ImageJ's `Stack.setPosition` does:
//! `channel`, `slice` and `frame` counted from 1. Channel 0 means every
//! channel.

use calipix_command::{
    CommandCtx, CommandSpec, DatasetId, Effects, Error, ImageEffect, Outputs, Params, Registry,
    Result, typed,
};
use calipix_core::{AxisKind, ChannelMeta, ColorModel, Image, ImageMeta, Lut};
use calipix_render::ChannelMode;

/// Ids of the commands defined here. None of them changes a sample, which
/// lets a viewer keep what it has loaded when one of them runs.
pub const DISPLAY_COMMANDS: [&str; 7] = [
    "view.set_display_range",
    "view.reset_display_range",
    "view.auto_contrast",
    "view.enhance_contrast",
    "view.set_lut",
    "view.invert_lut",
    "view.set_channel_mode",
];

/// The property recording a channel mode that [`ColorModel`] cannot tell
/// apart: `color` or `grayscale`. It is the `mode` entry of ImageJ's TIFF
/// description, so the mode survives saving.
pub const MODE_PROPERTY: &str = "imagej.mode";

/// How the channels of an image with this metadata are shown:
/// [`ChannelMode::Composite`], [`ChannelMode::Color`] or
/// [`ChannelMode::Grayscale`], never `Auto`.
pub fn channel_mode(meta: &ImageMeta) -> ChannelMode {
    if meta.color == ColorModel::Composite {
        return ChannelMode::Composite;
    }
    match meta.properties.get(MODE_PROPERTY).map(String::as_str) {
        Some("grayscale") => ChannelMode::Grayscale,
        _ => ChannelMode::Color,
    }
}

/// The position (one index per axis beyond X and Y) of the plane at
/// `channel` (from 0), `slice` and `frame` (from 1). Axes of other kinds
/// are at their first index.
pub fn plane_position(
    meta: &ImageMeta,
    channel: usize,
    slice: i64,
    frame: i64,
) -> Result<Vec<usize>> {
    let index = |name: &str, value: i64, size: usize| {
        if value >= 1 && value as u64 <= size as u64 {
            Ok(value as usize - 1)
        } else {
            Err(Error::InvalidArg {
                param: name.to_string(),
                reason: format!("{value} is outside 1 to {size}"),
            })
        }
    };
    meta.axes()
        .iter()
        .zip(meta.shape())
        .skip(2)
        .map(|(axis, &size)| match axis.kind {
            AxisKind::Channel => Ok(channel.min(size - 1)),
            AxisKind::Z => index("slice", slice, size),
            AxisKind::Time => index("frame", frame, size),
            _ => Ok(0),
        })
        .collect()
}

/// The channels (from 0) a `channel` argument names: all of them for 0.
fn channels(meta: &ImageMeta, channel: i64) -> Result<Vec<usize>> {
    let count = meta.size(&AxisKind::Channel);
    match channel {
        0 => Ok((0..count).collect()),
        c if c >= 1 && c as u64 <= count as u64 => Ok(vec![c as usize - 1]),
        c => Err(Error::InvalidArg {
            param: "channel".to_string(),
            reason: if count == 1 {
                format!("{c} is not a channel; the image has one channel")
            } else {
                format!("{c} is not a channel; the image has channels 1 to {count}")
            },
        }),
    }
}

/// Replaces the metadata of image `id` with an edited copy. `edit` gets
/// one [`ChannelMeta`] per channel to work on.
fn update(
    ctx: &mut CommandCtx<'_>,
    id: DatasetId,
    edit: impl FnOnce(&Image, &mut ImageMeta) -> Result<()>,
) -> Result<Outputs> {
    let image = ctx.image(id)?.clone();
    let mut meta = image.meta().clone();
    let count = meta.size(&AxisKind::Channel);
    if meta.channels.len() != count {
        meta.channels = vec![ChannelMeta::default(); count];
    }
    edit(&image, &mut meta)?;
    ctx.workspace_mut().set_image(id, image.with_meta(meta)?)?;
    Ok(Outputs::new())
}

fn is_rgb(image: &Image) -> bool {
    image.meta().color == ColorModel::Rgb
}

/// Sets the display range of each of `channels` to what `range` gives for
/// its plane at `slice` and `frame`. For an RGB image, whose three
/// channels are one picture, the range found for the first is given to
/// all three.
fn set_ranges(
    image: &Image,
    meta: &mut ImageMeta,
    mut selected: Vec<usize>,
    slice: i64,
    frame: i64,
    range: impl Fn(&[usize]) -> Result<Option<(f64, f64)>>,
) -> Result<usize> {
    let rgb = is_rgb(image);
    if rgb {
        selected = vec![0];
    }
    let mut changed = 0;
    for channel in selected {
        let position = plane_position(image.meta(), channel, slice, frame)?;
        let Some(found) = range(&position)? else {
            continue;
        };
        changed += 1;
        if rgb {
            for entry in &mut meta.channels {
                entry.display_range = Some(found);
            }
        } else {
            meta.channels[channel].display_range = Some(found);
        }
    }
    Ok(changed)
}

/// The lookup table a channel is shown through when the image is shown in
/// colour, whether it has its own or a default.
fn shown_lut(image: &Image, channel: usize) -> Lut {
    let mode = match channel_mode(image.meta()) {
        ChannelMode::Composite => ChannelMode::Composite,
        _ => ChannelMode::Color,
    };
    calipix_render::channel_lut(image, channel, mode)
}

/// [`calipix_render::inverted_lut`], named so that inverting twice gives
/// back the original name.
fn inverted(lut: &Lut) -> Lut {
    let mut flipped = calipix_render::inverted_lut(lut);
    if let Some(original) = lut.name.strip_suffix(" (inverted)") {
        flipped.name = original.to_string();
    }
    flipped
}

fn no_lut_for_rgb() -> Error {
    Error::failed(
        "an RGB image has no lookup table; change its channel mode to composite first \
         (View > Set Channel Mode)",
    )
}

#[derive(Params)]
struct SetRangeParams {
    /// The image whose display changes.
    #[param(label = "Image")]
    input: DatasetId,
    /// Sample value shown as the darkest level.
    #[param(label = "Minimum", no_persist)]
    min: f64,
    /// Sample value shown as the brightest level.
    #[param(label = "Maximum", no_persist)]
    max: f64,
    /// Channel to change, counted from 1; 0 changes every channel.
    #[param(default = 0, min = 0)]
    channel: i64,
}

#[derive(Params)]
struct PlaneParams {
    /// The image whose display changes.
    #[param(label = "Image")]
    input: DatasetId,
    /// Channel to change, counted from 1; 0 changes every channel.
    #[param(default = 0, min = 0)]
    channel: i64,
    /// Slice whose samples decide the range, counted from 1.
    #[param(default = 1, min = 1, advanced, no_persist)]
    slice: i64,
    /// Frame whose samples decide the range, counted from 1.
    #[param(default = 1, min = 1, advanced, no_persist)]
    frame: i64,
}

#[derive(Params)]
struct AutoParams {
    /// The image whose display changes.
    #[param(label = "Image")]
    input: DatasetId,
    /// Channel to change, counted from 1; 0 changes every channel.
    #[param(default = 0, min = 0)]
    channel: i64,
    /// Slice whose samples decide the range, counted from 1.
    #[param(default = 1, min = 1, advanced, no_persist)]
    slice: i64,
    /// Frame whose samples decide the range, counted from 1.
    #[param(default = 1, min = 1, advanced, no_persist)]
    frame: i64,
    /// How hard to stretch: 1 is the first press of ImageJ's Auto button,
    /// and each further press narrows the range, up to 10.
    #[param(label = "Strength", default = 1, min = 1, advanced, no_persist)]
    press: i64,
}

#[derive(Params)]
struct EnhanceParams {
    /// The image whose display changes.
    #[param(label = "Image")]
    input: DatasetId,
    /// Share of the samples allowed to show as pure black or pure white,
    /// half at each end.
    #[param(
        label = "Saturated pixels",
        default = 0.35,
        min = 0,
        max = 100,
        step = 0.05,
        unit = "%"
    )]
    saturated: f64,
    /// Take the range from every plane of the image rather than one.
    #[param(default = false, ijm = "use")]
    use_stack_histogram: bool,
    /// Channel to change, counted from 1; 0 changes every channel.
    #[param(default = 0, min = 0)]
    channel: i64,
    /// Slice whose samples decide the range, counted from 1.
    #[param(default = 1, min = 1, advanced, no_persist)]
    slice: i64,
    /// Frame whose samples decide the range, counted from 1.
    #[param(default = 1, min = 1, advanced, no_persist)]
    frame: i64,
}

#[derive(Params)]
struct SetLutParams {
    /// The image whose display changes.
    #[param(label = "Image")]
    input: DatasetId,
    /// The lookup table to show the channel through.
    #[param(
        label = "Lookup table",
        choices(
            "fire", "grays", "ice", "spectrum", "rgb332", "red", "green", "blue", "cyan",
            "magenta", "yellow", "redgreen"
        )
    )]
    lut: String,
    /// Channel to change, counted from 1; 0 changes every channel.
    #[param(default = 0, min = 0)]
    channel: i64,
}

#[derive(Params)]
struct ChannelParams {
    /// The image whose display changes.
    #[param(label = "Image")]
    input: DatasetId,
    /// Channel to change, counted from 1; 0 changes every channel.
    #[param(default = 0, min = 0)]
    channel: i64,
}

#[derive(Params)]
struct ModeParams {
    /// The image whose display changes.
    #[param(label = "Image")]
    input: DatasetId,
    /// Composite shows all channels together, each in its own colour;
    /// color and grayscale show one channel at a time.
    #[param(
        label = "Mode",
        default = "composite",
        choices("composite", "color", "grayscale"),
        ijm = "display"
    )]
    mode: String,
}

/// A spec for a command in the View menu that changes how an image looks.
fn display_spec(id: &str, title: &str, weight: f64) -> CommandSpec {
    CommandSpec::new(id, title)
        .menu("View", weight)
        .effects(Effects {
            image: ImageEffect::Modifies,
            ..Effects::default()
        })
}

pub(crate) fn register(registry: &mut Registry) -> Result<()> {
    registry.register(typed(
        display_spec("view.set_display_range", "Set Display Range", 210.0)
            .summary("Chooses the sample values shown as black and as white.")
            .doc(
                "Samples at or below the minimum show as the darkest level and samples at \
                 or above the maximum as the brightest. Sample values are not changed.",
            ),
        |ctx, p: SetRangeParams| {
            if !p.min.is_finite() || !p.max.is_finite() {
                return Err(Error::failed("the display range needs two finite numbers"));
            }
            update(ctx, p.input, |image, meta| {
                for channel in channels(image.meta(), p.channel)? {
                    meta.channels[channel].display_range = Some((p.min, p.max));
                }
                Ok(())
            })
        },
    ))?;

    registry.register(typed(
        display_spec("view.auto_contrast", "Auto Contrast", 220.0)
            .summary("Picks a display range that suits the plane being viewed.")
            .doc(
                "Does what the Auto button of ImageJ's Brightness and Contrast window does: \
                 the range is taken from the plane's histogram, ignoring thinly populated \
                 ends. Sample values are not changed.",
            ),
        |ctx, p: AutoParams| {
            update(ctx, p.input, |image, meta| {
                let selected = channels(image.meta(), p.channel)?;
                let press = p.press.clamp(1, u32::MAX as i64) as u32;
                set_ranges(image, meta, selected, p.slice, p.frame, |position| {
                    Ok(Some(
                        calipix_render::auto_contrast(image, position, press)?.range,
                    ))
                })?;
                Ok(())
            })
        },
    ))?;

    registry.register(typed(
        display_spec("view.enhance_contrast", "Enhance Contrast", 230.0)
            .summary("Stretches the display range until a given share of the samples is saturated.")
            .doc(
                "Follows ImageJ's Process > Enhance Contrast without its Normalize and \
                 Equalize options, which rewrite the image: here only the display range \
                 changes.",
            )
            .ijm_name("Enhance Contrast..."),
        |ctx, p: EnhanceParams| {
            update(ctx, p.input, |image, meta| {
                let selected = channels(image.meta(), p.channel)?;
                let changed = set_ranges(image, meta, selected, p.slice, p.frame, |position| {
                    Ok(calipix_render::enhance_contrast(
                        image,
                        position,
                        p.saturated,
                        p.use_stack_histogram,
                    )?)
                })?;
                if changed == 0 {
                    return Err(Error::failed(
                        "no range was found: at this saturation the darkest and brightest \
                         samples left are the same",
                    ));
                }
                Ok(())
            })
        },
    ))?;

    registry.register(typed(
        display_spec("view.reset_display_range", "Reset Display Range", 240.0)
            .summary("Shows the full range of the plane being viewed.")
            .doc(
                "The range becomes 0 to 255 for 8-bit images and the smallest and largest \
                 sample of the plane for others, as the Reset button of ImageJ's Brightness \
                 and Contrast window does.",
            ),
        |ctx, p: PlaneParams| {
            update(ctx, p.input, |image, meta| {
                let selected = channels(image.meta(), p.channel)?;
                set_ranges(image, meta, selected, p.slice, p.frame, |position| {
                    Ok(Some(calipix_render::reset_range(image, position)?))
                })?;
                Ok(())
            })
        },
    ))?;

    registry.register(typed(
        display_spec("view.set_lut", "Set Lookup Table", 310.0)
            .summary("Shows a channel through one of the built-in lookup tables.")
            .doc(
                "The lookup tables are ImageJ's: Fire, Grays, Ice, Spectrum, 3-3-2 RGB, Red, \
                 Green, Blue, Cyan, Magenta, Yellow and Red/Green. Sample values are not \
                 changed.",
            ),
        |ctx, p: SetLutParams| {
            let lut = calipix_render::builtin_lut(&p.lut)
                .ok_or_else(|| Error::failed(format!("there is no lookup table `{}`", p.lut)))?;
            update(ctx, p.input, |image, meta| {
                if is_rgb(image) {
                    return Err(no_lut_for_rgb());
                }
                for channel in channels(image.meta(), p.channel)? {
                    meta.channels[channel].lut = Some(lut.clone());
                }
                Ok(())
            })
        },
    ))?;

    registry.register(typed(
        display_spec("view.invert_lut", "Invert Lookup Table", 320.0)
            .summary("Reverses a channel's lookup table, so that dark shows as bright.")
            .doc("Sample values are not changed; use Process > Invert for that.")
            .ijm_name("Invert LUT"),
        |ctx, p: ChannelParams| {
            update(ctx, p.input, |image, meta| {
                if is_rgb(image) {
                    return Err(no_lut_for_rgb());
                }
                for channel in channels(image.meta(), p.channel)? {
                    meta.channels[channel].lut = Some(inverted(&shown_lut(image, channel)));
                }
                Ok(())
            })
        },
    ))?;

    registry.register(typed(
        display_spec("view.set_channel_mode", "Set Channel Mode", 410.0)
            .summary("Chooses whether channels are shown together or one at a time.")
            .doc(
                "Composite blends every visible channel, each through its own lookup table. \
                 Color shows the selected channel through its lookup table and grayscale \
                 shows it in shades of grey. An RGB image made composite keeps its look and \
                 gains a display range and lookup table per channel.",
            )
            .ijm_name("Make Composite"),
        |ctx, p: ModeParams| {
            update(ctx, p.input, |image, meta| {
                if image.meta().size(&AxisKind::Channel) < 2 {
                    return Err(Error::failed(
                        "the image has a single channel, so there is nothing to combine",
                    ));
                }
                if p.mode == "composite" {
                    meta.color = ColorModel::Composite;
                    meta.properties.remove(MODE_PROPERTY);
                } else {
                    meta.color = ColorModel::Grayscale;
                    meta.properties
                        .insert(MODE_PROPERTY.to_string(), p.mode.clone());
                }
                Ok(())
            })
        },
    ))?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use calipix_core::DType;

    #[test]
    fn every_built_in_lookup_table_can_be_chosen() {
        let ids: Vec<String> = calipix_render::builtin_luts()
            .iter()
            .map(|lut| lut.id.to_string())
            .collect();
        let spec = SetLutParams::specs()
            .into_iter()
            .find(|param| param.name == "lut")
            .unwrap();
        assert_eq!(spec.ty, calipix_command::ParamType::Choice(ids));
    }

    #[test]
    fn positions_count_from_one_and_are_checked() {
        let meta = ImageMeta::xyczt(DType::U8, 2, 2, 3, 4, 5).unwrap();
        assert_eq!(plane_position(&meta, 2, 4, 1).unwrap(), [2, 3, 0]);
        assert!(plane_position(&meta, 0, 5, 1).is_err());
        assert!(plane_position(&meta, 0, 1, 0).is_err());
        // Axes the image lacks take any position's first plane.
        let flat = ImageMeta::plane(DType::U8, 2, 2).unwrap();
        assert_eq!(plane_position(&flat, 0, 1, 1).unwrap(), Vec::<usize>::new());
        assert!(plane_position(&flat, 0, 2, 1).is_ok());
        assert_eq!(channels(&meta, 0).unwrap(), [0, 1, 2]);
        assert_eq!(channels(&meta, 3).unwrap(), [2]);
        assert!(channels(&meta, 4).is_err());
    }
}
