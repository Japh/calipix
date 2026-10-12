//! How each open image is being looked at: which plane, which channels,
//! and a display range that is still being dragged.
//!
//! None of this is image data. The lookup table, display range and channel
//! mode belong to the image (they are saved with it and changed by
//! commands, so they can be undone); the plane on screen and which
//! channels are hidden belong to the person looking, like zoom. They live
//! here rather than in the viewer widget so that panels, the status bar
//! and commands all read one answer, and so that they can be tested
//! without a window.

use calipix_command::DatasetId;
use calipix_core::{AxisKind, ColorModel, Image, ImageMeta};
use calipix_render::ChannelMode;

/// View state of one open image.
#[derive(Clone, Debug, Default)]
pub struct ImageViewState {
    /// Index along each axis beyond X and Y, in axis order.
    pub position: Vec<usize>,
    /// Whether each channel is shown in composite mode. Channels beyond
    /// the end are visible.
    pub visible: Vec<bool>,
    /// The image the last Auto Contrast produced, and which press it was.
    /// Pressing again while that image is still the one shown stretches
    /// further, as ImageJ's Auto button does.
    pub(super) auto: Option<(Image, u32)>,
}

impl ImageViewState {
    /// The position, made to fit an image of this shape.
    pub fn position_in(&self, meta: &ImageMeta) -> Vec<usize> {
        let shape = meta.plane_shape();
        (0..shape.len())
            .map(|slot| {
                self.position
                    .get(slot)
                    .copied()
                    .unwrap_or(0)
                    .min(shape[slot] - 1)
            })
            .collect()
    }

    pub fn is_visible(&self, channel: usize) -> bool {
        self.visible.get(channel).copied().unwrap_or(true)
    }
}

/// A display range being dragged: shown at once, not yet in the history.
#[derive(Clone, Debug)]
pub(super) struct RangeGesture {
    pub id: DatasetId,
    /// The channel being changed, or `None` for all of them.
    pub channel: Option<usize>,
    /// The image the gesture started from.
    pub base: Image,
    /// `base` with the dragged range, which is what the viewer shows.
    pub shown: Image,
    pub range: (f64, f64),
    /// The command that makes it permanent has been started.
    pub committing: bool,
}

/// True if the channels of an image with this metadata are shown together.
pub fn shown_together(meta: &ImageMeta) -> bool {
    meta.color == ColorModel::Rgb
        || calipix_commands::view::channel_mode(meta) == ChannelMode::Composite
}

/// True if the image is one picture: a single plane, or the three channels
/// of an RGB image.
pub fn is_single_plane(meta: &ImageMeta) -> bool {
    let planes = if meta.color == ColorModel::Rgb { 3 } else { 1 };
    meta.plane_count() == planes
}

/// The channel a position selects, if the image has channels.
pub fn channel_at(meta: &ImageMeta, position: &[usize]) -> usize {
    meta.axis_index(&AxisKind::Channel)
        .and_then(|axis| position.get(axis - 2).copied())
        .unwrap_or(0)
}

/// `position` with its channel replaced.
pub fn with_channel(meta: &ImageMeta, position: &[usize], channel: usize) -> Vec<usize> {
    let mut position = position.to_vec();
    if let Some(axis) = meta.axis_index(&AxisKind::Channel)
        && let Some(slot) = position.get_mut(axis - 2)
    {
        *slot = channel;
    }
    position
}

/// The 1-based slice and frame a position selects.
pub fn slice_and_frame(meta: &ImageMeta, position: &[usize]) -> (i64, i64) {
    let at = |kind: AxisKind| {
        meta.axis_index(&kind)
            .and_then(|axis| position.get(axis - 2).copied())
            .unwrap_or(0) as i64
            + 1
    };
    (at(AxisKind::Z), at(AxisKind::Time))
}
