//! Composite selections: outlines made of several sub-paths, possibly with
//! holes and curved segments, as ImageJ's `ShapeRoi` holds them.

use crate::model::Frame;

/// One step of a [`Path`], in image coordinates. The same five kinds as
/// Java's `PathIterator`, which is what ImageJ stores in `.roi` files.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Segment {
    /// Starts a new sub-path at a point.
    MoveTo(f32, f32),
    /// A straight line to a point.
    LineTo(f32, f32),
    /// A quadratic curve: control point, end point.
    QuadTo(f32, f32, f32, f32),
    /// A cubic curve: two control points, end point.
    CubicTo(f32, f32, f32, f32, f32, f32),
    /// Closes the current sub-path with a line to its start.
    Close,
}

/// The outline of a composite selection. Its area is everything enclosed
/// an odd number of times (the even-odd rule), so a sub-path inside
/// another one cuts a hole.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Path {
    segments: Vec<Segment>,
}

/// `ShapeRoi.FLATNESS`: how closely reported outlines follow curves.
pub(crate) const FLATNESS: f64 = 0.1;
/// `ShapeRoi.FILL_FLATNESS`: how closely masks follow curves.
pub(crate) const FILL_FLATNESS: f64 = 0.01;
/// `ShapeRoi.MAXERROR`: tolerance of curve lengths.
const MAX_ERROR: f64 = 1.0e-3;
/// Recursion limit of Java's `FlatteningPathIterator`.
const FLATTEN_LIMIT: u32 = 10;

impl Path {
    pub fn new(segments: Vec<Segment>) -> Path {
        Path { segments }
    }

    pub fn segments(&self) -> &[Segment] {
        &self.segments
    }

    pub fn is_empty(&self) -> bool {
        self.segments.is_empty()
    }

    /// A path of closed polygons, one per item.
    pub fn from_polygons<I, P>(polygons: I) -> Path
    where
        I: IntoIterator<Item = P>,
        P: IntoIterator<Item = (f32, f32)>,
    {
        let mut segments = Vec::new();
        for polygon in polygons {
            let mut first = true;
            for (x, y) in polygon {
                segments.push(if first {
                    Segment::MoveTo(x, y)
                } else {
                    Segment::LineTo(x, y)
                });
                first = false;
            }
            if !first {
                segments.push(Segment::Close);
            }
        }
        Path { segments }
    }

    /// The numbers ImageJ stores for the path: for each segment its type
    /// (0 move, 1 line, 2 quadratic, 3 cubic, 4 close) followed by its
    /// coordinates. `ShapeRoi.getShapeAsArray`.
    pub fn to_array(&self) -> Vec<f32> {
        let mut out = Vec::new();
        for s in &self.segments {
            match *s {
                Segment::MoveTo(x, y) => out.extend([0.0, x, y]),
                Segment::LineTo(x, y) => out.extend([1.0, x, y]),
                Segment::QuadTo(a, b, x, y) => out.extend([2.0, a, b, x, y]),
                Segment::CubicTo(a, b, c, d, x, y) => out.extend([3.0, a, b, c, d, x, y]),
                Segment::Close => out.push(4.0),
            }
        }
        out
    }

    /// Reads the numbers written by [`to_array`](Self::to_array).
    /// `ShapeRoi.makeShapeFromArray`; like it, stops at a segment cut
    /// short by the end of the array. Returns `None` for an unknown
    /// segment type.
    pub fn from_array(array: &[f32]) -> Option<Path> {
        let mut segments = Vec::new();
        let mut i = 0;
        while i < array.len() {
            let kind = array[i] as i32;
            let n = match kind {
                0 | 1 => 2,
                2 => 4,
                3 => 6,
                4 => 0,
                _ => return None,
            };
            if i + 1 + n > array.len() {
                break;
            }
            let c = &array[i + 1..i + 1 + n];
            segments.push(match kind {
                0 => Segment::MoveTo(c[0], c[1]),
                1 => Segment::LineTo(c[0], c[1]),
                2 => Segment::QuadTo(c[0], c[1], c[2], c[3]),
                3 => Segment::CubicTo(c[0], c[1], c[2], c[3], c[4], c[5]),
                _ => Segment::Close,
            });
            i += 1 + n;
        }
        Some(Path { segments })
    }

    /// True if the path has curved segments.
    pub fn has_curves(&self) -> bool {
        self.segments
            .iter()
            .any(|s| matches!(s, Segment::QuadTo(..) | Segment::CubicTo(..)))
    }

    /// The path moved by `(dx, dy)`.
    pub fn translated(&self, dx: f64, dy: f64) -> Path {
        self.mapped(|x, y| ((f64::from(x) + dx) as f32, (f64::from(y) + dy) as f32))
    }

    /// The path with every point (control points included) replaced.
    pub fn mapped(&self, f: impl Fn(f32, f32) -> (f32, f32)) -> Path {
        let segments = self
            .segments
            .iter()
            .map(|s| match *s {
                Segment::MoveTo(x, y) => {
                    let (x, y) = f(x, y);
                    Segment::MoveTo(x, y)
                }
                Segment::LineTo(x, y) => {
                    let (x, y) = f(x, y);
                    Segment::LineTo(x, y)
                }
                Segment::QuadTo(a, b, x, y) => {
                    let (a, b) = f(a, b);
                    let (x, y) = f(x, y);
                    Segment::QuadTo(a, b, x, y)
                }
                Segment::CubicTo(a, b, c, d, x, y) => {
                    let (a, b) = f(a, b);
                    let (c, d) = f(c, d);
                    let (x, y) = f(x, y);
                    Segment::CubicTo(a, b, c, d, x, y)
                }
                Segment::Close => Segment::Close,
            })
            .collect();
        Path { segments }
    }

    /// The exact bounds of the path as `(min x, min y, max x, max y)`,
    /// following curves rather than their control points (what
    /// `Path2D.getBounds2D` returns since Java 19). `None` for an empty
    /// path.
    pub(crate) fn bounds2d(&self) -> Option<(f64, f64, f64, f64)> {
        let mut b: Option<(f64, f64, f64, f64)> = None;
        let mut add = |x: f64, y: f64| {
            b = Some(match b {
                None => (x, y, x, y),
                Some((x0, y0, x1, y1)) => (x0.min(x), y0.min(y), x1.max(x), y1.max(y)),
            });
        };
        let (mut cx, mut cy) = (0.0f64, 0.0f64);
        for s in &self.segments {
            match *s {
                Segment::MoveTo(x, y) | Segment::LineTo(x, y) => {
                    cx = f64::from(x);
                    cy = f64::from(y);
                    add(cx, cy);
                }
                Segment::QuadTo(a, b, x, y) => {
                    let p = [cx, cy, f64::from(a), f64::from(b), f64::from(x), f64::from(y)];
                    for axis in 0..2 {
                        let (p0, p1, p2) = (p[axis], p[2 + axis], p[4 + axis]);
                        let d = p0 - 2.0 * p1 + p2;
                        if d != 0.0 {
                            let t = (p0 - p1) / d;
                            if t > 0.0 && t < 1.0 {
                                let (qx, qy) = quad_at(&p, t);
                                add(qx, qy);
                            }
                        }
                    }
                    cx = p[4];
                    cy = p[5];
                    add(cx, cy);
                }
                Segment::CubicTo(a, b, c, d, x, y) => {
                    let p = [
                        cx,
                        cy,
                        f64::from(a),
                        f64::from(b),
                        f64::from(c),
                        f64::from(d),
                        f64::from(x),
                        f64::from(y),
                    ];
                    for axis in 0..2 {
                        let (p0, p1, p2, p3) = (p[axis], p[2 + axis], p[4 + axis], p[6 + axis]);
                        // Derivative: 3[(p1-p0)(1-t)^2 + 2(p2-p1)(1-t)t + (p3-p2)t^2]
                        let qa = -p0 + 3.0 * p1 - 3.0 * p2 + p3;
                        let qb = 2.0 * (p0 - 2.0 * p1 + p2);
                        let qc = p1 - p0;
                        let mut roots = [f64::NAN; 2];
                        if qa.abs() < 1e-12 {
                            if qb != 0.0 {
                                roots[0] = -qc / qb;
                            }
                        } else {
                            let disc = qb * qb - 4.0 * qa * qc;
                            if disc >= 0.0 {
                                let sq = disc.sqrt();
                                roots[0] = (-qb + sq) / (2.0 * qa);
                                roots[1] = (-qb - sq) / (2.0 * qa);
                            }
                        }
                        for t in roots {
                            if t > 0.0 && t < 1.0 {
                                let (qx, qy) = cubic_at(&p, t);
                                add(qx, qy);
                            }
                        }
                    }
                    cx = p[6];
                    cy = p[7];
                    add(cx, cy);
                }
                Segment::Close => {}
            }
        }
        b
    }

    /// The integer bounding rectangle ImageJ gives the selection:
    /// `Shape.getBounds()` in the `ShapeRoi(float[])` constructor.
    pub(crate) fn frame(&self) -> Frame {
        match self.bounds2d() {
            None => Frame {
                x: 0,
                y: 0,
                w: 0,
                h: 0,
            },
            Some((x0, y0, x1, y1)) => {
                let x = x0.floor() as i32;
                let y = y0.floor() as i32;
                let extent = |low: i32, high: f64| {
                    (high.ceil() - f64::from(low)).clamp(0.0, f64::from(i32::MAX)) as i32
                };
                Frame {
                    x,
                    y,
                    w: extent(x, x1),
                    h: extent(y, y1),
                }
            }
        }
    }

    /// The path in double precision, for the algorithms Java runs on
    /// `double` coordinates.
    pub(crate) fn steps(&self) -> Vec<PathStep> {
        let d = f64::from;
        self.segments
            .iter()
            .map(|s| match *s {
                Segment::MoveTo(x, y) => PathStep::Move(d(x), d(y)),
                Segment::LineTo(x, y) => PathStep::Line(d(x), d(y)),
                Segment::QuadTo(a, b, x, y) => PathStep::Quad([d(a), d(b), d(x), d(y)]),
                Segment::CubicTo(a, b, c, e, x, y) => {
                    PathStep::Cubic([d(a), d(b), d(c), d(e), d(x), d(y)])
                }
                Segment::Close => PathStep::Close,
            })
            .collect()
    }

    /// The path relative to its frame, in single precision, as `ShapeRoi`
    /// keeps it.
    pub(crate) fn local(&self, frame: Frame) -> Path {
        self.translated(-f64::from(frame.x), -f64::from(frame.y))
    }

    /// `ShapeRoi.getFloatPolygon(flatness, separateSubpaths,
    /// addPointForClose, absoluteCoord)`: the vertices of the path with
    /// curves replaced by line segments. With `separate`, sub-paths are
    /// separated by a NaN vertex.
    pub(crate) fn float_polygon(
        &self,
        flatness: f64,
        separate: bool,
        add_point_for_close: bool,
        absolute: bool,
    ) -> (Vec<f32>, Vec<f32>) {
        let frame = self.frame();
        let local = self.local(frame);
        let (mut xp, mut yp) = flatten(&local.steps(), flatness, separate, add_point_for_close);
        if absolute {
            let (xb, yb) = (frame.x as f32, frame.y as f32);
            for v in &mut xp {
                *v += xb;
            }
            for v in &mut yp {
                *v += yb;
            }
        }
        if xp.last().is_some_and(|v| v.is_nan()) {
            xp.pop();
            yp.pop();
        }
        (xp, yp)
    }

    /// The sub-paths as separate polygons in image coordinates, curves
    /// flattened as for reported outlines.
    pub fn polygons(&self) -> Vec<Vec<(f64, f64)>> {
        let (xs, ys) = self.float_polygon(FLATNESS, true, false, true);
        let mut out = vec![Vec::new()];
        for (x, y) in xs.into_iter().zip(ys) {
            if x.is_nan() {
                out.push(Vec::new());
            } else {
                out.last_mut()
                    .expect("starts with one polygon")
                    .push((f64::from(x), f64::from(y)));
            }
        }
        out.retain(|p| !p.is_empty());
        out
    }
}

fn quad_at(p: &[f64; 6], t: f64) -> (f64, f64) {
    let u = 1.0 - t;
    (
        u * u * p[0] + 2.0 * u * t * p[2] + t * t * p[4],
        u * u * p[1] + 2.0 * u * t * p[3] + t * t * p[5],
    )
}

fn cubic_at(p: &[f64; 8], t: f64) -> (f64, f64) {
    let u = 1.0 - t;
    (
        u * u * u * p[0] + 3.0 * u * u * t * p[2] + 3.0 * u * t * t * p[4] + t * t * t * p[6],
        u * u * u * p[1] + 3.0 * u * u * t * p[3] + 3.0 * u * t * t * p[5] + t * t * t * p[7],
    )
}

/// Square of the distance from `(px, py)` to the segment from `(x1, y1)`
/// to `(x2, y2)`, as defined for Java's `Line2D.ptSegDistSq`.
fn pt_seg_dist_sq(x1: f64, y1: f64, x2: f64, y2: f64, px: f64, py: f64) -> f64 {
    let (x2, y2) = (x2 - x1, y2 - y1);
    let (mut px, mut py) = (px - x1, py - y1);
    let mut dot = px * x2 + py * y2;
    let proj_len_sq = if dot <= 0.0 {
        0.0
    } else {
        px = x2 - px;
        py = y2 - py;
        dot = px * x2 + py * y2;
        if dot <= 0.0 {
            0.0
        } else {
            dot * dot / (x2 * x2 + y2 * y2)
        }
    };
    (px * px + py * py - proj_len_sq).max(0.0)
}

/// Replaces a cubic curve by line segments the way Java's
/// `FlatteningPathIterator` does: halve the curve until both control
/// points are within `flatness` of the chord, at most ten times.
fn flatten_cubic(p: [f64; 8], flat_sq: f64, level: u32, out: &mut Vec<(f64, f64)>) {
    let flatness_sq = pt_seg_dist_sq(p[0], p[1], p[6], p[7], p[2], p[3])
        .max(pt_seg_dist_sq(p[0], p[1], p[6], p[7], p[4], p[5]));
    if level >= FLATTEN_LIMIT || flatness_sq < flat_sq {
        out.push((p[6], p[7]));
        return;
    }
    let (left, right) = split_cubic(&p);
    flatten_cubic(left, flat_sq, level + 1, out);
    flatten_cubic(right, flat_sq, level + 1, out);
}

/// `CubicCurve2D.subdivide`: the two halves of a cubic curve.
fn split_cubic(p: &[f64; 8]) -> ([f64; 8], [f64; 8]) {
    let (x1, y1, cx1, cy1, cx2, cy2, x2, y2) = (p[0], p[1], p[2], p[3], p[4], p[5], p[6], p[7]);
    let (mx, my) = ((cx1 + cx2) / 2.0, (cy1 + cy2) / 2.0);
    let (lx1, ly1) = ((x1 + cx1) / 2.0, (y1 + cy1) / 2.0);
    let (rx2, ry2) = ((x2 + cx2) / 2.0, (y2 + cy2) / 2.0);
    let (lx2, ly2) = ((lx1 + mx) / 2.0, (ly1 + my) / 2.0);
    let (rx1, ry1) = ((mx + rx2) / 2.0, (my + ry2) / 2.0);
    let (cx, cy) = ((lx2 + rx1) / 2.0, (ly2 + ry1) / 2.0);
    (
        [x1, y1, lx1, ly1, lx2, ly2, cx, cy],
        [cx, cy, rx1, ry1, rx2, ry2, x2, y2],
    )
}

fn flatten_quad(p: [f64; 6], flat_sq: f64, level: u32, out: &mut Vec<(f64, f64)>) {
    if level >= FLATTEN_LIMIT || pt_seg_dist_sq(p[0], p[1], p[4], p[5], p[2], p[3]) < flat_sq {
        out.push((p[4], p[5]));
        return;
    }
    let (left, right) = split_quad(&p);
    flatten_quad(left, flat_sq, level + 1, out);
    flatten_quad(right, flat_sq, level + 1, out);
}

/// `QuadCurve2D.subdivide`.
fn split_quad(p: &[f64; 6]) -> ([f64; 6], [f64; 6]) {
    let (x1, y1, cx, cy, x2, y2) = (p[0], p[1], p[2], p[3], p[4], p[5]);
    let (lx, ly) = ((x1 + cx) / 2.0, (y1 + cy) / 2.0);
    let (rx, ry) = ((x2 + cx) / 2.0, (y2 + cy) / 2.0);
    let (mx, my) = ((lx + rx) / 2.0, (ly + ry) / 2.0);
    ([x1, y1, lx, ly, mx, my], [mx, my, rx, ry, x2, y2])
}

/// `ShapeRoi.getFloatPolygon(Shape, ...)` without the final offset.
pub(crate) fn flatten(
    steps: &[PathStep],
    flatness: f64,
    separate: bool,
    add_point_for_close: bool,
) -> (Vec<f32>, Vec<f32>) {
    let flat_sq = flatness * flatness;
    let mut xp: Vec<f32> = Vec::new();
    let mut yp: Vec<f32> = Vec::new();
    let mut start = 0usize;
    let (mut cx, mut cy) = (0.0f64, 0.0f64);
    let (mut mx, mut my) = (0.0f64, 0.0f64);
    let mut buffer = Vec::new();
    let separator = |xp: &mut Vec<f32>, yp: &mut Vec<f32>| {
        if separate && xp.last().is_some_and(|v| !v.is_nan()) {
            xp.push(f32::NAN);
            yp.push(f32::NAN);
        }
    };
    for s in steps {
        match *s {
            PathStep::Move(x, y) => {
                separator(&mut xp, &mut yp);
                start = xp.len();
                xp.push(x as f32);
                yp.push(y as f32);
                cx = x;
                cy = y;
                mx = cx;
                my = cy;
            }
            PathStep::Line(x, y) => {
                xp.push(x as f32);
                yp.push(y as f32);
                cx = x;
                cy = y;
            }
            PathStep::Quad(c) => {
                buffer.clear();
                flatten_quad([cx, cy, c[0], c[1], c[2], c[3]], flat_sq, 0, &mut buffer);
                for &(px, py) in &buffer {
                    xp.push(px as f32);
                    yp.push(py as f32);
                }
                cx = c[2];
                cy = c[3];
            }
            PathStep::Cubic(c) => {
                buffer.clear();
                let p = [cx, cy, c[0], c[1], c[2], c[3], c[4], c[5]];
                flatten_cubic(p, flat_sq, 0, &mut buffer);
                for &(px, py) in &buffer {
                    xp.push(px as f32);
                    yp.push(py as f32);
                }
                cx = c[4];
                cy = c[5];
            }
            PathStep::Close => {
                if start < xp.len() {
                    let last = xp.len() - 1;
                    let closed = xp[last] == xp[start] && yp[last] == yp[start];
                    if add_point_for_close && !closed {
                        xp.push(xp[start]);
                        yp.push(yp[start]);
                    } else if closed && last > start {
                        xp.pop();
                        yp.pop();
                    }
                }
                separator(&mut xp, &mut yp);
                cx = mx;
                cy = my;
            }
        }
    }
    (xp, yp)
}

/// Java's `Ellipse2D` outline: four cubic curves starting at the
/// rightmost point and running through the bottom, in double precision.
pub(crate) fn ellipse_segments(x: f64, y: f64, w: f64, h: f64) -> Vec<[f64; 6]> {
    // The usual constant for approximating a quarter circle by a cubic.
    const CTRL: f64 = 0.552_284_749_830_793_3;
    let pcv = 0.5 + CTRL * 0.5;
    let ncv = 0.5 - CTRL * 0.5;
    let ctrl: [[f64; 6]; 4] = [
        [1.0, pcv, pcv, 1.0, 0.5, 1.0],
        [ncv, 1.0, 0.0, pcv, 0.0, 0.5],
        [0.0, ncv, ncv, 0.0, 0.5, 0.0],
        [pcv, 0.0, 1.0, ncv, 1.0, 0.5],
    ];
    ctrl.iter()
        .map(|c| {
            [
                x + c[0] * w,
                y + c[1] * h,
                x + c[2] * w,
                y + c[3] * h,
                x + c[4] * w,
                y + c[5] * h,
            ]
        })
        .collect()
}

/// The path of an ellipse inscribed in a rectangle, as Java's
/// `Ellipse2D.getPathIterator` gives it.
pub(crate) fn ellipse_path(x: f64, y: f64, w: f64, h: f64) -> Vec<PathStep> {
    let mut out = vec![PathStep::Move(x + w, y + 0.5 * h)];
    out.extend(ellipse_segments(x, y, w, h).into_iter().map(PathStep::Cubic));
    out.push(PathStep::Close);
    out
}

/// A path step in double precision, before it is stored in a `Path`.
#[derive(Clone, Copy, Debug)]
pub(crate) enum PathStep {
    Move(f64, f64),
    Line(f64, f64),
    Quad([f64; 4]),
    Cubic([f64; 6]),
    Close,
}

/// The path of a rectangle with rounded corners, as Java's
/// `RoundRectangle2D.getPathIterator` gives it: down the left side first.
pub(crate) fn round_rect_path(x: f64, y: f64, w: f64, h: f64, arc: f64) -> Vec<PathStep> {
    const CTRL: f64 = 0.552_284_749_830_793_3;
    let acv = (1.0 - CTRL) / 2.0;
    let aw = w.min(arc.abs());
    let ah = h.min(arc.abs());
    // Each row: pairs of (fraction of size, fraction of arc size) for x
    // then y, per point.
    let pt = |fx: f64, ax: f64, fy: f64, ay: f64| (x + fx * w + ax * aw, y + fy * h + ay * ah);
    let cubic = |c: [f64; 12]| {
        let a = pt(c[0], c[1], c[2], c[3]);
        let b = pt(c[4], c[5], c[6], c[7]);
        let e = pt(c[8], c[9], c[10], c[11]);
        PathStep::Cubic([a.0, a.1, b.0, b.1, e.0, e.1])
    };
    let line = |fx, ax, fy, ay| {
        let p = pt(fx, ax, fy, ay);
        PathStep::Line(p.0, p.1)
    };
    let start = pt(0.0, 0.0, 0.0, 0.5);
    vec![
        PathStep::Move(start.0, start.1),
        line(0.0, 0.0, 1.0, -0.5),
        cubic([
            0.0, 0.0, 1.0, -acv, 0.0, acv, 1.0, 0.0, 0.0, 0.5, 1.0, 0.0,
        ]),
        line(1.0, -0.5, 1.0, 0.0),
        cubic([
            1.0, -acv, 1.0, 0.0, 1.0, 0.0, 1.0, -acv, 1.0, 0.0, 1.0, -0.5,
        ]),
        line(1.0, 0.0, 0.0, 0.5),
        cubic([
            1.0, 0.0, 0.0, acv, 1.0, -acv, 0.0, 0.0, 1.0, -0.5, 0.0, 0.0,
        ]),
        line(0.0, 0.5, 0.0, 0.0),
        cubic([
            0.0, acv, 0.0, 0.0, 0.0, 0.0, 0.0, acv, 0.0, 0.0, 0.0, 0.5,
        ]),
        PathStep::Close,
    ]
}

/// Stores a double-precision path in single precision, moved by
/// `(dx, dy)`, as `new GeneralPath(transform.createTransformedShape(s))`
/// does.
pub(crate) fn to_path(steps: &[PathStep], dx: f64, dy: f64) -> Path {
    let f = |v: f64, d: f64| (v + d) as f32;
    Path::new(
        steps
            .iter()
            .map(|s| match *s {
                PathStep::Move(x, y) => Segment::MoveTo(f(x, dx), f(y, dy)),
                PathStep::Line(x, y) => Segment::LineTo(f(x, dx), f(y, dy)),
                PathStep::Quad(c) => {
                    Segment::QuadTo(f(c[0], dx), f(c[1], dy), f(c[2], dx), f(c[3], dy))
                }
                PathStep::Cubic(c) => Segment::CubicTo(
                    f(c[0], dx),
                    f(c[1], dy),
                    f(c[2], dx),
                    f(c[3], dy),
                    f(c[4], dx),
                    f(c[5], dy),
                ),
                PathStep::Close => Segment::Close,
            })
            .collect(),
    )
}

/// Vertices of a shape given relative to `frame`, flattened as
/// `ShapeRoi.getFloatPolygon` does. `absolute` adds the frame's corner.
pub(crate) fn flatten_local(
    local: &[PathStep],
    frame: Frame,
    flatness: f64,
    separate: bool,
    absolute: bool,
) -> (Vec<f32>, Vec<f32>) {
    let (mut xp, mut yp) = flatten(local, flatness, separate, false);
    if absolute {
        let (xb, yb) = (frame.x as f32, frame.y as f32);
        for v in &mut xp {
            *v += xb;
        }
        for v in &mut yp {
            *v += yb;
        }
    }
    if xp.last().is_some_and(|v| v.is_nan()) {
        xp.pop();
        yp.pop();
    }
    (xp, yp)
}

/// How often a curve is halved at most when its length is measured. Each
/// halving cuts the error to about a quarter, so curves up to tens of
/// millions of pixels long reach ImageJ's tolerance well before this;
/// ImageJ itself has no limit and would not return for longer ones.
const LENGTH_DEPTH: u32 = 18;

/// Length of a cubic curve: `ShapeRoi.cBezLength` (Gravesen's adaptive
/// subdivision).
pub(crate) fn cubic_length(p: &[f64; 8]) -> f64 {
    fn length(p: &[f64; 8], depth: u32) -> f64 {
        let d = |a: usize, b: usize| ((p[b] - p[a]).powi(2) + (p[b + 1] - p[a + 1]).powi(2)).sqrt();
        let chord = d(0, 6);
        let polygon = d(0, 2) + d(2, 4) + d(4, 6);
        if (polygon - chord) / 2.0 > MAX_ERROR && depth < LENGTH_DEPTH {
            let (left, right) = split_cubic(p);
            return length(&left, depth + 1) + length(&right, depth + 1);
        }
        0.5 * polygon + 0.5 * chord
    }
    length(p, 0)
}

/// Length of a quadratic curve: `ShapeRoi.qBezLength`.
pub(crate) fn quad_length(p: &[f64; 6]) -> f64 {
    fn length(p: &[f64; 6], depth: u32) -> f64 {
        let d = |a: usize, b: usize| ((p[b] - p[a]).powi(2) + (p[b + 1] - p[a + 1]).powi(2)).sqrt();
        let chord = d(0, 4);
        let polygon = d(0, 2) + d(2, 4);
        if (polygon - chord) / 2.0 > MAX_ERROR && depth < LENGTH_DEPTH {
            let (left, right) = split_quad(p);
            return length(&left, depth + 1) + length(&right, depth + 1);
        }
        (2.0 * polygon + chord) / 3.0
    }
    length(p, 0)
}

/// ImageJ's `Wand.autoOutline(x, y, 255, 255)` on a mask in legacy mode,
/// as `OvalRoi.getPolygon` uses it: the outline of the 8-connected region
/// found by walking right from the start pixel.
pub(crate) fn wand_outline(
    data: &[u8],
    width: i32,
    height: i32,
    start_x: i32,
    start_y: i32,
) -> Vec<(i32, i32)> {
    let inside = |x: i32, y: i32| {
        x >= 0 && x < width && y >= 0 && y < height && data[(y * width + x) as usize] != 0
    };
    let inside_dir = |x: i32, y: i32, direction: i32| match direction & 3 {
        0 => inside(x, y),
        1 => inside(x, y - 1),
        2 => inside(x - 1, y - 1),
        _ => inside(x - 1, y),
    };
    let mut points = Vec::new();
    if start_x < 0 || start_x >= width || start_y < 0 || start_y >= height {
        return points;
    }
    let mut x = start_x;
    let y = start_y;
    if inside(x, y) {
        while inside(x, y) {
            x += 1;
        }
    } else {
        loop {
            x += 1;
            if x >= width {
                return points;
            }
            if inside(x, y) {
                break;
            }
        }
    }
    // Wand.traceEdge, 8-connected.
    let (start_x, mut start_y) = (x, y);
    let start_direction = if inside(start_x, start_y) {
        1
    } else {
        start_y += 1;
        3
    };
    let (mut x, mut y) = (start_x, start_y);
    let mut direction: i32 = start_direction;
    loop {
        let mut new_direction = direction + 1;
        loop {
            if inside_dir(x, y, new_direction) {
                break;
            }
            new_direction -= 1;
            if new_direction < direction {
                break;
            }
        }
        if new_direction != direction {
            points.push((x, y));
        }
        match new_direction & 3 {
            0 => x += 1,
            1 => y -= 1,
            2 => x -= 1,
            _ => y += 1,
        }
        direction = new_direction;
        if x == start_x && y == start_y && (direction & 3) == start_direction {
            break;
        }
        if points.len() > 8 * (width as usize + 1) * (height as usize + 1) {
            break; // cannot happen for a valid mask; guards against looping
        }
    }
    if points.first().is_some_and(|p| p.0 != x) {
        points.push((x, y));
    }
    points
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn array_form_round_trips() {
        let path = Path::new(vec![
            Segment::MoveTo(1.0, 2.0),
            Segment::LineTo(3.5, 2.0),
            Segment::QuadTo(4.0, 4.0, 3.0, 5.0),
            Segment::CubicTo(1.0, 6.0, 0.0, 5.0, 1.0, 3.0),
            Segment::Close,
        ]);
        assert_eq!(Path::from_array(&path.to_array()), Some(path));
        assert_eq!(Path::from_array(&[9.0, 1.0]), None);
        // A segment cut short ends the path, as in ImageJ.
        let cut = Path::from_array(&[0.0, 1.0, 2.0, 1.0, 3.0]).unwrap();
        assert_eq!(cut.segments(), &[Segment::MoveTo(1.0, 2.0)]);
    }
}
