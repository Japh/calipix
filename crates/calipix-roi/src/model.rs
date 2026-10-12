//! How ImageJ holds each kind of selection internally.
//!
//! ImageJ keeps a selection as an integer bounding rectangle plus
//! coordinates relative to it, in `int` or single-precision `float`
//! arrays. Masks, outlines, lengths and the file format are all computed
//! from that representation, so the rounding it introduces is part of the
//! results. This module rebuilds the representation from a [`Roi`]; the
//! other modules port ImageJ's algorithms on top of it.

use crate::{Point, Roi};

/// Java's `Float.MIN_VALUE`, the smallest positive `float`. ImageJ's
/// `FloatPolygon.calculateBounds` starts its maxima from it, so a polygon
/// lying wholly at negative coordinates gets bounds that reach to zero.
const JAVA_FLOAT_MIN_VALUE: f32 = f32::from_bits(1);

/// Java's `(int)Math.round(double)` for values in the `int` range.
pub(crate) fn jround(v: f64) -> i32 {
    (v + 0.5).floor() as i32
}

/// Java's `Roi.toIntR`: `(int)Math.floor(v + 0.5)` on a `float`.
pub(crate) fn to_int_r(v: f32) -> i32 {
    (f64::from(v) + 0.5).floor() as i32
}

/// A selection's integer bounding rectangle, ImageJ's `Roi.getBounds()`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Frame {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

/// Vertex coordinates relative to the selection's base point.
#[derive(Clone, Debug)]
pub(crate) enum Coords {
    Int(Vec<i32>, Vec<i32>),
    Float(Vec<f32>, Vec<f32>),
}

/// ImageJ's `PolygonRoi` state: polygons, freehand and traced outlines,
/// polylines, angles, points, ellipses and rotated rectangles.
#[derive(Clone, Debug)]
pub(crate) struct Poly {
    pub frame: Frame,
    /// `Roi.bounds`, present for sub-pixel selections.
    pub fbounds: Option<[f64; 4]>,
    pub coords: Coords,
}

impl Poly {
    /// `Roi.getXBase()`.
    pub fn xbase(&self) -> f64 {
        self.fbounds.map_or(f64::from(self.frame.x), |b| b[0])
    }

    /// `Roi.getYBase()`.
    pub fn ybase(&self) -> f64 {
        self.fbounds.map_or(f64::from(self.frame.y), |b| b[1])
    }

    pub fn len(&self) -> usize {
        match &self.coords {
            Coords::Int(x, _) => x.len(),
            Coords::Float(x, _) => x.len(),
        }
    }

    /// `PolygonRoi.getFloatPolygon()`: the vertices in image coordinates.
    pub fn float_polygon(&self) -> (Vec<f32>, Vec<f32>) {
        match &self.coords {
            Coords::Int(xp, yp) => (
                xp.iter()
                    .map(|&v| v.wrapping_add(self.frame.x) as f32)
                    .collect(),
                yp.iter()
                    .map(|&v| v.wrapping_add(self.frame.y) as f32)
                    .collect(),
            ),
            Coords::Float(xpf, ypf) => {
                let (xb, yb) = (self.xbase() as f32, self.ybase() as f32);
                (
                    xpf.iter().map(|&v| v + xb).collect(),
                    ypf.iter().map(|&v| v + yb).collect(),
                )
            }
        }
    }

    /// `PolygonRoi.getNonSplineCoordinates()`: integer vertices relative
    /// to the frame, as the file format stores them.
    pub fn relative_int(&self) -> (Vec<i32>, Vec<i32>) {
        match &self.coords {
            Coords::Int(xp, yp) => (xp.clone(), yp.clone()),
            Coords::Float(xpf, ypf) => (
                xpf.iter().map(|&v| to_int_r(v)).collect(),
                ypf.iter().map(|&v| to_int_r(v)).collect(),
            ),
        }
    }

    /// `PolygonRoi.getPolygon()`: integer vertices in image coordinates.
    pub fn int_polygon(&self) -> (Vec<i32>, Vec<i32>) {
        let (x, y) = self.relative_int();
        (
            x.iter().map(|v| v.wrapping_add(self.frame.x)).collect(),
            y.iter().map(|v| v.wrapping_add(self.frame.y)).collect(),
        )
    }

    /// True if the vertices are held as integers, or are floats that are
    /// all whole numbers.
    pub fn int_coords(&self) -> Option<(Vec<i32>, Vec<i32>)> {
        match &self.coords {
            Coords::Int(xp, yp) => Some((xp.clone(), yp.clone())),
            Coords::Float(..) => None,
        }
    }
}

fn is_whole(v: f64) -> bool {
    v == v.trunc() && v.abs() < 1.0e9
}

/// True if every coordinate is a whole number, in which case ImageJ's
/// macro functions build the integer form of the selection.
pub(crate) fn all_whole(points: &[Point]) -> bool {
    points.iter().all(|&(x, y)| is_whole(x) && is_whole(y))
}

/// `Roi.setIntBounds`: the integer rectangle enclosing sub-pixel bounds.
/// Lines and points use pixel-centre coordinates, areas pixel corners.
pub(crate) fn int_bounds(b: [f64; 4], line: bool) -> Frame {
    if line {
        let x = (b[0] + 0.5).floor() as i32;
        let y = (b[1] + 0.5).floor() as i32;
        Frame {
            x,
            y,
            w: ((b[0] + b[2] + 1.5).floor() as i32).wrapping_sub(x),
            h: ((b[1] + b[3] + 1.5).floor() as i32).wrapping_sub(y),
        }
    } else {
        let x = b[0].floor() as i32;
        let y = b[1].floor() as i32;
        Frame {
            x,
            y,
            w: ((b[0] + b[2]).ceil() as i32).wrapping_sub(x),
            h: ((b[1] + b[3]).ceil() as i32).wrapping_sub(y),
        }
    }
}

/// `FloatPolygon.calculateBounds`: `(minX, minY, maxX, maxY)`.
pub(crate) fn float_min_max(xs: &[f32], ys: &[f32]) -> (f32, f32, f32, f32) {
    let (mut min_x, mut min_y) = (f32::MAX, f32::MAX);
    let (mut max_x, mut max_y) = (JAVA_FLOAT_MIN_VALUE, JAVA_FLOAT_MIN_VALUE);
    for (&x, &y) in xs.iter().zip(ys) {
        // Java's Math.min and Math.max propagate NaN; coordinates here are
        // always finite.
        min_x = min_x.min(x);
        max_x = max_x.max(x);
        min_y = min_y.min(y);
        max_y = max_y.max(y);
    }
    (min_x, min_y, max_x, max_y)
}

/// The integer form of `PolygonRoi`: `PolygonRoi(int[], int[], n, type)`
/// followed by `finishPolygon`.
pub(crate) fn poly_int(xs: &[i32], ys: &[i32], line: bool) -> Poly {
    let min_x = xs.iter().copied().min().unwrap_or(0);
    let max_x = xs.iter().copied().max().unwrap_or(0);
    let min_y = ys.iter().copied().min().unwrap_or(0);
    let max_y = ys.iter().copied().max().unwrap_or(0);
    let extra = i32::from(line);
    Poly {
        frame: Frame {
            x: min_x,
            y: min_y,
            w: max_x.wrapping_sub(min_x).wrapping_add(extra),
            h: max_y.wrapping_sub(min_y).wrapping_add(extra),
        },
        fbounds: None,
        coords: Coords::Int(
            xs.iter().map(|v| v.wrapping_sub(min_x)).collect(),
            ys.iter().map(|v| v.wrapping_sub(min_y)).collect(),
        ),
    }
}

/// The sub-pixel form of `PolygonRoi`: `PolygonRoi(float[], float[], n,
/// type)` followed by `finishPolygon`.
pub(crate) fn poly_float(mut xs: Vec<f32>, mut ys: Vec<f32>, line: bool) -> Poly {
    if xs.is_empty() {
        return Poly {
            frame: Frame {
                x: 0,
                y: 0,
                w: 0,
                h: 0,
            },
            fbounds: Some([0.0; 4]),
            coords: Coords::Float(xs, ys),
        };
    }
    let (min_x, min_y, max_x, max_y) = float_min_max(&xs, &ys);
    let b = [
        f64::from(min_x),
        f64::from(min_y),
        f64::from(max_x - min_x),
        f64::from(max_y - min_y),
    ];
    for v in &mut xs {
        *v -= min_x;
    }
    for v in &mut ys {
        *v -= min_y;
    }
    Poly {
        frame: int_bounds(b, line),
        fbounds: Some(b),
        coords: Coords::Float(xs, ys),
    }
}

/// `EllipseRoi.makePolygonRelative` and its twin in `RotatedRectRoi`: the
/// frame is `FloatPolygon.getBounds()` and the vertices are kept relative
/// to its integer corner.
pub(crate) fn poly_relative(mut xs: Vec<f32>, mut ys: Vec<f32>) -> Poly {
    let (min_x, min_y, max_x, max_y) = float_min_max(&xs, &ys);
    let x = f64::from(min_x).floor() as i32;
    let y = f64::from(min_y).floor() as i32;
    let w = (f64::from(max_x - x as f32) + 0.5) as i32;
    let h = (f64::from(max_y - y as f32) + 0.5) as i32;
    for v in &mut xs {
        *v -= x as f32;
    }
    for v in &mut ys {
        *v -= y as f32;
    }
    Poly {
        frame: Frame { x, y, w, h },
        fbounds: None,
        coords: Coords::Float(xs, ys),
    }
}

/// A polygon-like selection built from points the way ImageJ's macro
/// functions build it: integer arrays when every coordinate is whole,
/// float arrays otherwise.
pub(crate) fn poly_from_points(points: &[Point], line: bool) -> Poly {
    if all_whole(points) {
        let xs: Vec<i32> = points.iter().map(|p| p.0 as i32).collect();
        let ys: Vec<i32> = points.iter().map(|p| p.1 as i32).collect();
        poly_int(&xs, &ys, line)
    } else {
        poly_float(
            points.iter().map(|p| p.0 as f32).collect(),
            points.iter().map(|p| p.1 as f32).collect(),
            line,
        )
    }
}

/// `EllipseRoi.makeEllipse`: the 72 vertices ImageJ approximates an
/// ellipse with, given the end points of its major axis.
pub(crate) fn ellipse_vertices(
    x1: f64,
    y1: f64,
    x2: f64,
    y2: f64,
    aspect_ratio: f64,
) -> (Vec<f32>, Vec<f32>) {
    const VERTICES: usize = 72;
    let aspect_ratio = aspect_ratio.clamp(0.0, 1.0);
    let center_x = (x1 + x2) / 2.0;
    let center_y = (y1 + y2) / 2.0;
    let dx = x2 - x1;
    let dy = y2 - y1;
    let major = (dx * dx + dy * dy).sqrt();
    let minor = major * aspect_ratio;
    let phi_b = dy.atan2(dx);
    let alpha = phi_b * 180.0 / std::f64::consts::PI;
    let mut xs = Vec::with_capacity(VERTICES);
    let mut ys = Vec::with_capacity(VERTICES);
    for i in 0..VERTICES {
        let degrees = i as f64 * 360.0 / VERTICES as f64;
        let beta1 = degrees / 180.0 * std::f64::consts::PI;
        let dx = beta1.cos() * major / 2.0;
        let dy = beta1.sin() * minor / 2.0;
        let beta2 = dy.atan2(dx);
        let rad = (dx * dx + dy * dy).sqrt();
        let beta3 = beta2 + alpha / 180.0 * std::f64::consts::PI;
        xs.push((center_x + beta3.cos() * rad) as f32);
        ys.push((center_y + beta3.sin() * rad) as f32);
    }
    (xs, ys)
}

/// `RotatedRectRoi.makeRectangle`: the four corners of a rectangle given
/// the end points of its centre line and its width.
pub(crate) fn rotated_rect_vertices(
    x1: f64,
    y1: f64,
    x2: f64,
    y2: f64,
    width: f64,
) -> (Vec<f32>, Vec<f32>) {
    let length = ((x2 - x1) * (x2 - x1) + (y2 - y1) * (y2 - y1)).sqrt();
    let wsa = 0.5 * width * (y2 - y1) / length;
    let wca = -0.5 * width * (x2 - x1) / length;
    (
        vec![
            (x1 + wsa) as f32,
            (x2 + wsa) as f32,
            (x2 - wsa) as f32,
            (x1 - wsa) as f32,
        ],
        vec![
            (y1 + wca) as f32,
            (y2 + wca) as f32,
            (y2 - wca) as f32,
            (y1 - wca) as f32,
        ],
    )
}

/// The frame of a rectangle or oval: `Roi(double x, double y, double
/// width, double height)` truncates the corner and rounds the size up,
/// and no selection is narrower than one pixel.
pub(crate) fn rect_frame(x: f64, y: f64, width: f64, height: f64) -> Frame {
    Frame {
        x: x as i32,
        y: y as i32,
        w: (width.ceil() as i32).max(1),
        h: (height.ceil() as i32).max(1),
    }
}

/// `Line.updateCoordinates`: the frame of a straight line.
pub(crate) fn line_frame(x1: f64, y1: f64, x2: f64, y2: f64) -> Frame {
    int_bounds(
        [x1.min(x2), y1.min(y2), (x2 - x1).abs(), (y2 - y1).abs()],
        true,
    )
}

impl Roi {
    /// The `PolygonRoi` behind this selection, for the kinds ImageJ
    /// implements as one.
    pub(crate) fn poly(&self) -> Option<Poly> {
        Some(match self {
            Roi::Polygon { points } | Roi::Freehand { points } | Roi::Traced { points } => {
                poly_from_points(points, false)
            }
            Roi::Polyline { points, .. } | Roi::Freeline { points, .. } => {
                poly_from_points(points, true)
            }
            Roi::Angle { points } => poly_from_points(points, true),
            // PointRoi converts integer coordinates to floats on creation.
            Roi::Points { points } => poly_float(
                points.iter().map(|p| p.0 as f32).collect(),
                points.iter().map(|p| p.1 as f32).collect(),
                true,
            ),
            Roi::Ellipse {
                x1,
                y1,
                x2,
                y2,
                aspect_ratio,
            } => {
                let (xs, ys) = ellipse_vertices(*x1, *y1, *x2, *y2, *aspect_ratio);
                poly_relative(xs, ys)
            }
            Roi::RotatedRectangle {
                x1,
                y1,
                x2,
                y2,
                width,
            } => {
                let (xs, ys) = rotated_rect_vertices(*x1, *y1, *x2, *y2, *width);
                poly_relative(xs, ys)
            }
            _ => return None,
        })
    }

    /// ImageJ's `Roi.getBounds()`.
    pub(crate) fn frame(&self) -> Frame {
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
            } => rect_frame(*x, *y, *width, *height),
            Roi::Line { x1, y1, x2, y2, .. } => line_frame(*x1, *y1, *x2, *y2),
            Roi::Composite { path } => path.frame(),
            _ => self.poly().expect("polygon-like selection").frame,
        }
    }
}
