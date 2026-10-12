//! Filtering a plane in horizontal strips.

use calipix_core::{Pixel, Rect, Result};
use rayon::prelude::*;

use super::Frame;

/// One strip handed to a filter by [`filter_strips`]: some whole rows of
/// the plane, and the part of them to compute.
#[derive(Clone, Copy, Debug)]
pub struct Strip<'a, T> {
    /// The strip's rows with up to `halo` more above and below, full width.
    pub rows: &'a [T],
    /// Size of `rows`, and the rectangle to compute within them.
    pub frame: Frame,
    /// Plane row that the first row of `rows` is.
    pub first_row: usize,
}

/// Applies a neighbourhood filter to `frame.rect` of a plane strip by
/// strip, and returns the plane with that rectangle filtered.
///
/// The rectangle is cut into strips of `strip_rows` rows. For each,
/// `filter` is given those rows plus `halo` rows above and below (fewer at
/// the top and bottom of the plane) and must return the filtered samples of
/// the strip's rectangle, row by row. Strips run in parallel.
///
/// The result is the same as filtering the rectangle in one piece, for any
/// `strip_rows`, provided that
///
/// * a result depends on no row further than `halo` away,
/// * rows missing beyond the top or bottom of what the filter is given are
///   treated as copies of the nearest row, and
/// * a result does not depend on which row of the rectangle is first.
///
/// Strips span the full width because ImageJ's filters carry running sums
/// along each row from the left edge of the rectangle; cutting rows would
/// change floating-point rounding. Only the strip in work is converted to
/// float, so an 8-bit plane never needs a float copy of itself, and a
/// source that reads rows on demand could feed strips one at a time.
pub fn filter_strips<T, F>(
    src: &[T],
    frame: Frame,
    halo: usize,
    strip_rows: usize,
    filter: F,
) -> Result<Vec<T>>
where
    T: Pixel,
    F: Fn(Strip<'_, T>) -> Result<Vec<T>> + Sync,
{
    frame.check(src.len())?;
    let Frame {
        width,
        height,
        rect,
    } = frame;
    let strip_rows = strip_rows.max(1);
    let mut out = src.to_vec();
    out[rect.y * width..(rect.y + rect.h) * width]
        .par_chunks_mut(strip_rows * width)
        .enumerate()
        .try_for_each(|(i, target)| -> Result<()> {
            let y0 = rect.y + i * strip_rows;
            let rows = target.len() / width;
            let top = y0.saturating_sub(halo);
            let bottom = (y0 + rows).saturating_add(halo).min(height);
            let strip = Strip {
                rows: &src[top * width..bottom * width],
                frame: Frame::new(
                    width,
                    bottom - top,
                    Rect::new(rect.x, y0 - top, rect.w, rows),
                ),
                first_row: top,
            };
            let filtered = filter(strip)?;
            if filtered.len() != rect.w * rows {
                return Err(calipix_core::Error::Shape(format!(
                    "a filter returned {} samples for a strip of {} x {rows}",
                    filtered.len(),
                    rect.w
                )));
            }
            for (row, samples) in target.chunks_mut(width).zip(filtered.chunks(rect.w)) {
                row[rect.x..rect.x + rect.w].copy_from_slice(samples);
            }
            Ok(())
        })?;
    Ok(out)
}

/// A strip height that keeps the halo a small part of the work and gives
/// every thread several strips. Results do not depend on it.
pub fn default_strip_rows(halo: usize, rows: usize) -> usize {
    let per_thread = rows.div_ceil(rayon::current_num_threads().max(1) * 4);
    per_thread.max(2 * halo).max(16)
}
