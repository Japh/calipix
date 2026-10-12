//! What every command that changes samples has in common: which planes it
//! acts on, and confining its effect to the selection.
//!
//! A command embeds [`PlaneScope`] in its parameters with
//! `#[param(flatten)]` and calls [`PlaneScope::apply`] with a function
//! that processes one plane. The scope then follows ImageJ's rules:
//!
//! * Only the plane on view is processed unless the whole stack is asked
//!   for (ImageJ's "Process all N images?").
//! * The three channels of an RGB image are always processed together.
//! * With an area selection, samples outside it are left as they were.
//!   The function still sees the whole plane, and is told the selection's
//!   bounding rectangle, so that filters can read neighbours outside it.

use calipix_command::{CommandCtx, DatasetId, Error, Outputs, Params, Result};
use calipix_core::{AxisKind, Buffer, ColorModel, Image, ImageMeta, Pixel, Rect, dispatch};
use calipix_roi::{Mask, Roi};

use crate::view::plane_position;

/// Which planes of which image a command acts on.
#[derive(Params, Clone, Debug)]
pub struct PlaneScope {
    /// The image to change.
    #[param(label = "Image")]
    pub input: DatasetId,
    /// Channel to change, counted from 1; 0 changes every channel.
    #[param(default = 0, min = 0, advanced, no_persist, position = "channel")]
    pub channel: i64,
    /// Slice to change, counted from 1.
    #[param(default = 1, min = 1, advanced, no_persist, position = "slice")]
    pub slice: i64,
    /// Frame to change, counted from 1.
    #[param(default = 1, min = 1, advanced, no_persist, position = "frame")]
    pub frame: i64,
    /// Change every plane of the image, not only the one on view.
    #[param(
        label = "Whole stack",
        default = false,
        ijm = "stack",
        position = "stack"
    )]
    pub stack: bool,
}

/// What a plane-processing function is told about the plane it is given.
#[derive(Clone, Copy, Debug)]
pub struct PlaneInfo {
    /// Plane number within the image.
    pub plane: usize,
    /// Channel the plane belongs to, counted from 0.
    pub channel: usize,
    pub width: usize,
    pub height: usize,
    /// The part of the plane whose result will be kept: the selection's
    /// bounding rectangle, or the whole plane without a selection.
    pub rect: Rect,
}

impl PlaneScope {
    /// A scope covering every plane of `input`.
    pub fn whole(input: DatasetId) -> PlaneScope {
        PlaneScope {
            input,
            channel: 0,
            slice: 1,
            frame: 1,
            stack: true,
        }
    }

    /// The plane numbers in scope for an image with this metadata, in
    /// increasing order.
    pub fn planes(&self, meta: &ImageMeta) -> Result<Vec<usize>> {
        if self.stack {
            return Ok((0..meta.plane_count()).collect());
        }
        let count = meta.size(&AxisKind::Channel);
        let channels: Vec<usize> = if self.channel == 0 || meta.color == ColorModel::Rgb {
            (0..count).collect()
        } else if self.channel as u64 <= count as u64 {
            vec![self.channel as usize - 1]
        } else {
            return Err(Error::InvalidArg {
                param: "channel".to_string(),
                reason: format!("{} is outside 0 to {count}", self.channel),
            });
        };
        let mut planes = channels
            .into_iter()
            .map(|c| {
                let position = plane_position(meta, c, self.slice, self.frame)?;
                Ok(meta.plane_index(&position)?)
            })
            .collect::<Result<Vec<_>>>()?;
        planes.sort_unstable();
        planes.dedup();
        Ok(planes)
    }

    /// Replaces each plane in scope with what `process` makes of it,
    /// within the image's selection. `verb` describes the work for the
    /// progress display ("Blurring").
    ///
    /// `process` must return a plane of the same pixel type and size; a
    /// command that changes the pixel type converts the whole image
    /// instead.
    pub fn apply(
        &self,
        ctx: &mut CommandCtx<'_>,
        verb: &str,
        process: impl Fn(&Buffer, &PlaneInfo) -> calipix_core::Result<Buffer>,
    ) -> Result<Outputs> {
        let dataset = ctx.workspace().get(self.input)?;
        let image = dataset.image.clone();
        let meta = image.meta().clone();
        let (width, height) = (meta.width(), meta.height());
        let whole = Rect::new(0, 0, width, height);

        // Only an area selection confines processing; with a line or
        // points selected, ImageJ processes the whole image.
        let roi = dataset.roi.as_ref().filter(|roi| roi.is_area());
        let (rect, mask) = match roi {
            None => (whole, None),
            Some(roi) => {
                let mask = selection_mask(roi, width, height)?;
                let rect = mask.rect;
                (rect, (!roi.is_rectangular()).then_some(mask))
            }
        };

        let planes = self.planes(&meta)?;
        let mut done: Vec<(usize, Buffer)> = Vec::with_capacity(planes.len());
        for (step, &plane) in planes.iter().enumerate() {
            ctx.check_cancelled()?;
            ctx.progress(step as f64 / planes.len() as f64, verb);
            let original = image.read_plane(plane)?;
            let info = PlaneInfo {
                plane,
                channel: meta.plane_channel(plane)?,
                width,
                height,
                rect,
            };
            let processed = process(&original, &info)?;
            if processed.dtype() != original.dtype() || processed.len() != original.len() {
                return Err(Error::failed(format!(
                    "{verb} produced a {} plane of {} samples from a {} plane of {}",
                    processed.dtype(),
                    processed.len(),
                    original.dtype(),
                    original.len()
                )));
            }
            let kept = if rect == whole && mask.is_none() {
                processed
            } else {
                keep_within(&original, &processed, width, rect, mask.as_ref())?
            };
            done.push((plane, kept));
        }

        let result = if done.len() == meta.plane_count() {
            Image::from_planes(meta, done.into_iter().map(|(_, plane)| plane).collect())?
        } else {
            let mut result = image;
            for (plane, data) in done {
                result = result.with_plane(plane, data)?;
            }
            result
        };
        ctx.workspace_mut().set_image(self.input, result)?;
        Ok(Outputs::new())
    }
}

/// The pixels of a `width` x `height` image that the area selection `roi`
/// covers, or why it covers none: it lies outside the image, or it is
/// too small or too thin to contain the centre of any pixel.
pub(crate) fn selection_mask(roi: &Roi, width: usize, height: usize) -> Result<Mask> {
    roi.mask(width, height).ok_or_else(|| {
        if roi.pixel_bounds(width, height).is_none() {
            Error::failed("the selection is outside the image")
        } else {
            Error::failed(format!(
                "the selection covers no pixel: the {} is too small or too thin to contain \
                 the centre of a pixel",
                roi.kind()
            ))
        }
    })
}

/// `original` with the samples inside `rect` (and inside `mask`, if given)
/// taken from `processed`.
pub fn keep_within(
    original: &Buffer,
    processed: &Buffer,
    width: usize,
    rect: Rect,
    mask: Option<&Mask>,
) -> calipix_core::Result<Buffer> {
    fn merge<T: Pixel>(
        original: &[T],
        processed: &Buffer,
        width: usize,
        rect: Rect,
        mask: Option<&Mask>,
    ) -> calipix_core::Result<Buffer> {
        let processed = processed.try_slice::<T>()?;
        let mut out = original.to_vec();
        for row in 0..rect.h {
            let start = (rect.y + row) * width + rect.x;
            match mask {
                None => {
                    out[start..start + rect.w].copy_from_slice(&processed[start..start + rect.w])
                }
                Some(mask) => {
                    let selected = &mask.data[row * rect.w..(row + 1) * rect.w];
                    for (i, _) in selected.iter().enumerate().filter(|(_, m)| **m != 0) {
                        out[start + i] = processed[start + i];
                    }
                }
            }
        }
        Ok(Buffer::from_vec(out))
    }
    dispatch!(original, |o| merge(o, processed, width, rect, mask))
}
