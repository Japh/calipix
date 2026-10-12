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

#[cfg(test)]
mod tests {
    use super::*;

    /// Each sample plus the sample `reach` rows above it (or the top row).
    fn add_row_above(strip: Strip<'_, u32>, reach: usize) -> Vec<u32> {
        let Frame { width, rect: r, .. } = strip.frame;
        let mut out = Vec::new();
        for y in r.y..r.y + r.h {
            for x in r.x..r.x + r.w {
                let above = y.saturating_sub(reach);
                out.push(strip.rows[y * width + x] + strip.rows[above * width + x]);
            }
        }
        out
    }

    #[test]
    fn strips_of_any_height_give_the_same_plane() {
        let (w, h) = (5, 11);
        let src: Vec<u32> = (0..(w * h) as u32).map(|v| v * v).collect();
        let frame = Frame::new(w, h, Rect::new(1, 2, 3, 8));
        let whole = filter_strips(&src, frame, 3, 100, |s| Ok(add_row_above(s, 3))).unwrap();
        // Outside the rectangle nothing changes; inside, rows are summed.
        assert_eq!(whole[0..2 * w], src[0..2 * w]);
        assert_eq!(whole[2 * w], src[2 * w]);
        assert_eq!(whole[5 * w + 1], src[5 * w + 1] + src[2 * w + 1]);
        assert_eq!(whole[2 * w + 1], src[2 * w + 1] + src[1]);
        for rows in 1..10 {
            let tiled = filter_strips(&src, frame, 3, rows, |s| Ok(add_row_above(s, 3)));
            assert_eq!(tiled.unwrap(), whole, "strips of {rows}");
        }
        // Too small a halo shows: the filter reads a row it was not given.
        let short = filter_strips(&src, frame, 1, 2, |s| Ok(add_row_above(s, 3))).unwrap();
        assert_ne!(short, whole);
    }

    #[test]
    fn strips_are_given_their_place_in_the_plane() {
        let src = vec![0u8; 4 * 9];
        let frame = Frame::new(4, 9, Rect::new(0, 3, 4, 5));
        let out = filter_strips(&src, frame, 2, 2, |s| {
            assert_eq!(s.rows.len(), s.frame.width * s.frame.height);
            let first = (s.first_row + s.frame.rect.y) as u8;
            Ok((0..s.frame.rect.h)
                .flat_map(|j| [first + j as u8; 4])
                .collect())
        })
        .unwrap();
        let rows: Vec<u8> = out.chunks(4).map(|row| row[0]).collect();
        assert_eq!(rows, [0, 0, 0, 3, 4, 5, 6, 7, 0]);
    }

    #[test]
    fn mistakes_are_errors() {
        let src = vec![0u8; 12];
        let frame = Frame::whole(4, 3);
        assert!(filter_strips(&src, frame, 0, 1, |_| Ok(vec![0u8; 3])).is_err());
        assert!(filter_strips(&src, Frame::whole(5, 3), 0, 1, |s| Ok(s.rows.to_vec())).is_err());
        let outside = Frame::new(4, 3, Rect::new(2, 0, 3, 1));
        assert!(filter_strips(&src, outside, 0, 1, |s| Ok(s.rows.to_vec())).is_err());
        assert!(default_strip_rows(40, 10) >= 80);
    }
}
