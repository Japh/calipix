//! Selections, also called regions of interest: the part of an image a
//! command acts on or measures.
//!
//! A [`Roi`] is described in image coordinates, where pixel `(x, y)` covers
//! the square from `(x, y)` to `(x + 1, y + 1)`. Which pixels it covers is
//! decided the way ImageJ decides it, so that results restricted to a
//! selection match ImageJ's.

use std::fmt;
use std::str::FromStr;

use calipix_core::Rect;

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum Error {
    #[error("cannot read `{text}` as a selection: {reason}")]
    Parse { text: String, reason: String },
}

/// The pixels a selection covers within its bounding rectangle.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mask {
    /// The rectangle the mask describes, in image pixels.
    pub rect: Rect,
    /// One byte per pixel of `rect`, row-major: 255 inside the selection,
    /// 0 outside.
    pub data: Vec<u8>,
}

impl Mask {
    /// True if the pixel at image coordinates `(x, y)` is selected.
    pub fn contains(&self, x: usize, y: usize) -> bool {
        let r = self.rect;
        x >= r.x
            && y >= r.y
            && x < r.x + r.w
            && y < r.y + r.h
            && self.data[(y - r.y) * r.w + (x - r.x)] != 0
    }

    /// Number of selected pixels.
    pub fn count(&self) -> usize {
        self.data.iter().filter(|&&v| v != 0).count()
    }
}

/// A selection.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub enum Roi {
    /// An axis-aligned rectangle with its top-left corner at `(x, y)`.
    Rectangle {
        x: f64,
        y: f64,
        width: f64,
        height: f64,
    },
}

impl Roi {
    pub fn rectangle(x: f64, y: f64, width: f64, height: f64) -> Roi {
        Roi::Rectangle {
            x,
            y,
            width,
            height,
        }
    }

    /// True if the selection encloses an area (rather than being a line or
    /// a set of points).
    pub fn is_area(&self) -> bool {
        match self {
            Roi::Rectangle { .. } => true,
        }
    }

    /// The smallest rectangle containing the selection, as
    /// `(x, y, width, height)` in image coordinates.
    pub fn bounds(&self) -> (f64, f64, f64, f64) {
        match *self {
            Roi::Rectangle {
                x,
                y,
                width,
                height,
            } => (x, y, width, height),
        }
    }

    /// The whole pixels of the selection's bounding rectangle that lie
    /// within a `width` x `height` image, or `None` if there are none.
    /// Edges are rounded to the nearest pixel boundary, as ImageJ does for
    /// integer selections.
    pub fn pixel_bounds(&self, width: usize, height: usize) -> Option<Rect> {
        let (x, y, w, h) = self.bounds();
        let clamp = |v: f64, max: usize| v.round().clamp(0.0, max as f64) as usize;
        let (x0, y0) = (clamp(x, width), clamp(y, height));
        let (x1, y1) = (clamp(x + w, width), clamp(y + h, height));
        (x1 > x0 && y1 > y0).then(|| Rect::new(x0, y0, x1 - x0, y1 - y0))
    }

    /// Which pixels of a `width` x `height` image the selection covers.
    ///
    /// Returns `None` when nothing is covered. For a rectangle every pixel
    /// of [`pixel_bounds`](Self::pixel_bounds) is selected.
    pub fn mask(&self, width: usize, height: usize) -> Option<Mask> {
        let rect = self.pixel_bounds(width, height)?;
        match self {
            Roi::Rectangle { .. } => Some(Mask {
                rect,
                data: vec![255; rect.w * rect.h],
            }),
        }
    }

    /// True if [`mask`](Self::mask) would select every pixel of its
    /// rectangle, so that callers can skip masking.
    pub fn is_rectangular(&self) -> bool {
        match self {
            Roi::Rectangle { .. } => true,
        }
    }
}

/// The text form used on the command line, in macros and by agents:
/// `rectangle x y width height`.
impl fmt::Display for Roi {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Roi::Rectangle {
                x,
                y,
                width,
                height,
            } => write!(f, "rectangle {x} {y} {width} {height}"),
        }
    }
}

impl FromStr for Roi {
    type Err = Error;

    fn from_str(text: &str) -> Result<Roi, Error> {
        let fail = |reason: &str| Error::Parse {
            text: text.to_string(),
            reason: reason.to_string(),
        };
        let mut words = text.split_whitespace();
        let kind = words.next().ok_or_else(|| fail("it is empty"))?;
        let numbers = words
            .map(|w| w.parse::<f64>().ok().filter(|n| n.is_finite()))
            .collect::<Option<Vec<f64>>>()
            .ok_or_else(|| fail("expected numbers after the shape"))?;
        match (kind, numbers.as_slice()) {
            ("rectangle", &[x, y, width, height]) => {
                if width > 0.0 && height > 0.0 {
                    Ok(Roi::rectangle(x, y, width, height))
                } else {
                    Err(fail("width and height must be positive"))
                }
            }
            ("rectangle", _) => Err(fail("a rectangle takes x, y, width and height")),
            _ => Err(fail("unknown shape")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rectangle_mask_and_bounds() {
        let roi = Roi::rectangle(2.0, 1.0, 3.0, 2.0);
        assert!(roi.is_area());
        assert_eq!(roi.pixel_bounds(10, 10), Some(Rect::new(2, 1, 3, 2)));
        let mask = roi.mask(10, 10).unwrap();
        assert_eq!(mask.count(), 6);
        assert!(mask.contains(4, 2));
        assert!(!mask.contains(5, 2));
        assert!(!mask.contains(1, 1));
    }

    #[test]
    fn selections_are_clipped_to_the_image() {
        let roi = Roi::rectangle(-2.0, 8.0, 5.0, 5.0);
        assert_eq!(roi.pixel_bounds(10, 10), Some(Rect::new(0, 8, 3, 2)));
        assert_eq!(Roi::rectangle(20.0, 0.0, 5.0, 5.0).mask(10, 10), None);
        assert_eq!(
            Roi::rectangle(0.2, 0.2, 0.1, 0.1).pixel_bounds(10, 10),
            None
        );
    }

    #[test]
    fn text_round_trips() {
        let roi = Roi::rectangle(1.5, 2.0, 30.0, 4.25);
        assert_eq!(roi.to_string(), "rectangle 1.5 2 30 4.25");
        assert_eq!(roi.to_string().parse::<Roi>(), Ok(roi));
        for bad in [
            "",
            "rectangle 1 2 3",
            "rectangle 1 2 0 4",
            "blob 1 2",
            "rectangle a b c d",
        ] {
            assert!(bad.parse::<Roi>().is_err(), "{bad}");
        }
    }
}
