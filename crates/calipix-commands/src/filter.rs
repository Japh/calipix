//! Filter commands (Process > Filters).
//!
//! Each command filters the planes in its [`PlaneScope`] with a function
//! of `calipix_ops::filter`, which reproduces the ImageJ command named in
//! `ijm_name`. Parameters carry ImageJ's macro keywords and defaults.

use calipix_command::{
    CommandSpec, DatasetId, Effects, Error, Flags, ImageEffect, Params, Registry, Result, typed,
};
use calipix_core::{ColorModel, ImageMeta, Lut};
use calipix_ops::filter::{self, Direction, Frame, Kernel, Outliers, Rank};

use crate::process::{PlaneInfo, PlaneScope};

/// ImageJ's default kernel for Convolve, a 5 x 5 sharpening kernel.
const DEFAULT_KERNEL: &str = "-1 -1 -1 -1 -1; -1 -1 -1 -1 -1; -1 -1 24 -1 -1; \
                              -1 -1 -1 -1 -1; -1 -1 -1 -1 -1";

#[derive(Params)]
struct ScopeOnly {
    #[param(flatten)]
    scope: PlaneScope,
}

#[derive(Params)]
struct GaussianParams {
    #[param(flatten)]
    scope: PlaneScope,
    /// Standard deviation of the Gaussian: the distance at which its weight
    /// falls to 61% of the centre's.
    #[param(default = 2.0, min = 0, unit = "pixels", ijm = "sigma")]
    sigma: f64,
    /// Read sigma in the image's unit of length (such as micrometres)
    /// instead of pixels. Ignored for an image without a pixel size.
    #[param(label = "Scaled units", default = false, ijm = "scaled")]
    scaled: bool,
}

/// The radius of a circular neighbourhood, for filters ImageJ lets run up
/// to radius 1000.
#[derive(Params)]
struct RadiusParams {
    #[param(flatten)]
    scope: PlaneScope,
    /// Radius of the circular neighbourhood. 0.5 takes the four nearest
    /// neighbours and 1 the surrounding 3 x 3 samples.
    #[param(default = 2.0, min = 0, max = 1000, unit = "pixels", ijm = "radius")]
    radius: f64,
}

/// The same for the median-based filters, which ImageJ limits to 100.
#[derive(Params)]
struct MedianRadiusParams {
    #[param(flatten)]
    scope: PlaneScope,
    /// Radius of the circular neighbourhood. 0.5 takes the four nearest
    /// neighbours and 1 the surrounding 3 x 3 samples.
    #[param(default = 2.0, min = 0, max = 100, unit = "pixels", ijm = "radius")]
    radius: f64,
}

#[derive(Params)]
struct OutliersParams {
    #[param(flatten)]
    scope: PlaneScope,
    /// Radius of the circular neighbourhood whose median replaces an
    /// outlier.
    #[param(default = 2.0, min = 0, max = 100, unit = "pixels", ijm = "radius")]
    radius: f64,
    /// How far a sample must lie from the median of its neighbourhood to
    /// be replaced.
    #[param(default = 50.0, min = 0, ijm = "threshold")]
    threshold: f64,
    /// Replace samples brighter than their surroundings, or darker.
    #[param(
        label = "Which outliers",
        default = "bright",
        choices("bright", "dark"),
        ijm = "which"
    )]
    which: String,
}

#[derive(Params)]
struct TopHatParams {
    #[param(flatten)]
    scope: PlaneScope,
    /// Radius of the circular neighbourhood; structures larger than it are
    /// treated as background.
    #[param(default = 2.0, min = 0, max = 1000, unit = "pixels", ijm = "radius")]
    radius: f64,
    /// The background is brighter than the objects.
    #[param(label = "Light background", default = false, ijm = "light")]
    light_background: bool,
    /// Keep the estimated background (a grayscale opening) instead of
    /// subtracting it from the image.
    #[param(
        label = "Do not subtract (grayscale open)",
        default = false,
        ijm = "don't"
    )]
    dont_subtract: bool,
}

#[derive(Params)]
struct ConvolveParams {
    #[param(flatten)]
    scope: PlaneScope,
    /// The kernel's coefficients: values separated by spaces or commas,
    /// rows by semicolons or line breaks, for example `1 2 1; 2 4 2; 1 2 1`.
    /// The numbers of rows and of columns must be odd. Without a row
    /// separator, 9, 25, 49... values make a square kernel.
    #[param(default = DEFAULT_KERNEL, ijm = "text1")]
    kernel: String,
    /// Divide each result by the sum of the coefficients, which keeps the
    /// image's brightness. Has no effect if they sum to zero.
    #[param(label = "Normalize kernel", default = true, ijm = "normalize")]
    normalize: bool,
}

#[derive(Params)]
struct UnsharpParams {
    #[param(flatten)]
    scope: PlaneScope,
    /// Standard deviation of the Gaussian blur that is subtracted.
    #[param(
        label = "Radius (sigma)",
        default = 1.0,
        min = 0,
        unit = "pixels",
        ijm = "radius"
    )]
    sigma: f64,
    /// Strength of the sharpening: the share of the blurred image that is
    /// subtracted. ImageJ suggests 0.1 to 0.9.
    #[param(
        label = "Mask weight",
        default = 0.6,
        min = 0,
        max = 0.99,
        ijm = "mask"
    )]
    weight: f64,
}

#[derive(Params)]
struct ShadowsParams {
    #[param(flatten)]
    scope: PlaneScope,
    /// The side the light comes from.
    #[param(
        default = "north",
        choices(
            "north",
            "northeast",
            "east",
            "southeast",
            "south",
            "southwest",
            "west",
            "northwest"
        )
    )]
    direction: String,
}

/// A spec for a command under Process > Filters that changes samples.
fn filter_spec(id: &str, title: &str, weight: f64, previewable: bool) -> CommandSpec {
    CommandSpec::new(id, title)
        .menu("Process > Filters", weight)
        .effects(Effects {
            image: ImageEffect::Modifies,
            ..Effects::default()
        })
        .flags(Flags {
            previewable,
            cancellable: true,
            ..Flags::default()
        })
}

/// The rectangle of a plane to filter.
fn frame(info: &PlaneInfo) -> Frame {
    Frame::new(info.width, info.height, info.rect)
}

/// Metadata of the image a scope acts on.
fn meta_of(ctx: &calipix_command::CommandCtx<'_>, input: DatasetId) -> Result<ImageMeta> {
    Ok(ctx.image(input)?.meta().clone())
}

/// Whether a channel is shown through an inverting lookup table, by the
/// test of ImageJ's `ImageProcessor.isInvertedLut`: no entry up to 254 is
/// brighter than the one before it. RGB images have no lookup table.
fn inverted_lut(meta: &ImageMeta, channel: usize) -> bool {
    fn inverting(lut: &Lut) -> bool {
        let level = |i: usize| lut.table[i].iter().map(|&v| v as u32).sum::<u32>();
        !(1..255).any(|i| level(i - 1) < level(i))
    }
    meta.color != ColorModel::Rgb
        && meta
            .channels
            .get(channel)
            .and_then(|c| c.lut.as_ref())
            .is_some_and(inverting)
}

/// Sigma in pixels along X and Y. With `scaled`, sigma is in the image's
/// unit and is divided by the pixel size, but only if the image has a
/// unit other than pixels, as in ImageJ's `GaussianBlur`.
fn sigma_in_pixels(meta: &ImageMeta, sigma: f64, scaled: bool) -> Result<(f64, f64)> {
    let (x, y) = (&meta.axes()[0], &meta.axes()[1]);
    let calibrated = x
        .unit
        .as_deref()
        .is_some_and(|unit| !matches!(unit.trim(), "" | "pixel" | "pixels"));
    if !(scaled && calibrated) {
        return Ok((sigma, sigma));
    }
    if !(x.scale > 0.0 && y.scale > 0.0) {
        return Err(Error::failed(format!(
            "the pixel size ({} x {}) must be positive to use scaled units",
            x.scale, y.scale
        )));
    }
    Ok((sigma / x.scale, sigma / y.scale))
}

fn rank_command(
    registry: &mut Registry,
    spec: CommandSpec,
    rank: Rank,
    median_limit: bool,
) -> Result<()> {
    let run = move |ctx: &mut calipix_command::CommandCtx<'_>, scope: PlaneScope, radius: f64| {
        scope.apply(ctx, "Filtering", |plane, info| {
            filter::rank_filter(plane, frame(info), radius, rank)
        })
    };
    if median_limit {
        registry.register(typed(spec, move |ctx, p: MedianRadiusParams| {
            run(ctx, p.scope, p.radius)
        }))
    } else {
        registry.register(typed(spec, move |ctx, p: RadiusParams| {
            run(ctx, p.scope, p.radius)
        }))
    }
}

pub(crate) fn register(registry: &mut Registry) -> Result<()> {
    registry.register(typed(
        filter_spec("filter.gaussian_blur", "Gaussian Blur", 10.0, true)
            .summary("Blurs an image with a Gaussian of a chosen width.")
            .doc(
                "Each sample becomes a weighted mean of its surroundings, the weights falling \
                 off as a Gaussian with standard deviation sigma. Larger values blur more. \
                 Samples beyond the edge of the image count as the nearest edge sample.",
            )
            .ijm_name("Gaussian Blur..."),
        |ctx, p: GaussianParams| {
            let meta = meta_of(ctx, p.scope.input)?;
            let (sigma_x, sigma_y) = sigma_in_pixels(&meta, p.sigma, p.scaled)?;
            p.scope.apply(ctx, "Blurring", |plane, info| {
                let accuracy = filter::gaussian_accuracy(plane.dtype());
                filter::gaussian_blur(plane, frame(info), sigma_x, sigma_y, accuracy)
            })
        },
    ))?;

    rank_command(
        registry,
        filter_spec("filter.median", "Median", 20.0, true)
            .summary("Replaces each sample with the median of its circular neighbourhood.")
            .doc(
                "Removes noise while keeping edges sharp. In float images, NaN neighbours \
                 are left out and NaN samples stay NaN.",
            )
            .ijm_name("Median..."),
        Rank::Median,
        true,
    )?;
    rank_command(
        registry,
        filter_spec("filter.mean", "Mean", 30.0, true)
            .summary("Replaces each sample with the mean of its circular neighbourhood.")
            .ijm_name("Mean..."),
        Rank::Mean,
        false,
    )?;
    rank_command(
        registry,
        filter_spec("filter.minimum", "Minimum", 40.0, true)
            .summary("Replaces each sample with the smallest in its circular neighbourhood.")
            .doc("Shrinks bright regions and grows dark ones (grayscale erosion).")
            .ijm_name("Minimum..."),
        Rank::Minimum,
        false,
    )?;
    rank_command(
        registry,
        filter_spec("filter.maximum", "Maximum", 50.0, true)
            .summary("Replaces each sample with the largest in its circular neighbourhood.")
            .doc("Grows bright regions and shrinks dark ones (grayscale dilation).")
            .ijm_name("Maximum..."),
        Rank::Maximum,
        false,
    )?;
    rank_command(
        registry,
        filter_spec("filter.variance", "Variance", 60.0, true)
            .summary("Replaces each sample with the variance of its circular neighbourhood.")
            .doc(
                "Highlights edges and texture. In integer images, results above the largest \
                 value of the pixel type are clipped to it.",
            )
            .ijm_name("Variance..."),
        Rank::Variance,
        false,
    )?;

    registry.register(typed(
        filter_spec("filter.top_hat", "Top Hat", 70.0, true)
            .summary("Removes the background, keeping structures smaller than a chosen radius.")
            .doc(
                "Estimates the background with a grayscale opening (a minimum filter followed \
                 by a maximum filter) and subtracts it. For a light background the closing is \
                 used, and in integer images the background comes out at the top of the range.",
            )
            .ijm_name("Top Hat..."),
        |ctx, p: TopHatParams| {
            let meta = meta_of(ctx, p.scope.input)?;
            p.scope.apply(ctx, "Filtering", |plane, info| {
                filter::top_hat(
                    plane,
                    frame(info),
                    p.radius,
                    p.light_background,
                    p.dont_subtract,
                    inverted_lut(&meta, info.channel),
                )
            })
        },
    ))?;

    registry.register(typed(
        filter_spec("filter.convolve", "Convolve", 100.0, true)
            .summary("Filters an image with a kernel of coefficients that you type in.")
            .doc(
                "Each sample becomes the sum of its neighbours multiplied by the matching \
                 coefficients of the kernel. Samples beyond the edge of the image count as \
                 the nearest edge sample.",
            )
            .ijm_name("Convolve..."),
        |ctx, p: ConvolveParams| {
            let kernel = Kernel::parse(&p.kernel).map_err(|e| Error::InvalidArg {
                param: "kernel".to_string(),
                reason: e.to_string(),
            })?;
            p.scope.apply(ctx, "Convolving", |plane, info| {
                filter::convolve(plane, frame(info), &kernel, p.normalize)
            })
        },
    ))?;

    registry.register(typed(
        filter_spec("filter.unsharp_mask", "Unsharp Mask", 110.0, true)
            .summary("Sharpens an image by subtracting a blurred copy of it.")
            .doc(
                "With b the image blurred by a Gaussian of the given sigma and w the mask \
                 weight, each sample becomes (sample - w * b) / (1 - w). Large structures \
                 keep their contrast while fine detail is strengthened.",
            )
            .ijm_name("Unsharp Mask..."),
        |ctx, p: UnsharpParams| {
            p.scope.apply(ctx, "Sharpening", |plane, info| {
                filter::unsharp_mask(plane, frame(info), p.sigma, p.weight)
            })
        },
    ))?;

    registry.register(typed(
        filter_spec("filter.smooth", "Smooth", 200.0, false)
            .summary(
                "Blurs an image slightly: each sample becomes the mean of its 3 x 3 neighbourhood.",
            )
            .ijm_name("Smooth"),
        |ctx, p: ScopeOnly| {
            p.scope.apply(ctx, "Smoothing", |plane, info| {
                filter::smooth(plane, frame(info))
            })
        },
    ))?;

    registry.register(typed(
        filter_spec("filter.sharpen", "Sharpen", 210.0, false)
            .summary("Strengthens fine detail with a 3 x 3 sharpening kernel.")
            .doc(
                "Each sample becomes twelve times itself minus its eight neighbours, divided \
                 by four. This also strengthens noise.",
            )
            .ijm_name("Sharpen"),
        |ctx, p: ScopeOnly| {
            let rgb = meta_of(ctx, p.scope.input)?.color == ColorModel::Rgb;
            p.scope.apply(ctx, "Sharpening", |plane, info| {
                filter::sharpen(plane, frame(info), rgb)
            })
        },
    ))?;

    registry.register(typed(
        filter_spec("filter.find_edges", "Find Edges", 220.0, false)
            .summary("Highlights sharp changes in intensity with a Sobel edge detector.")
            .doc(
                "Each sample becomes the size of the intensity gradient there: the square \
                 root of the sum of the squares of the horizontal and vertical derivatives.",
            )
            .ijm_name("Find Edges"),
        |ctx, p: ScopeOnly| {
            p.scope.apply(ctx, "Finding edges", |plane, info| {
                filter::find_edges(plane, frame(info))
            })
        },
    ))?;

    registry.register(typed(
        filter_spec("filter.shadows", "Shadows", 230.0, false)
            .summary("Embosses an image as if lit from one side.")
            .doc(
                "Convolves with a 3 x 3 kernel that brightens edges facing the light and \
                 darkens those facing away. ImageJ has one command per direction (Process > \
                 Shadows > North and so on).",
            ),
        |ctx, p: ShadowsParams| {
            let direction: Direction = p.direction.parse().map_err(|e| Error::InvalidArg {
                param: "direction".to_string(),
                reason: format!("{e}"),
            })?;
            let rgb = meta_of(ctx, p.scope.input)?.color == ColorModel::Rgb;
            p.scope.apply(ctx, "Embossing", |plane, info| {
                filter::shadows(plane, frame(info), direction, rgb)
            })
        },
    ))?;

    registry.register(typed(
        filter_spec("filter.despeckle", "Despeckle", 300.0, false)
            .summary("Removes isolated bright and dark samples with a 3 x 3 median filter.")
            .ijm_name("Despeckle"),
        |ctx, p: ScopeOnly| {
            p.scope.apply(ctx, "Despeckling", |plane, info| {
                filter::despeckle(plane, frame(info))
            })
        },
    ))?;

    registry.register(typed(
        filter_spec("filter.remove_outliers", "Remove Outliers", 310.0, true)
            .summary("Replaces samples that stand out from their surroundings by the local median.")
            .doc(
                "A sample is replaced by the median of its circular neighbourhood if it is \
                 brighter (or darker) than that median by more than the threshold. For an \
                 image shown through an inverting lookup table, bright and dark are as \
                 displayed.",
            )
            .ijm_name("Remove Outliers..."),
        |ctx, p: OutliersParams| {
            let which = match p.which.as_str() {
                "dark" => Outliers::Dark,
                _ => Outliers::Bright,
            };
            let meta = meta_of(ctx, p.scope.input)?;
            p.scope.apply(ctx, "Removing outliers", |plane, info| {
                filter::remove_outliers(
                    plane,
                    frame(info),
                    p.radius,
                    p.threshold,
                    which,
                    inverted_lut(&meta, info.channel),
                )
            })
        },
    ))?;

    registry.register(typed(
        filter_spec("filter.remove_nans", "Remove NaNs", 320.0, true)
            .summary("Replaces NaN samples of a float image by the median of their neighbours.")
            .doc(
                "Each NaN (not a number) becomes the median of the samples in its circular \
                 neighbourhood that are numbers, and stays NaN if there are none. Other \
                 samples are unchanged. Needs a float image.",
            )
            .ijm_name("Remove NaNs..."),
        |ctx, p: MedianRadiusParams| {
            p.scope.apply(ctx, "Removing NaNs", |plane, info| {
                filter::remove_nans(plane, frame(info), p.radius)
            })
        },
    ))?;

    Ok(())
}
