//! Measurements of a selection's shape as ImageJ reports them: outline
//! coordinates, perimeter or length, convex hull, Feret diameters.

use std::f64::consts::PI;

use crate::model::{Coords, Frame, Poly, jround, to_int_r};
use crate::path::{self, FLATNESS, Path, Segment};
use crate::{Point, Roi};

/// Caliper measurements of a selection, ImageJ's `Roi.getFeretValues()`.
///
/// `max`, `min` and `x`, `y` are in calibrated units when computed with a
/// pixel size; the end points are always in pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Feret {
    /// The largest distance between two points of the outline ("Feret").
    pub max: f64,
    /// Direction of that diameter in degrees, 0 to 180, anticlockwise
    /// from the positive x axis ("FeretAngle").
    pub angle: f64,
    /// The smallest caliper width ("MinFeret").
    pub min: f64,
    /// Start of the largest diameter ("FeretX", "FeretY").
    pub x: f64,
    pub y: f64,
    /// End points of the largest diameter, in pixels.
    pub max_ends: [Point; 2],
    /// End points of the smallest caliper width, in pixels.
    pub min_ends: [Point; 2],
}

fn sqr(v: f64) -> f64 {
    v * v
}

/// `FloatPolygon.getConvexHull`: gift wrapping, anticlockwise on screen,
/// with collinear points dropped.
pub(crate) fn convex_hull(xpoints: &[f32], ypoints: &[f32]) -> (Vec<f32>, Vec<f32>) {
    let npoints = xpoints.len();
    let (mut xx, mut yy) = (Vec::new(), Vec::new());
    if npoints == 0 {
        return (xx, yy);
    }
    let smallest_y = ypoints.iter().copied().fold(f32::MAX, |a, b| if b < a { b } else { a });
    let mut smallest_x = f32::MAX;
    let mut p1 = 0;
    for i in 0..npoints {
        if ypoints[i] == smallest_y && xpoints[i] < smallest_x {
            smallest_x = xpoints[i];
            p1 = i;
        }
    }
    let pstart = p1;
    loop {
        let (x1, y1) = (f64::from(xpoints[p1]), f64::from(ypoints[p1]));
        let mut p2 = p1;
        let (mut x2, mut y2) = (0.0, 0.0);
        loop {
            p2 += 1;
            if p2 == npoints {
                p2 = 0;
            }
            if p2 == p1 {
                break;
            }
            x2 = f64::from(xpoints[p2]);
            y2 = f64::from(ypoints[p2]);
            if !(x2 == x1 && y2 == y1) {
                break;
            }
        }
        let mut p3 = p2 + 1;
        if p3 == npoints {
            p3 = 0;
        }
        if p2 != p1 {
            loop {
                let (x3, y3) = (f64::from(xpoints[p3]), f64::from(ypoints[p3]));
                let determinate = x1 * (y2 - y3) - y1 * (x2 - x3) + (y3 * x2 - y2 * x3);
                let mut collinear_and_further = false;
                if determinate == 0.0 && p3 != p2 {
                    let d2 = sqr(x2 - x1) + sqr(y2 - y1);
                    let d3 = sqr(x3 - x1) + sqr(y3 - y1);
                    collinear_and_further = d3 > d2;
                }
                if determinate > 0.0 || collinear_and_further {
                    x2 = x3;
                    y2 = y3;
                    p2 = p3;
                }
                p3 += 1;
                if p3 == npoints {
                    p3 = 0;
                }
                if p3 == p1 {
                    break;
                }
            }
        }
        xx.push(x1 as f32);
        yy.push(y1 as f32);
        if p2 == p1 {
            break;
        }
        p1 = p2;
        if xx.len() > 1 && xpoints[p1] == xx[0] && ypoints[p1] == yy[0] {
            break;
        }
        if p1 == pstart || xx.len() > npoints {
            break;
        }
    }
    (xx, yy)
}

/// `FloatPolygon.contains`: the even-odd rule, with points exactly on a
/// boundary counted as left of or above it.
pub(crate) fn polygon_contains(xs: &[f64], ys: &[f64], x: f64, y: f64) -> bool {
    let n = xs.len();
    let mut inside = false;
    let mut j = n.wrapping_sub(1);
    for i in 0..n {
        if ((ys[i] >= y) != (ys[j] >= y))
            && (x > (xs[j] - xs[i]) * (y - ys[i]) / (ys[j] - ys[i]) + xs[i])
        {
            inside = !inside;
        }
        j = i;
    }
    inside
}

/// `PolygonRoi.getLength(float[], float[], n, closeShape, imp)`.
fn length_f32(xs: &[f32], ys: &[f32], close: bool, pw: f64, ph: f64) -> f64 {
    let n = xs.len();
    if n < 2 {
        return 0.0;
    }
    let mut length = 0.0;
    for i in 0..n - 1 {
        length += (sqr(f64::from(xs[i + 1] - xs[i]) * pw) + sqr(f64::from(ys[i + 1] - ys[i]) * ph))
            .sqrt();
    }
    if close {
        length +=
            (sqr(f64::from(xs[0] - xs[n - 1]) * pw) + sqr(f64::from(ys[0] - ys[n - 1]) * ph)).sqrt();
    }
    length
}

/// `PolygonRoi.getLength(int[], int[], n, closeShape, imp)`.
fn length_i32(xs: &[i32], ys: &[i32], close: bool, pw: f64, ph: f64) -> f64 {
    let n = xs.len();
    if n < 2 {
        return 0.0;
    }
    let d = |a: i32, b: i32| f64::from(a.wrapping_sub(b));
    let mut length = 0.0;
    for i in 0..n - 1 {
        length += (sqr(d(xs[i + 1], xs[i]) * pw) + sqr(d(ys[i + 1], ys[i]) * ph)).sqrt();
    }
    if close {
        length += (sqr(d(xs[0], xs[n - 1]) * pw) + sqr(d(ys[0], ys[n - 1]) * ph)).sqrt();
    }
    length
}

/// `PolygonRoi.getSmoothedLineLength` and `getSmoothedPerimeter`: the
/// length after a three-point running average, for freehand outlines.
fn smoothed_length(coords: &Coords, close: bool, pw: f64, ph: f64) -> f64 {
    let (w2, h2) = (pw * pw, ph * ph);
    let seg = |dx: f64, dy: f64| (dx * dx * w2 + dy * dy * h2).sqrt();
    let mut length = 0.0;
    match coords {
        Coords::Int(xp, yp) => {
            let n = xp.len();
            let f = |v: i32| f64::from(v);
            length += seg(
                f(xp[0] + xp[1] + xp[2]) / 3.0 - f(xp[0]),
                f(yp[0] + yp[1] + yp[2]) / 3.0 - f(yp[0]),
            );
            for i in 1..n - 2 {
                length += seg(f(xp[i + 2] - xp[i - 1]) / 3.0, f(yp[i + 2] - yp[i - 1]) / 3.0);
            }
            length += seg(
                f(xp[n - 1]) - f(xp[n - 3] + xp[n - 2] + xp[n - 1]) / 3.0,
                f(yp[n - 1]) - f(yp[n - 3] + yp[n - 2] + yp[n - 1]) / 3.0,
            );
            if close {
                length += seg(f(xp[n - 1] - xp[0]), f(yp[n - 1] - yp[0]));
            }
        }
        Coords::Float(xp, yp) => {
            let n = xp.len();
            let f = |v: f32| f64::from(v);
            length += seg(
                f(xp[0] + xp[1] + xp[2]) / 3.0 - f(xp[0]),
                f(yp[0] + yp[1] + yp[2]) / 3.0 - f(yp[0]),
            );
            for i in 1..n - 2 {
                length += seg(f(xp[i + 2] - xp[i - 1]) / 3.0, f(yp[i + 2] - yp[i - 1]) / 3.0);
            }
            length += seg(
                f(xp[n - 1]) - f(xp[n - 3] + xp[n - 2] + xp[n - 1]) / 3.0,
                f(yp[n - 1]) - f(yp[n - 3] + yp[n - 2] + yp[n - 1]) / 3.0,
            );
            if close {
                length += seg(f(xp[n - 1] - xp[0]), f(yp[n - 1] - yp[0]));
            }
        }
    }
    length
}

/// `PolygonRoi.getTracedPerimeter`: the perimeter of an outline that
/// follows pixel edges, counting corners as diagonal steps: the boundary
/// length less `2 - sqrt(2)` for each corner not adjacent to the last
/// one counted.
fn traced_perimeter(xp: &[i32], yp: &[i32], pw: f64, ph: f64) -> f64 {
    let n = xp.len();
    if n < 4 {
        return 0.0;
    }
    let (mut sumdx, mut sumdy, mut corners) = (0i64, 0i64, 0i64);
    let mut dx1 = xp[0] - xp[n - 1];
    let mut dy1 = yp[0] - yp[n - 1];
    let mut side1 = dx1.abs() + dy1.abs();
    let mut corner = false;
    for i in 0..n {
        let next = if i + 1 == n { 0 } else { i + 1 };
        let dx2 = xp[next] - xp[i];
        let dy2 = yp[next] - yp[i];
        sumdx += i64::from(dx1.abs());
        sumdy += i64::from(dy1.abs());
        let side2 = dx2.abs() + dy2.abs();
        if side1 > 1 || !corner {
            corner = true;
            corners += 1;
        } else {
            corner = false;
        }
        dx1 = dx2;
        dy1 = dy2;
        side1 = side2;
    }
    sumdx as f64 * pw + sumdy as f64 * ph
        - (corners as f64 * ((pw + ph) - (pw * pw + ph * ph).sqrt()))
}

/// `PolygonRoi.isTraced` for a polygon: every edge horizontal or vertical
/// between whole-number vertices.
fn rectilinear(poly: &Poly) -> Option<(Vec<i32>, Vec<i32>)> {
    let (xp, yp) = match &poly.coords {
        Coords::Int(xp, yp) => (xp.clone(), yp.clone()),
        Coords::Float(xpf, ypf) => {
            if xpf
                .iter()
                .chain(ypf)
                .any(|&v| v != (v as i32) as f32)
            {
                return None;
            }
            (
                xpf.iter().map(|&v| v as i32).collect(),
                ypf.iter().map(|&v| v as i32).collect(),
            )
        }
    };
    let n = xp.len();
    for i in 0..n {
        let next = if i + 1 == n { 0 } else { i + 1 };
        if xp[next] != xp[i] && yp[next] != yp[i] {
            return None;
        }
    }
    Some((xp, yp))
}

/// What `ShapeRoi.guessType` makes of one sub-path.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Guess {
    None,
    Curved,
    Line,
    Rectangle,
    Traced,
    Polygon,
    Polyline,
    Freehand,
    Freeline,
}

/// `ShapeRoi.guessType` with `forceAngle` off.
pub(crate) fn guess_type(
    segments: usize,
    polygon_length: f64,
    hv_int_only: bool,
    force_trace: bool,
    closed: bool,
) -> Guess {
    if polygon_length.is_nan() {
        return Guess::Curved;
    }
    let n = segments as f64;
    let long_edges = polygon_length / (n * n.sqrt()) >= 2.0;
    if segments < 2 {
        Guess::None
    } else if segments == 2 {
        if closed { Guess::None } else { Guess::Line }
    } else if segments == 4 && closed && hv_int_only && long_edges && !force_trace {
        Guess::Rectangle
    } else if closed && hv_int_only && (!long_edges || force_trace) {
        Guess::Traced
    } else if segments <= 10 || long_edges {
        if closed {
            Guess::Polygon
        } else {
            Guess::Polyline
        }
    } else if closed {
        Guess::Freehand
    } else {
        Guess::Freeline
    }
}

/// `ShapeRoi.createRoi`: the plain selection a sub-path stands for.
pub(crate) fn roi_from_guess(guess: Guess, xs: &[f32], ys: &[f32]) -> Option<Roi> {
    let points = || -> Vec<Point> {
        xs.iter()
            .zip(ys)
            .map(|(&x, &y)| (f64::from(x), f64::from(y)))
            .collect()
    };
    if xs.is_empty() {
        return None;
    }
    Some(match guess {
        Guess::None | Guess::Curved => return None,
        Guess::Line => Roi::Line {
            x1: f64::from(xs[0]),
            y1: f64::from(ys[0]),
            x2: f64::from(xs[1]),
            y2: f64::from(ys[1]),
            width: 1.0,
        },
        Guess::Rectangle => {
            let min = |v: &[f32]| v.iter().copied().fold(f32::INFINITY, f32::min);
            let max = |v: &[f32]| v.iter().copied().fold(f32::NEG_INFINITY, f32::max);
            let (x0, y0) = (f64::from(min(xs)) as i32, f64::from(min(ys)) as i32);
            let (x1, y1) = (f64::from(max(xs)) as i32, f64::from(max(ys)) as i32);
            Roi::rectangle(
                f64::from(x0),
                f64::from(y0),
                f64::from(x1 - x0),
                f64::from(y1 - y0),
            )
        }
        Guess::Traced => Roi::Traced {
            points: xs
                .iter()
                .zip(ys)
                .map(|(&x, &y)| (f64::from(to_int_r(x)), f64::from(to_int_r(y))))
                .collect(),
        },
        Guess::Polygon => Roi::Polygon { points: points() },
        Guess::Polyline => Roi::Polyline {
            points: points(),
            width: 1.0,
        },
        Guess::Freehand => Roi::Freehand { points: points() },
        Guess::Freeline => Roi::Freeline {
            points: points(),
            width: 1.0,
        },
    })
}

/// `ShapeRoi.getLength`: the summed perimeter of the sub-paths. A
/// sub-path of straight segments is measured as the plain selection it
/// would simplify to (so outlines along pixel edges get the corner
/// correction); curves are measured along the curve.
fn shape_length(path: &Path, pw: f64, ph: f64) -> f64 {
    let frame = path.frame();
    if frame.w == 0 && frame.h == 0 {
        return 0.0;
    }
    let (xbase, ybase) = (frame.x as f32, frame.y as f32);
    let local = path.local(frame);
    let segments = local.segments();
    if segments.is_empty() {
        return 0.0;
    }
    let mut xs: Vec<f32> = Vec::new();
    let mut ys: Vec<f32> = Vec::new();
    let mut sub_paths = 0;
    let mut hv_only = true;
    let mut closed = false;
    let (mut start_x, mut start_y) = (0.0f64, 0.0f64);
    let (mut last_x, mut last_y) = (0.0f64, 0.0f64);
    let mut path_length = 0.0f64;
    let mut total = 0.0f64;
    let mut uncal = 0.0f64;
    let mut index = 0;
    let mut done = false;
    loop {
        let segment = if done {
            None
        } else {
            let s = segments[index];
            index += 1;
            done = index == segments.len();
            Some(s)
        };
        let starts_new = matches!(segment, Some(Segment::MoveTo(..))) && !xs.is_empty();
        if segment.is_none() || starts_new {
            closed = closed
                || (!xs.is_empty() && xs[0] == xs[xs.len() - 1] && ys[0] == ys[ys.len() - 1]);
            let whole = |v: &[f32]| v.iter().all(|&c| c == (c as i32) as f32);
            if uncal.is_nan() || !whole(&xs) || !whole(&ys) {
                hv_only = false;
            }
            let force_trace = !done || sub_paths > 0;
            let guess = guess_type(xs.len(), uncal, hv_only, force_trace, closed);
            let absolute_x: Vec<f32> = xs.iter().map(|&v| v + xbase).collect();
            let absolute_y: Vec<f32> = ys.iter().map(|&v| v + ybase).collect();
            if let Some(roi) = roi_from_guess(guess, &absolute_x, &absolute_y)
                && !uncal.is_nan()
            {
                path_length = roi.length_scaled(pw, ph);
            }
            total += path_length;
        }
        let Some(segment) = segment else {
            return total;
        };
        closed = false;
        let d = f64::from;
        let mut end: Option<(f32, f32)> = None;
        let mut line_to: Option<(f32, f32)> = None;
        match segment {
            Segment::MoveTo(x, y) => {
                xs.clear();
                ys.clear();
                sub_paths += 1;
                path_length = 0.0;
                start_x = d(x) * pw;
                start_y = d(y) * ph;
                hv_only = true;
                end = Some((x, y));
            }
            Segment::LineTo(x, y) => {
                path_length += (sqr(last_y - d(y) * ph) + sqr(last_x - d(x) * pw)).sqrt();
                end = Some((x, y));
                line_to = end;
            }
            Segment::QuadTo(a, b, x, y) => {
                // ImageJ passes the control point's x and the end point's
                // x here where the control point's y and the end point's x
                // belong; the result is reproduced as it is.
                let c = [d(a) * pw, d(b) * ph, d(x) * pw, d(y) * ph];
                path_length += path::quad_length(&[last_x, last_y, c[0], c[2], c[2], c[3]]);
                uncal = f64::NAN;
                end = Some((x, y));
            }
            Segment::CubicTo(a, b, c, e, x, y) => {
                path_length += path::cubic_length(&[
                    last_x,
                    last_y,
                    d(a) * pw,
                    d(b) * ph,
                    d(c) * pw,
                    d(e) * ph,
                    d(x) * pw,
                    d(y) * ph,
                ]);
                uncal = f64::NAN;
                end = Some((x, y));
            }
            Segment::Close => {
                path_length += (sqr(last_x - start_x) + sqr(last_y - start_y)).sqrt();
                closed = true;
                if !xs.is_empty() {
                    line_to = Some((xs[0], ys[0]));
                }
            }
        }
        if let (Some((x, y)), false) = (line_to, xs.is_empty()) {
            let dx = x - xs[xs.len() - 1];
            let dy = y - ys[ys.len() - 1];
            uncal += (sqr(d(dx)) + sqr(d(dy))).sqrt();
            if dx != 0.0 && dy != 0.0 {
                hv_only = false;
            }
        }
        if let Some((x, y)) = end {
            xs.push(x);
            ys.push(y);
            last_x = d(x) * pw;
            last_y = d(y) * ph;
        }
    }
}

/// True if `(x, y)` is inside a rectangle with rounded corners, by the
/// definition of Java's `RoundRectangle2D.contains`.
fn round_rect_contains(frame: Frame, arc: f64, x: f64, y: f64) -> bool {
    let (mut x0, mut y0) = (f64::from(frame.x), f64::from(frame.y));
    let (x1, y1) = (x0 + f64::from(frame.w), y0 + f64::from(frame.h));
    if x < x0 || y < y0 || x >= x1 || y >= y1 {
        return false;
    }
    let aw = f64::from(frame.w).min(arc.abs()) / 2.0;
    let ah = f64::from(frame.h).min(arc.abs()) / 2.0;
    // Inside the cross formed by the straight sides: no corner to test.
    if x >= x0 + aw && x < x1 - aw {
        return true;
    }
    if y >= y0 + ah && y < y1 - ah {
        return true;
    }
    x0 = if x < x0 + aw { x0 + aw } else { x1 - aw };
    y0 = if y < y0 + ah { y0 + ah } else { y1 - ah };
    let (nx, ny) = ((x - x0) / aw, (y - y0) / ah);
    nx * nx + ny * ny <= 1.0
}

impl Roi {
    /// ImageJ's `Roi.getFloatPolygon()` for this kind of selection.
    pub(crate) fn float_polygon(&self) -> (Vec<f32>, Vec<f32>) {
        match self {
            Roi::Rectangle {
                x,
                y,
                width,
                height,
            } => (
                vec![*x as f32, (x + width) as f32, (x + width) as f32, *x as f32],
                vec![*y as f32, *y as f32, (y + height) as f32, (y + height) as f32],
            ),
            Roi::RoundedRectangle {
                corner_diameter, ..
            } => {
                // new ShapeRoi(this).getFloatPolygon(): the shape is a
                // RoundRectangle2D.Double at the origin, flattened in
                // double precision.
                let frame = self.frame();
                let steps = path::round_rect_path(
                    0.0,
                    0.0,
                    f64::from(frame.w),
                    f64::from(frame.h),
                    f64::from(*corner_diameter),
                );
                path::flatten_local(&steps, frame, FLATNESS, false, true)
            }
            Roi::Oval { .. } => {
                // new ShapeRoi(new Ellipse2D.Double(x-0.0004, y-0.0004,
                // width+0.0008, height+0.0008)).getFloatPolygon()
                let f = self.frame();
                let (ex, ey) = (f64::from(f.x) - 0.0004, f64::from(f.y) - 0.0004);
                let (ew, eh) = (f64::from(f.w) + 0.0008, f64::from(f.h) + 0.0008);
                let x0 = ex.floor() as i32;
                let y0 = ey.floor() as i32;
                let shape_frame = Frame {
                    x: x0,
                    y: y0,
                    w: ((ex + ew).ceil() as i32 - x0).max(1),
                    h: ((ey + eh).ceil() as i32 - y0).max(1),
                };
                let steps = path::ellipse_path(ex, ey, ew, eh);
                let local = path::to_path(&steps, -f64::from(x0), -f64::from(y0));
                path::flatten_local(&local.steps(), shape_frame, FLATNESS, false, true)
            }
            Roi::Line {
                x1,
                y1,
                x2,
                y2,
                width,
            } => {
                if *width <= 1.0 {
                    (vec![*x1 as f32, *x2 as f32], vec![*y1 as f32, *y2 as f32])
                } else {
                    // Line.getFloatPolygon(strokeWidth): the outline of
                    // the wide line, in area coordinates.
                    let (mut dx, mut dy) = (x2 - x1, y2 - y1);
                    let len = (dx * dx + dy * dy).sqrt();
                    dx *= 0.5 / len;
                    dy *= 0.5 / len;
                    let w = *width;
                    (
                        vec![
                            (x1 + 0.5 - dx + dy * w) as f32,
                            (x1 + 0.5 - dx - dy * w) as f32,
                            (x2 + 0.5 + dx - dy * w) as f32,
                            (x2 + 0.5 + dx + dy * w) as f32,
                        ],
                        vec![
                            (y1 + 0.5 - dy - dx * w) as f32,
                            (y1 + 0.5 - dy + dx * w) as f32,
                            (y2 + 0.5 + dy + dx * w) as f32,
                            (y2 + 0.5 + dy - dx * w) as f32,
                        ],
                    )
                }
            }
            Roi::Composite { path } => path.float_polygon(FLATNESS, false, false, true),
            _ => self.poly().expect("polygon-like selection").float_polygon(),
        }
    }

    /// The vertices of the selection's outline in image coordinates, as
    /// ImageJ's `Roi.getFloatPolygon()` and the macro function
    /// `getSelectionCoordinates` give them.
    ///
    /// Curved shapes are approximated by line segments to within 0.1
    /// pixel: an oval and a rounded rectangle the way ImageJ flattens
    /// them, a composite with all its sub-paths run together (see
    /// [`Path::polygons`] to keep them apart). A straight line gives its
    /// two end points, or the four corners of its outline if it is wider
    /// than one pixel. Coordinates have single precision, as in ImageJ.
    pub fn outline(&self) -> Vec<Point> {
        if let Roi::Line {
            x1, y1, x2, y2, ..
        } = self
        {
            // getSelectionCoordinates reports a line's end points in
            // double precision whatever its width.
            return vec![(*x1, *y1), (*x2, *y2)];
        }
        let (xs, ys) = self.float_polygon();
        xs.into_iter()
            .zip(ys)
            .map(|(x, y)| (f64::from(x), f64::from(y)))
            .collect()
    }

    /// The outline with whole-number vertices, ImageJ's
    /// `Roi.getPolygon()`. For an oval this is the boundary of its mask
    /// along pixel edges; otherwise the vertices of
    /// [`outline`](Self::outline) rounded to the nearest whole number.
    pub fn int_outline(&self) -> Vec<(i32, i32)> {
        match self {
            Roi::Oval { .. } => {
                let (frame, data) = self.full_mask();
                path::wand_outline(&data, frame.w, frame.h, frame.w / 2, frame.h / 2)
                    .into_iter()
                    .map(|(x, y)| (x + frame.x, y + frame.y))
                    .collect()
            }
            Roi::Rectangle { .. } | Roi::RoundedRectangle { .. } => {
                let f = self.frame();
                vec![
                    (f.x, f.y),
                    (f.x + f.w, f.y),
                    (f.x + f.w, f.y + f.h),
                    (f.x, f.y + f.h),
                ]
            }
            Roi::Line { .. } | Roi::Composite { .. } => {
                let (xs, ys) = self.float_polygon();
                xs.iter()
                    .zip(&ys)
                    .map(|(&x, &y)| (to_int_r(x), to_int_r(y)))
                    .collect()
            }
            _ => {
                let (xs, ys) = self.poly().expect("polygon-like selection").int_polygon();
                xs.into_iter().zip(ys).collect()
            }
        }
    }

    /// The perimeter of an area selection or the length of a line, in
    /// pixels: ImageJ's `Roi.getLength()` without calibration, which is
    /// what "Perimeter" and "Length" report for an uncalibrated image.
    pub fn length(&self) -> f64 {
        self.length_scaled(1.0, 1.0)
    }

    /// [`length`](Self::length) for pixels `pixel_width` by `pixel_height`
    /// units in size, as ImageJ computes it for a calibrated image.
    ///
    /// Per kind (ImageJ method in brackets):
    /// * rectangle: `2w + 2h` of the integer bounds; rounded corners
    ///   replace four corners by an ellipse (`Roi.getLength`);
    /// * oval: Ramanujan's approximation of the ellipse inscribed in the
    ///   integer bounds (`OvalRoi.getLength`);
    /// * traced selections, and polygons whose edges all run along pixel
    ///   edges: boundary length less `2 - sqrt(2)` per counted corner
    ///   (`PolygonRoi.getTracedPerimeter`); NaN for a traced selection
    ///   with fractional coordinates, as in ImageJ;
    /// * freehand areas and freehand lines: length after a three-point
    ///   running average (`getSmoothedPerimeter`,
    ///   `getSmoothedLineLength`);
    /// * other polygons, polylines, angles, ellipses, rotated rectangles:
    ///   the sum of the edge lengths;
    /// * straight line: distance between the end points
    ///   (`Line.getLength`);
    /// * composite: the sum over its sub-paths, each measured as the
    ///   plain selection it would simplify to (`ShapeRoi.getLength`).
    pub fn length_scaled(&self, pixel_width: f64, pixel_height: f64) -> f64 {
        let (pw, ph) = (pixel_width, pixel_height);
        match self {
            Roi::Rectangle { .. } | Roi::RoundedRectangle { .. } => {
                let f = self.frame();
                let mut perimeter = 2.0 * f64::from(f.w) * pw + 2.0 * f64::from(f.h) * ph;
                if let Roi::RoundedRectangle {
                    corner_diameter, ..
                } = self
                {
                    let d = f64::from(*corner_diameter);
                    let a = 0.5 * d.min(f64::from(f.w)) * pw;
                    let b = 0.5 * d.min(f64::from(f.h)) * ph;
                    perimeter += PI * (3.0 * (a + b) - ((3.0 * a + b) * (a + 3.0 * b)).sqrt())
                        - 4.0 * (a + b);
                }
                perimeter
            }
            Roi::Oval { .. } => {
                let f = self.frame();
                let a = 0.5 * f64::from(f.w) * pw;
                let b = 0.5 * f64::from(f.h) * ph;
                PI * (3.0 * (a + b) - ((3.0 * a + b) * (a + 3.0 * b)).sqrt())
            }
            Roi::Line { x1, y1, x2, y2, .. } => (sqr((x2 - x1) * pw) + sqr((y2 - y1) * ph)).sqrt(),
            Roi::Composite { path } => shape_length(path, pw, ph),
            Roi::Ellipse { .. } | Roi::RotatedRectangle { .. } => {
                let poly = self.poly().expect("polygon-like selection");
                match &poly.coords {
                    Coords::Float(xs, ys) => length_f32(xs, ys, true, pw, ph),
                    Coords::Int(xs, ys) => length_i32(xs, ys, true, pw, ph),
                }
            }
            _ => {
                let poly = self.poly().expect("polygon-like selection");
                let n = poly.len();
                let traced = match self {
                    Roi::Traced { .. } => Some(poly.int_coords()),
                    Roi::Polygon { .. } => rectilinear(&poly).map(Some),
                    _ => None,
                };
                if let Some(coords) = traced {
                    return match coords {
                        Some((xp, yp)) => traced_perimeter(&xp, &yp, pw, ph),
                        None => f64::NAN,
                    };
                }
                if n > 2 {
                    if matches!(self, Roi::Freehand { .. }) {
                        return smoothed_length(&poly.coords, true, pw, ph);
                    }
                    if matches!(self, Roi::Freeline { .. })
                        && !(poly.frame.w == 0 || poly.frame.h == 0)
                    {
                        return smoothed_length(&poly.coords, false, pw, ph);
                    }
                }
                let close = self.is_area();
                match &poly.coords {
                    Coords::Float(xs, ys) => length_f32(xs, ys, close, pw, ph),
                    Coords::Int(xs, ys) => length_i32(xs, ys, close, pw, ph),
                }
            }
        }
    }

    /// `Roi.getFloatPolygon("")`: the outline without a repeated closing
    /// vertex.
    fn open_float_polygon(&self) -> (Vec<f32>, Vec<f32>) {
        let (mut xs, mut ys) = self.float_polygon();
        let n = xs.len();
        if !matches!(self, Roi::Composite { .. })
            && self.is_area()
            && n > 1
            && xs[0] == xs[n - 1]
            && ys[0] == ys[n - 1]
        {
            xs.pop();
            ys.pop();
        }
        (xs, ys)
    }

    /// The convex hull of the outline, anticlockwise on screen starting
    /// from the topmost (then leftmost) vertex: ImageJ's
    /// `Roi.getFloatConvexHull()`.
    pub fn convex_hull(&self) -> Vec<Point> {
        let (xs, ys) = self.open_float_polygon();
        let (hx, hy) = convex_hull(&xs, &ys);
        hx.into_iter()
            .zip(hy)
            .map(|(x, y)| (f64::from(x), f64::from(y)))
            .collect()
    }

    /// Feret diameters in pixels: ImageJ's `Roi.getFeretValues()`.
    /// `None` if the selection has no outline.
    pub fn feret(&self) -> Option<Feret> {
        self.feret_scaled(1.0, 1.0)
    }

    /// [`feret`](Self::feret) for pixels `pixel_width` by `pixel_height`
    /// units in size.
    ///
    /// The largest diameter is the largest distance between two vertices
    /// of the convex hull; the smallest is the least width of the hull
    /// between parallel lines. An oval uses the axes of its bounds
    /// (`OvalRoi.getFeretValues`) and an ellipse its own axes when pixels
    /// are square (`EllipseRoi.getFeretValues`).
    pub fn feret_scaled(&self, pixel_width: f64, pixel_height: f64) -> Option<Feret> {
        let (pw, ph) = (pixel_width, pixel_height);
        if let Roi::Oval { .. } = self {
            let f = self.frame();
            let (x, y, w, h) = (
                f64::from(f.x),
                f64::from(f.y),
                f64::from(f.w),
                f64::from(f.h),
            );
            let high = ph * h > pw * w;
            let sel = |a: f64, b: f64| if high { a } else { b };
            let start = (x + sel(0.5 * w, 0.0), y + sel(h, 0.5 * h));
            return Some(Feret {
                max: sel(h * ph, w * pw),
                angle: sel(90.0, 0.0),
                min: sel(w * pw, h * ph),
                x: start.0 * pw,
                y: start.1 * ph,
                max_ends: [start, (x + sel(0.5 * w, w), y + sel(0.0, 0.5 * h))],
                min_ends: [
                    (x + sel(0.0, 0.5 * w), y + sel(0.5 * h, h)),
                    (x + sel(w, 0.5 * w), y + sel(0.5 * h, 0.0)),
                ],
            });
        }
        let (xs, ys) = self.open_float_polygon();
        let (hx, hy) = convex_hull(&xs, &ys);
        let n = hx.len();
        if n == 0 {
            return None;
        }
        let px = |i: usize| f64::from(hx[i]);
        let py = |i: usize| f64::from(hy[i]);
        let (mut p1, mut p2) = (0, 0);
        let mut diameter_sq = 0.0;
        for i in 0..n {
            for j in i + 1..n {
                let dx = (px(i) - px(j)) * pw;
                let dy = (py(i) - py(j)) * ph;
                let d = dx * dx + dy * dy;
                if d > diameter_sq {
                    diameter_sq = d;
                    p1 = i;
                    p2 = j;
                }
            }
        }
        if px(p1) > px(p2) {
            std::mem::swap(&mut p1, &mut p2);
        }
        let (xf1, yf1, xf2, yf2) = (px(p1), py(p1), px(p2), py(p2));
        let mut angle = (180.0 / PI) * ((yf1 - yf2) * ph).atan2((xf2 - xf1) * pw);
        if angle < 0.0 {
            angle += 180.0;
        }
        // Smallest caliper width: for each side of the hull, the largest
        // distance of any vertex from it; the least of those.
        let (mut x0, mut y0) = (px(n - 1), py(n - 1));
        let mut min_feret = f64::MAX;
        let mut ends = [0.0f64; 4];
        let mut candidate = [0.0f64; 4];
        for i in 0..n {
            let (xprev, yprev) = (x0, y0);
            x0 = px(i);
            y0 = py(i);
            let mut xnorm = (y0 - yprev) * ph;
            let mut ynorm = (xprev - x0) * pw;
            let factor = 1.0 / (xnorm * xnorm + ynorm * ynorm).sqrt();
            xnorm *= factor * pw;
            ynorm *= factor * ph;
            let mut max_dist = 0.0;
            for j in 0..n {
                let (x1, y1) = (px(j), py(j));
                let dist = (x1 - x0) * xnorm + (y1 - y0) * ynorm;
                if dist > max_dist {
                    max_dist = dist;
                    candidate = [
                        x1,
                        y1,
                        x1 - (xnorm / pw * dist) / pw,
                        y1 - (ynorm / ph * dist) / ph,
                    ];
                }
            }
            if max_dist < min_feret {
                min_feret = max_dist;
                ends = candidate;
            }
        }
        let mut feret = Feret {
            max: diameter_sq.sqrt(),
            angle,
            min: min_feret,
            x: xf1,
            y: yf1,
            max_ends: [(xf1, yf1), (xf2, yf2)],
            min_ends: [(ends[0], ends[1]), (ends[2], ends[3])],
        };
        if let (Roi::Ellipse { aspect_ratio, .. }, true) = (self, pw == ph) {
            // EllipseRoi.getFeretValues: the axes are known exactly. The
            // parameters are read back from the stored vertices, as
            // EllipseRoi.getParams does.
            let p = self.axis_params().expect("an ellipse has axis end points");
            let ratio = aspect_ratio.clamp(0.0, 1.0);
            let (dx, dy) = (p[2] - p[0], p[3] - p[1]);
            let major = (dx * dx + dy * dy).sqrt();
            feret.max = major * pw;
            feret.min = major * ratio * pw;
            feret.max_ends = [(p[0], p[1]), (p[2], p[3])];
            let (xc, yc) = (0.5 * (p[2] + p[0]), 0.5 * (p[3] + p[1]));
            let (sx, sy) = (dy * 0.5 * ratio, dx * (-0.5) * ratio);
            feret.min_ends = [(xc + sx, yc + sy), (xc - sx, yc - sy)];
        }
        Some(feret)
    }

    /// The centre of the selection's outline, each edge weighted by its
    /// length: ImageJ's `Roi.getContourCentroid()`. `None` if the outline
    /// has no length.
    pub fn contour_centroid(&self) -> Option<Point> {
        let (xs, ys) = self.float_polygon();
        let n = xs.len();
        if n == 0 {
            return None;
        }
        let (mut xc, mut yc, mut sum) = (0.0, 0.0, 0.0);
        let mut n2 = n - 1;
        for n1 in 0..n {
            let dx = f64::from(xs[n1]) - f64::from(xs[n2]);
            let dy = f64::from(ys[n1]) - f64::from(ys[n2]);
            let x = f64::from(xs[n2]) + dx / 2.0;
            let y = f64::from(ys[n2]) + dy / 2.0;
            let l = (dx * dx + dy * dy).sqrt();
            xc += x * l;
            yc += y * l;
            sum += l;
            n2 = n1;
        }
        (sum > 0.0).then(|| (xc / sum, yc / sum))
    }

    /// The angle a selection reports, in degrees:
    /// * straight line: its direction, anticlockwise from the positive x
    ///   axis, -180 to 180 (`Line.getAngle`);
    /// * angle selection: the angle between its two arms, 0 to 180
    ///   (`PolygonRoi.getAngle`).
    ///
    /// `None` for other kinds.
    pub fn angle(&self) -> Option<f64> {
        let direction =
            |a: Point, b: Point| (180.0 / PI) * (a.1 - b.1).atan2(b.0 - a.0);
        match self {
            Roi::Line { x1, y1, x2, y2, .. } => Some(direction((*x1, *y1), (*x2, *y2))),
            Roi::Angle { .. } => {
                // PolygonRoi.getAngleAsString works on the coordinates
                // relative to the selection's base, in single precision
                // for sub-pixel selections.
                let poly = self.poly()?;
                let p: Vec<Point> = match &poly.coords {
                    Coords::Int(x, y) => (0..3).map(|i| (f64::from(x[i]), f64::from(y[i]))).collect(),
                    Coords::Float(x, y) => {
                        (0..3).map(|i| (f64::from(x[i]), f64::from(y[i]))).collect()
                    }
                };
                let angle1 = direction(p[0], p[1]);
                let angle2 = direction(p[1], p[2]);
                let mut degrees = (180.0 - (angle1 - angle2).abs()).abs();
                if degrees > 180.0 {
                    degrees = 360.0 - degrees;
                }
                Some(degrees)
            }
            _ => None,
        }
    }

    /// True if ImageJ counts pixel `(x, y)` as inside the selection:
    /// `Roi.contains(int, int)` and its overrides.
    ///
    /// This is the point test ImageJ uses interactively. It agrees with
    /// [`mask`](Self::mask) except possibly for pixels whose centre lies
    /// within rounding error of the outline; use the mask when results
    /// must match ImageJ's measurements.
    pub fn contains(&self, x: i32, y: i32) -> bool {
        let f = self.frame();
        let in_frame = i64::from(x) >= i64::from(f.x)
            && i64::from(y) >= i64::from(f.y)
            && i64::from(x) < i64::from(f.x) + i64::from(f.w)
            && i64::from(y) < i64::from(f.y) + i64::from(f.h);
        match self {
            Roi::Rectangle { .. } => in_frame,
            Roi::RoundedRectangle {
                corner_diameter, ..
            } => {
                in_frame
                    && round_rect_contains(
                        f,
                        f64::from(*corner_diameter),
                        f64::from(x) + 0.4999,
                        f64::from(y) + 0.4999,
                    )
            }
            Roi::Oval { .. } => {
                let a = f64::from(f.w) * 0.5;
                let b = f64::from(f.h) * 0.5;
                let dx = f64::from(x) - (f64::from(f.x) + a - 0.5);
                let dy = f64::from(y) - (f64::from(f.y) + b - 0.5);
                (dx * dx) / (a * a) + (dy * dy) / (b * b) <= 1.0
            }
            Roi::Line {
                x1,
                y1,
                x2,
                y2,
                width,
            } => {
                if *width <= 1.0 {
                    return false;
                }
                if (x == *x1 as i32 && y == *y1 as i32) || (x == *x2 as i32 && y == *y2 as i32) {
                    return true;
                }
                let (xs, ys) = self.float_polygon();
                let xs: Vec<f64> = xs.iter().map(|&v| f64::from(to_int_r(v))).collect();
                let ys: Vec<f64> = ys.iter().map(|&v| f64::from(to_int_r(v))).collect();
                polygon_contains(&xs, &ys, f64::from(x), f64::from(y))
            }
            Roi::Composite { path } => {
                // shape.contains(x - this.x + 0.494, y - this.y + 0.49994)
                let (xs, ys) = path.float_polygon(path::FILL_FLATNESS, true, true, false);
                let (px, py) = (
                    f64::from(x) - f64::from(f.x) + 0.494,
                    f64::from(y) - f64::from(f.y) + 0.49994,
                );
                let mut inside = false;
                let mut start = 0;
                for end in 0..=xs.len() {
                    if end == xs.len() || xs[end].is_nan() {
                        let sx: Vec<f64> = xs[start..end].iter().map(|&v| f64::from(v)).collect();
                        let sy: Vec<f64> = ys[start..end].iter().map(|&v| f64::from(v)).collect();
                        inside ^= polygon_contains(&sx, &sy, px, py);
                        start = end + 1;
                    }
                }
                inside
            }
            Roi::Points { .. } => {
                let poly = self.poly().expect("points are polygon-like");
                match &poly.coords {
                    Coords::Float(xs, ys) => xs.iter().zip(ys).any(|(&px, &py)| {
                        x as f32 == f.x as f32 + px && y as f32 == f.y as f32 + py
                    }),
                    Coords::Int(..) => false,
                }
            }
            _ => {
                if !in_frame {
                    return false;
                }
                let poly = self.poly().expect("polygon-like selection");
                let (xs, ys): (Vec<f64>, Vec<f64>) = match &poly.coords {
                    Coords::Int(x, y) => (
                        x.iter().map(|&v| f64::from(v)).collect(),
                        y.iter().map(|&v| f64::from(v)).collect(),
                    ),
                    Coords::Float(x, y) => (
                        x.iter().map(|&v| f64::from(v)).collect(),
                        y.iter().map(|&v| f64::from(v)).collect(),
                    ),
                };
                polygon_contains(
                    &xs,
                    &ys,
                    f64::from(x) - poly.xbase() + 0.4999,
                    f64::from(y) - poly.ybase() + 0.5,
                )
            }
        }
    }

    /// The pixels a straight line passes through, each once, from its
    /// start to its end: ImageJ's `Line.PointIterator`, which is what
    /// iterating over a thin line selection visits.
    ///
    /// The line is sampled at `ceil(length)` equal steps and each sample
    /// `(x, y)` falls in pixel `(round(x - 0.5), round(y - 0.5))`.
    /// `None` unless this is a straight line.
    pub fn line_pixels(&self) -> Option<Vec<(i32, i32)>> {
        let Roi::Line { x1, y1, x2, y2, .. } = *self else {
            return None;
        };
        let (dx, dy) = (x2 - x1, y2 - y1);
        let n = (dx * dx + dy * dy).sqrt().ceil() as i64;
        let (xinc, yinc) = (dx / n as f64, dy / n as f64);
        let mut out: Vec<(i32, i32)> = Vec::new();
        for i in 0..=n.max(0) {
            let (x, y) = if i == 0 {
                (x1, y1)
            } else {
                (x1 + i as f64 * xinc, y1 + i as f64 * yinc)
            };
            let p = (jround(x - 0.5), jround(y - 0.5));
            if out.last() != Some(&p) {
                out.push(p);
            }
        }
        Some(out)
    }

    /// Where ImageJ samples an image along a straight line for a profile:
    /// the positions `ImageProcessor.getLine(x1, y1, x2, y2)` reads, which
    /// is what `Line.getPixels()` uses for a line one pixel wide.
    ///
    /// There are `round(length) + 1` positions, evenly spaced from the
    /// start to the end by repeated addition. Without interpolation
    /// ImageJ reads the pixel at each position rounded to the nearest
    /// whole number (see [`profile_pixel`]); with interpolation it
    /// interpolates at the position itself. `None` unless this is a
    /// straight line.
    pub fn profile_positions(&self) -> Option<Vec<Point>> {
        let Roi::Line { x1, y1, x2, y2, .. } = *self else {
            return None;
        };
        let (dx, dy) = (x2 - x1, y2 - y1);
        let n = jround((dx * dx + dy * dy).sqrt()).max(0);
        let (xinc, yinc) = if n > 0 {
            (dx / f64::from(n), dy / f64::from(n))
        } else {
            (0.0, 0.0)
        };
        let (mut rx, mut ry) = (x1, y1);
        let mut out = Vec::with_capacity(n as usize + 1);
        for _ in 0..=n {
            out.push((rx, ry));
            rx += xinc;
            ry += yinc;
        }
        Some(out)
    }
}

/// The pixel ImageJ reads at a profile position when not interpolating:
/// each coordinate rounded half up.
pub fn profile_pixel(position: Point) -> (i32, i32) {
    (jround(position.0), jround(position.1))
}

/// The pixels ImageJ sets when it draws a one-pixel-wide line between two
/// pixels: `ImageProcessor.lineTo`. Used to draw outlines and overlays.
///
/// The longer of the two extents is stepped one pixel at a time and the
/// other coordinate is rounded half up; both end pixels are included.
pub fn draw_line_pixels(x1: i32, y1: i32, x2: i32, y2: i32) -> Vec<(i32, i32)> {
    let dx = i64::from(x2) - i64::from(x1);
    let dy = i64::from(y2) - i64::from(y1);
    let n = dx.abs().max(dy.abs());
    let xinc = if dx != 0 { dx as f64 / n as f64 } else { 0.0 };
    let yinc = if dy != 0 { dy as f64 / n as f64 } else { 0.0 };
    let (mut x, mut y) = (f64::from(x1), f64::from(y1));
    let mut out = Vec::with_capacity(n as usize + 1);
    for _ in 0..=n {
        out.push((jround(x), jround(y)));
        x += xinc;
        y += yinc;
    }
    out
}
