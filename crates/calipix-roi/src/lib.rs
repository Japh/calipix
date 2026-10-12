//! Selections, also called regions of interest: the part of an image a
//! command acts on or measures.
//!
//! A [`Roi`] is described in image coordinates, where pixel `(x, y)` covers
//! the square from `(x, y)` to `(x + 1, y + 1)`. Which pixels it covers,
//! and what its outline, perimeter and calipers measure, is decided the way
//! ImageJ 1.x decides it, so that results restricted to a selection match
//! ImageJ's. Lines and points are the exception to the coordinate rule, as
//! in ImageJ: their coordinates name pixel centres, so the line from
//! `(2, 3)` to `(8, 3)` runs through the middle of pixels 2 to 8 of row 3.
//!
//! # What to call
//!
//! * [`Roi::mask`] for the pixels an area selection covers; every
//!   measurement and every command confined to a selection uses it.
//! * [`Roi::outline`], [`Roi::length`], [`Roi::feret`],
//!   [`Roi::convex_hull`], [`Roi::contour_centroid`], [`Roi::angle`] for
//!   geometry as ImageJ reports it.
//! * [`Roi::line_pixels`] and [`Roi::profile_positions`] for the pixels
//!   along a straight line.
//! * [`decode`] and [`encode`] for ImageJ's `.roi` format, [`read_zip`]
//!   and [`write_zip`] for ROI Manager sets.
//!
//! # Text form
//!
//! Every selection has a one-line text form, used on the command line, in
//! macros, by agents and in the History panel. [`Roi`]'s `Display` writes
//! it and `FromStr` reads it back to an equal value.
//!
//! ```text
//! rectangle x y width height
//! rounded-rectangle x y width height corner-diameter
//! oval x y width height
//! polygon x,y x,y x,y ...            three or more vertices
//! freehand x,y x,y x,y ...
//! traced x,y x,y x,y ...
//! line x1 y1 x2 y2 [width w]
//! polyline x,y x,y ... [width w]     two or more vertices
//! freeline x,y x,y ... [width w]
//! angle x,y x,y x,y                  the middle point is the corner
//! points x,y ...                     one or more points
//! ellipse x1 y1 x2 y2 aspect-ratio   major axis end points; ratio 0 to 1
//! rotated-rectangle x1 y1 x2 y2 width
//! composite M x,y L x,y ... Z ...    see below
//! ```
//!
//! Numbers are decimal and may have fractions (`12.5`); words are
//! separated by spaces, and the two coordinates of a point by a comma
//! with no space. `width` after a line is its width in pixels and may be
//! left out when it is 1.
//!
//! A composite selection lists the steps of its outline, as in an SVG
//! path: `M x,y` starts a sub-path, `L x,y` draws a line, `Q x,y x,y` a
//! quadratic curve (control point, end point), `C x,y x,y x,y` a cubic
//! curve (two control points, end point) and `Z` closes the sub-path.
//! The selected area is what the outline encloses an odd number of times,
//! so `composite M 0,0 L 10,0 L 10,10 L 0,10 Z M 3,3 L 7,3 L 7,7 L 3,7 Z`
//! is a square with a square hole.

use std::fmt;

use calipix_core::Rect;

mod codec;
mod fill;
mod geometry;
mod mask;
mod model;
mod ops;
mod path;
mod text;
mod trace;
mod zip;

pub use codec::{ArrowHead, Color, OverlayOptions, RoiEntry, RoiProps, decode, decode_hex, encode};
pub use geometry::{Feret, draw_line_pixels, profile_pixel};
pub use ops::{Combine, EllipseFit};
pub use path::{Path, Segment};
pub use zip::{RoiSet, read_zip, write_zip};

/// A position in image coordinates, `(x, y)`.
pub type Point = (f64, f64);

#[derive(Clone, Debug, thiserror::Error, PartialEq)]
#[non_exhaustive]
pub enum Error {
    #[error("cannot read `{text}` as a selection: {reason}")]
    Parse { text: String, reason: String },
    /// The bytes are not a valid ImageJ selection or selection set.
    #[error("not a valid ImageJ selection: {0}")]
    Format(String),
    /// A valid ImageJ selection of a kind Calipix does not have.
    #[error("{0} selections are not supported")]
    Unsupported(String),
    /// A selection that cannot be written in ImageJ's format.
    #[error("cannot save this selection in ImageJ's format: {0}")]
    Encode(String),
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
///
/// Coordinates are `f64`, but ImageJ holds most selections in single
/// precision, so geometry is computed, and files are written, from the
/// nearest `f32` values.
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
    /// A rectangle whose corners are rounded by an ellipse
    /// `corner_diameter` pixels across.
    RoundedRectangle {
        x: f64,
        y: f64,
        width: f64,
        height: f64,
        corner_diameter: u32,
    },
    /// The ellipse inscribed in a rectangle.
    Oval {
        x: f64,
        y: f64,
        width: f64,
        height: f64,
    },
    /// A closed polygon.
    Polygon { points: Vec<Point> },
    /// A closed outline drawn freehand. Covers the same pixels as a
    /// polygon; its perimeter is measured after smoothing.
    Freehand { points: Vec<Point> },
    /// A closed outline running along pixel edges, as the wand tool and
    /// particle analysis make. Its perimeter counts corners as diagonals.
    Traced { points: Vec<Point> },
    /// A straight line between two pixel centres, `width` pixels wide.
    Line {
        x1: f64,
        y1: f64,
        x2: f64,
        y2: f64,
        width: f64,
    },
    /// A line of several straight segments.
    Polyline { points: Vec<Point>, width: f64 },
    /// A line drawn freehand; its length is measured after smoothing.
    Freeline { points: Vec<Point>, width: f64 },
    /// Two arms meeting at the middle point, for measuring an angle.
    Angle { points: [Point; 3] },
    /// One or more points, each naming a pixel centre.
    Points { points: Vec<Point> },
    /// An ellipse at any orientation, given by the end points of its
    /// major axis and the ratio of minor to major axis. ImageJ holds it
    /// as a polygon of 72 vertices.
    Ellipse {
        x1: f64,
        y1: f64,
        x2: f64,
        y2: f64,
        aspect_ratio: f64,
    },
    /// A rectangle at any orientation, given by the end points of its
    /// centre line and its width across it.
    RotatedRectangle {
        x1: f64,
        y1: f64,
        x2: f64,
        y2: f64,
        width: f64,
    },
    /// An area with several outlines, holes or curved edges: the result
    /// of combining or inverting selections.
    Composite { path: Path },
}

/// The kind of a selection, without its coordinates.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Kind {
    Rectangle,
    RoundedRectangle,
    Oval,
    Polygon,
    Freehand,
    Traced,
    Line,
    Polyline,
    Freeline,
    Angle,
    Points,
    Ellipse,
    RotatedRectangle,
    Composite,
}

impl Kind {
    /// The word that starts the text form of this kind of selection.
    pub fn name(self) -> &'static str {
        match self {
            Kind::Rectangle => "rectangle",
            Kind::RoundedRectangle => "rounded-rectangle",
            Kind::Oval => "oval",
            Kind::Polygon => "polygon",
            Kind::Freehand => "freehand",
            Kind::Traced => "traced",
            Kind::Line => "line",
            Kind::Polyline => "polyline",
            Kind::Freeline => "freeline",
            Kind::Angle => "angle",
            Kind::Points => "points",
            Kind::Ellipse => "ellipse",
            Kind::RotatedRectangle => "rotated-rectangle",
            Kind::Composite => "composite",
        }
    }
}

impl fmt::Display for Kind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
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

    pub fn rounded_rectangle(x: f64, y: f64, width: f64, height: f64, corner_diameter: u32) -> Roi {
        if corner_diameter == 0 {
            return Roi::rectangle(x, y, width, height);
        }
        Roi::RoundedRectangle {
            x,
            y,
            width,
            height,
            corner_diameter,
        }
    }

    pub fn oval(x: f64, y: f64, width: f64, height: f64) -> Roi {
        Roi::Oval {
            x,
            y,
            width,
            height,
        }
    }

    pub fn polygon(points: Vec<Point>) -> Roi {
        Roi::Polygon { points }
    }

    /// A straight line one pixel wide.
    pub fn line(x1: f64, y1: f64, x2: f64, y2: f64) -> Roi {
        Roi::Line {
            x1,
            y1,
            x2,
            y2,
            width: 1.0,
        }
    }

    /// A segmented line one pixel wide.
    pub fn polyline(points: Vec<Point>) -> Roi {
        Roi::Polyline { points, width: 1.0 }
    }

    pub fn point(x: f64, y: f64) -> Roi {
        Roi::Points {
            points: vec![(x, y)],
        }
    }

    pub fn points(points: Vec<Point>) -> Roi {
        Roi::Points { points }
    }

    pub fn ellipse(x1: f64, y1: f64, x2: f64, y2: f64, aspect_ratio: f64) -> Roi {
        Roi::Ellipse {
            x1,
            y1,
            x2,
            y2,
            aspect_ratio,
        }
    }

    pub fn rotated_rectangle(x1: f64, y1: f64, x2: f64, y2: f64, width: f64) -> Roi {
        Roi::RotatedRectangle {
            x1,
            y1,
            x2,
            y2,
            width,
        }
    }

    pub fn composite(path: Path) -> Roi {
        Roi::Composite { path }
    }

    /// The same line with another width. Other kinds are returned as
    /// they are.
    pub fn with_line_width(mut self, new_width: f64) -> Roi {
        match &mut self {
            Roi::Line { width, .. } | Roi::Polyline { width, .. } | Roi::Freeline { width, .. } => {
                *width = new_width
            }
            _ => {}
        }
        self
    }

    pub fn kind(&self) -> Kind {
        match self {
            Roi::Rectangle { .. } => Kind::Rectangle,
            Roi::RoundedRectangle { .. } => Kind::RoundedRectangle,
            Roi::Oval { .. } => Kind::Oval,
            Roi::Polygon { .. } => Kind::Polygon,
            Roi::Freehand { .. } => Kind::Freehand,
            Roi::Traced { .. } => Kind::Traced,
            Roi::Line { .. } => Kind::Line,
            Roi::Polyline { .. } => Kind::Polyline,
            Roi::Freeline { .. } => Kind::Freeline,
            Roi::Angle { .. } => Kind::Angle,
            Roi::Points { .. } => Kind::Points,
            Roi::Ellipse { .. } => Kind::Ellipse,
            Roi::RotatedRectangle { .. } => Kind::RotatedRectangle,
            Roi::Composite { .. } => Kind::Composite,
        }
    }

    /// The number ImageJ's macro function `selectionType()` returns:
    /// 0 rectangle, 1 oval, 2 polygon, 3 freehand, 4 traced, 5 straight
    /// line, 6 polyline, 7 freeline, 8 angle, 9 composite, 10 point.
    /// A rounded rectangle is 0, an ellipse 3 and a rotated rectangle 2,
    /// after the classes ImageJ builds them on.
    pub fn ij_type(&self) -> i32 {
        match self.kind() {
            Kind::Rectangle | Kind::RoundedRectangle => 0,
            Kind::Oval => 1,
            Kind::Polygon | Kind::RotatedRectangle => 2,
            Kind::Freehand | Kind::Ellipse => 3,
            Kind::Traced => 4,
            Kind::Line => 5,
            Kind::Polyline => 6,
            Kind::Freeline => 7,
            Kind::Angle => 8,
            Kind::Composite => 9,
            Kind::Points => 10,
        }
    }

    /// True if the selection encloses an area (rather than being a line or
    /// a set of points).
    pub fn is_area(&self) -> bool {
        !self.is_line() && !self.is_points()
    }

    /// True for straight, segmented and freehand lines and for angles.
    pub fn is_line(&self) -> bool {
        matches!(
            self,
            Roi::Line { .. } | Roi::Polyline { .. } | Roi::Freeline { .. } | Roi::Angle { .. }
        )
    }

    /// True for a point or multi-point selection.
    pub fn is_points(&self) -> bool {
        matches!(self, Roi::Points { .. })
    }

    /// The width of a line selection in pixels; 1 for an angle and 0 for
    /// selections that are not lines.
    pub fn line_width(&self) -> f64 {
        match self {
            Roi::Line { width, .. } | Roi::Polyline { width, .. } | Roi::Freeline { width, .. } => {
                *width
            }
            Roi::Angle { .. } => 1.0,
            _ => 0.0,
        }
    }

    /// The smallest rectangle containing the selection, as
    /// `(x, y, width, height)` in image coordinates: ImageJ's
    /// `Roi.getFloatBounds()`.
    ///
    /// Rectangles and ovals report the numbers they were given. Polygons
    /// with fractional vertices report the exact extent of their
    /// vertices; whole-number ones, ellipses, rotated rectangles and
    /// composites report [`int_bounds`](Self::int_bounds).
    pub fn bounds(&self) -> (f64, f64, f64, f64) {
        match self {
            Roi::Rectangle {
                x,
                y,
                width,
                height,
            }
            | Roi::RoundedRectangle {
                x,
                y,
                width,
                height,
                ..
            }
            | Roi::Oval {
                x,
                y,
                width,
                height,
            } => (*x, *y, *width, *height),
            Roi::Line { x1, y1, x2, y2, .. } => {
                (x1.min(*x2), y1.min(*y2), (x2 - x1).abs(), (y2 - y1).abs())
            }
            Roi::Composite { .. } => {
                let f = self.frame();
                (
                    f64::from(f.x),
                    f64::from(f.y),
                    f64::from(f.w),
                    f64::from(f.h),
                )
            }
            _ => {
                let poly = self.poly().expect("polygon-like selection");
                match poly.fbounds {
                    Some(b) => (b[0], b[1], b[2], b[3]),
                    None => {
                        let f = poly.frame;
                        (
                            f64::from(f.x),
                            f64::from(f.y),
                            f64::from(f.w),
                            f64::from(f.h),
                        )
                    }
                }
            }
        }
    }

    /// The bounding rectangle in whole pixels, as `(x, y, width, height)`:
    /// ImageJ's `Roi.getBounds()`, which is what the macro function
    /// `getSelectionBounds` returns and what a mask is laid out in.
    ///
    /// For lines and points, whose coordinates name pixel centres, it
    /// covers every pixel the selection touches.
    pub fn int_bounds(&self) -> (i32, i32, i32, i32) {
        let f = self.frame();
        (f.x, f.y, f.w, f.h)
    }

    /// True if [`mask`](Self::mask) would select every pixel of its
    /// rectangle, so that callers can skip masking.
    pub fn is_rectangular(&self) -> bool {
        matches!(self, Roi::Rectangle { .. })
    }

    /// The points of a point selection, the vertices of a polygon or
    /// line, or the two end points of a straight line, exactly as given.
    /// `None` for shapes defined by other parameters (see
    /// [`outline`](Self::outline) for those).
    pub fn vertices(&self) -> Option<Vec<Point>> {
        match self {
            Roi::Polygon { points }
            | Roi::Freehand { points }
            | Roi::Traced { points }
            | Roi::Points { points }
            | Roi::Polyline { points, .. }
            | Roi::Freeline { points, .. } => Some(points.clone()),
            Roi::Angle { points } => Some(points.to_vec()),
            Roi::Line { x1, y1, x2, y2, .. } => Some(vec![(*x1, *y1), (*x2, *y2)]),
            _ => None,
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
        // ImageJ truncates the corner of a sub-pixel rectangle and rounds
        // its size up, so even a tiny one covers a pixel.
        assert_eq!(
            Roi::rectangle(0.2, 0.2, 0.1, 0.1).pixel_bounds(10, 10),
            Some(Rect::new(0, 0, 1, 1))
        );
    }

    #[test]
    fn kinds_are_classified() {
        let line = Roi::line(1.0, 1.0, 5.0, 4.0);
        assert!(line.is_line() && !line.is_area() && !line.is_points());
        assert_eq!(line.mask(10, 10), None);
        assert_eq!(line.ij_type(), 5);
        let point = Roi::point(3.0, 4.0);
        assert!(point.is_points() && !point.is_area());
        let mask = point.mask(10, 10).unwrap();
        assert_eq!((mask.rect, mask.count()), (Rect::new(3, 4, 1, 1), 1));
        assert!(Roi::oval(0.0, 0.0, 4.0, 4.0).is_area());
        assert!(!Roi::oval(0.0, 0.0, 4.0, 4.0).is_rectangular());
    }
}
