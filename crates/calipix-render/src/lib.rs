//! CPU reference renderer: turns image planes into displayable RGBA.
//!
//! This is the definition of how an image should look. The GPU viewer must
//! agree with it, and it is what export, thumbnails and the software
//! fallback use. Nothing here changes sample values: lookup tables, display
//! ranges and channel visibility only change how they are shown.
//!
//! # From sample to colour
//!
//! 1. **Display range.** Each channel has a display range `(min, max)`:
//!    its [`ChannelMeta::display_range`](calipix_core::ChannelMeta), or a
//!    default ([`effective_display_range`]).
//! 2. **Level.** The sample becomes a display level from 0 to 255 by one of
//!    four formulas, chosen by pixel type ([`LevelRule`]): unsigned 8-bit,
//!    other integers, floats, and 8-bit RGB components. [`LevelMap`] is the
//!    reference implementation of all four.
//! 3. **Colour.** The level indexes the channel's 256-entry lookup table
//!    ([`channel_lut`]); for an RGB image the three levels are the colour.
//! 4. **Channels.** In composite mode the colours of all visible channels
//!    are added per component and clamped to 255 ([`ChannelMode`],
//!    [`Blend`]).
//! 5. **Scaling.** [`render_region`] draws any rectangle at any size.
//!
//! All of it follows ImageJ 1.x (`ByteProcessor`, `ShortProcessor`,
//! `FloatProcessor`, `ColorProcessor`, `CompositeImage`, `LutLoader`), as do
//! the histograms ([`plane_histogram`], [`stack_histogram`]) and the
//! automatic display ranges ([`auto_contrast`], [`enhance_contrast`]).

mod contrast;
mod histogram;
mod luts;
mod mapping;
mod render;

pub use contrast::{
    AutoContrast, DEFAULT_SATURATED, auto_contrast, auto_range, auto_threshold, enhance_contrast,
    reset_range,
};
pub use histogram::{
    Histogram, HistogramOptions, buffer_histogram, plane_histogram, rgb_to_gray, stack_histogram,
    value_counts,
};
pub use luts::{
    BUILTIN_LUTS, BuiltinLut, builtin_lut, builtin_luts, default_channel_lut, inverted_lut,
    lut_from_color,
};
pub use mapping::{LevelMap, LevelRule, default_display_range, to_levels};
pub use render::{
    Blend, ChannelMode, DisplayOptions, Reduction, Rgba8, channel_lut, effective_display_range,
    render_plane, render_plane_with, render_region, thumbnail, thumbnail_at,
};
