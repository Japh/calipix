//! Making one selection from another: the geometry behind the commands of
//! Edit > Selection.

use std::f64::consts::PI;

use crate::fill::{self, Polygon, Window};
use crate::geometry::{self, Guess};
use crate::model::{Coords, Frame, Poly, int_bounds, jround};
use crate::path::{self, Path, Segment};
use crate::{Point, Roi};

/// Largest number of pixels a temporary mask may have.
const MAX_WORK_PIXELS: i64 = 1 << 26;

/// The ellipse with the same area and second moments as a selection:
/// ImageJ's `EllipseFitter`, which gives the "Major", "Minor" and "Angle"
/// measurements and Edit > Selection > Fit Ellipse.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EllipseFit {
    /// Centre, in image coordinates.
    pub x_center: f64,
    pub y_center: f64,
    /// Lengths of the axes, in pixels.
    pub major: f64,
    pub minor: f64,
    /// Direction of the major axis in degrees, 0 to 180, anticlockwise
    /// from the positive x axis.
    pub angle: f64,
}

/// How two areas are combined.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Combine {
    /// Pixels in either.
    Or,
    /// Pixels in both.
    And,
    /// Pixels in exactly one.
    Xor,
    /// Pixels in the first but not the second.
    Not,
}

/// The selection ImageJ's `ThresholdToSelection` followed by
/// `ShapeRoi.trySimplify` would make of a mask: a rectangle, a traced
/// outline or polygon when there is a single outline, a composite
/// otherwise, `None` if no pixel is set.
///
/// `data` is `width` x `height`, row-major, non-zero meaning selected,
/// with its first pixel at image position `(x0, y0)`.
pub(crate) fn from_mask(data: &[u8], width: usize, height: usize, x0: i32, y0: i32) -> Option<Roi> {
    let polygons = crate::trace::trace_mask(data, width, height, x0, y0);
    simplify(Path::from_polygons(
        polygons
            .iter()
            .map(|p| p.iter().map(|&(x, y)| (x as f32, y as f32))),
    ))
}

/// `ShapeRoi.trySimplify`: a path with a single sub-path of straight
/// segments becomes the plain selection it describes.
pub(crate) fn simplify(path: Path) -> Option<Roi> {
    if path.is_empty() {
        return None;
    }
    let moves = path
        .segments()
        .iter()
        .filter(|s| matches!(s, Segment::MoveTo(..)))
        .count();
    if moves != 1 || path.has_curves() {
        return Some(Roi::Composite { path });
    }
    let (mut xs, mut ys) = (Vec::new(), Vec::new());
    let mut closed = false;
    let mut length = 0.0f64;
    let mut hv_only = true;
    for s in path.segments() {
        let to = match *s {
            Segment::MoveTo(x, y) => {
                xs.push(x);
                ys.push(y);
                continue;
            }
            Segment::LineTo(x, y) => {
                closed = false;
                Some((x, y, true))
            }
            Segment::Close => {
                closed = true;
                Some((xs[0], ys[0], false))
            }
            _ => None,
        };
        if let Some((x, y, add)) = to {
            let dx = x - xs[xs.len() - 1];
            let dy = y - ys[ys.len() - 1];
            length += (f64::from(dx).powi(2) + f64::from(dy).powi(2)).sqrt();
            if dx != 0.0 && dy != 0.0 {
                hv_only = false;
            }
            if add {
                xs.push(x);
                ys.push(y);
            }
        }
    }
    let n = xs.len();
    closed = closed || (n > 0 && xs[0] == xs[n - 1] && ys[0] == ys[n - 1]);
    let whole = |v: &[f32]| v.iter().all(|&c| c == (c as i32) as f32);
    if !whole(&xs) || !whole(&ys) {
        hv_only = false;
    }
    let guess = geometry::guess_type(n, length, hv_only, false, closed);
    match geometry::roi_from_guess(guess, &xs, &ys) {
        Some(roi) => Some(roi),
        None if guess == Guess::None => None,
        None => Some(Roi::Composite { path }),
    }
}

/// True if every edge of the path is horizontal or vertical between
/// whole-number vertices.
fn rectilinear(path: &Path) -> bool {
    let whole = |v: f32| v == (v as i32) as f32;
    let (mut last, mut start) = ((0.0f32, 0.0f32), (0.0f32, 0.0f32));
    path.segments().iter().all(|s| match *s {
        Segment::MoveTo(x, y) => {
            last = (x, y);
            start = last;
            whole(x) && whole(y)
        }
        Segment::LineTo(x, y) => {
            let ok = whole(x) && whole(y) && (x == last.0 || y == last.1);
            last = (x, y);
            ok
        }
        Segment::Close => {
            let ok = start.0 == last.0 || start.1 == last.1;
            last = start;
            ok
        }
        Segment::QuadTo(..) | Segment::CubicTo(..) => false,
    })
}

/// A mask on a canvas covering several selections.
struct Canvas {
    x: i32,
    y: i32,
    w: usize,
    h: usize,
    data: Vec<u8>,
}

impl Canvas {
    fn new(x: i32, y: i32, w: i64, h: i64) -> Option<Canvas> {
        if w <= 0 || h <= 0 || w.saturating_mul(h) > MAX_WORK_PIXELS {
            return None;
        }
        Some(Canvas {
            x,
            y,
            w: w as usize,
            h: h as usize,
            data: vec![0; (w * h) as usize],
        })
    }

    /// The mask of `roi`, restricted to the canvas.
    fn of(roi: &Roi, x: i32, y: i32, w: i64, h: i64) -> Option<Canvas> {
        let mut canvas = Canvas::new(x, y, w, h)?;
        let moved = roi.translated(-f64::from(x), -f64::from(y));
        if let Some(mask) = moved.mask(canvas.w, canvas.h) {
            for row in 0..mask.rect.h {
                let from = row * mask.rect.w;
                let to = (row + mask.rect.y) * canvas.w + mask.rect.x;
                canvas.data[to..to + mask.rect.w]
                    .copy_from_slice(&mask.data[from..from + mask.rect.w]);
            }
        }
        Some(canvas)
    }

    fn roi(&self) -> Option<Roi> {
        from_mask(&self.data, self.w, self.h, self.x, self.y)
    }
}

/// `EDM.makeFloatEDM(ip, 0, false)` followed by the conversion to 8 bits
/// that `EDM.toEDM` does: for each non-zero pixel, the distance to the
/// nearest zero pixel, rounded half up and capped at 255.
///
/// ImageJ finds the nearest zero pixel by passing candidates along rows
/// and diagonals in four sweeps, which is not always the true nearest;
/// the sweeps are reproduced so that enlarged selections match.
fn distance_map(data: &[u8], width: usize, height: usize) -> Vec<u8> {
    const NO_POINT: i32 = -1;
    let mut dist: Vec<f32> = data
        .iter()
        .map(|&v| if v != 0 { f32::MAX } else { 0.0 })
        .collect();
    let mut bufs = [vec![NO_POINT; width], vec![NO_POINT; width]];
    fn min_dist2(points: &mut [i32], p_prev: i32, p_diag: i32, x: i32, y: i32) -> f32 {
        let mut dist_sqr = i32::MAX;
        let p0 = points[x as usize];
        let mut nearest = p0;
        let d = |p: i32| {
            let (px, py) = (p & 0xffff, (p >> 16) & 0xffff);
            // Java's int arithmetic, which wraps for masks taller or
            // wider than 46340 pixels.
            let (dx, dy) = (x - px, y - py);
            dx.wrapping_mul(dx).wrapping_add(dy.wrapping_mul(dy))
        };
        if p0 != NO_POINT {
            dist_sqr = dist_sqr.min(d(p0));
        }
        if p_diag != p0 && p_diag != NO_POINT && d(p_diag) < dist_sqr {
            nearest = p_diag;
            dist_sqr = d(p_diag);
        }
        if p_prev != p_diag && p_prev != NO_POINT && d(p_prev) < dist_sqr {
            nearest = p_prev;
            dist_sqr = d(p_prev);
        }
        points[x as usize] = nearest;
        dist_sqr as f32
    }
    let line = |bufs: &mut [Vec<i32>; 2], dist: &mut [f32], y: usize| {
        for (pass, points) in bufs.iter_mut().enumerate() {
            let (mut p_prev, mut p_diag) = (NO_POINT, NO_POINT);
            for step in 0..width {
                let x = if pass == 0 { step } else { width - 1 - step };
                let offset = y * width + x;
                let p_next_diag = points[x];
                if data[offset] == 0 {
                    points[x] = x as i32 | (y as i32) << 16;
                } else {
                    let d2 = min_dist2(points, p_prev, p_diag, x as i32, y as i32);
                    if dist[offset] > d2 {
                        dist[offset] = d2;
                    }
                }
                p_prev = points[x];
                p_diag = p_next_diag;
            }
        }
    };
    for y in 0..height {
        line(&mut bufs, &mut dist, y);
    }
    bufs[0].fill(NO_POINT);
    bufs[1].fill(NO_POINT);
    for y in (0..height).rev() {
        line(&mut bufs, &mut dist, y);
    }
    dist.iter()
        .map(|&d2| (d2.sqrt() + 0.5).clamp(0.0, 255.0) as u8)
        .collect()
}

impl Roi {
    /// The selection moved by `(dx, dy)`.
    pub fn translated(&self, dx: f64, dy: f64) -> Roi {
        let shift = |points: &[Point]| -> Vec<Point> {
            points.iter().map(|&(x, y)| (x + dx, y + dy)).collect()
        };
        match self {
            Roi::Rectangle {
                x,
                y,
                width,
                height,
            } => Roi::rectangle(x + dx, y + dy, *width, *height),
            Roi::RoundedRectangle {
                x,
                y,
                width,
                height,
                corner_diameter,
            } => Roi::RoundedRectangle {
                x: x + dx,
                y: y + dy,
                width: *width,
                height: *height,
                corner_diameter: *corner_diameter,
            },
            Roi::Oval {
                x,
                y,
                width,
                height,
            } => Roi::oval(x + dx, y + dy, *width, *height),
            Roi::Polygon { points } => Roi::Polygon {
                points: shift(points),
            },
            Roi::Freehand { points } => Roi::Freehand {
                points: shift(points),
            },
            Roi::Traced { points } => Roi::Traced {
                points: shift(points),
            },
            Roi::Points { points } => Roi::Points {
                points: shift(points),
            },
            Roi::Polyline { points, width } => Roi::Polyline {
                points: shift(points),
                width: *width,
            },
            Roi::Freeline { points, width } => Roi::Freeline {
                points: shift(points),
                width: *width,
            },
            Roi::Angle { points } => {
                let p = shift(points);
                Roi::Angle {
                    points: [p[0], p[1], p[2]],
                }
            }
            Roi::Line {
                x1,
                y1,
                x2,
                y2,
                width,
            } => Roi::Line {
                x1: x1 + dx,
                y1: y1 + dy,
                x2: x2 + dx,
                y2: y2 + dy,
                width: *width,
            },
            Roi::Ellipse {
                x1,
                y1,
                x2,
                y2,
                aspect_ratio,
            } => Roi::ellipse(x1 + dx, y1 + dy, x2 + dx, y2 + dy, *aspect_ratio),
            Roi::RotatedRectangle {
                x1,
                y1,
                x2,
                y2,
                width,
            } => Roi::rotated_rectangle(x1 + dx, y1 + dy, x2 + dx, y2 + dy, *width),
            Roi::Composite { path } => Roi::Composite {
                path: path.translated(dx, dy),
            },
        }
    }

    /// The selection's bounding rectangle in whole pixels as a rectangle
    /// selection: Edit > Selection > To Bounding Box.
    pub fn bounding_box(&self) -> Roi {
        let f = self.frame();
        Roi::rectangle(
            f64::from(f.x),
            f64::from(f.y),
            f64::from(f.w),
            f64::from(f.h),
        )
    }

    /// The convex hull as a polygon selection: Edit > Selection > Convex
    /// Hull. `None` for a straight line, which ImageJ refuses, and when
    /// the hull has fewer than three vertices.
    pub fn convex_hull_roi(&self) -> Option<Roi> {
        if matches!(self, Roi::Line { .. }) {
            return None;
        }
        let points = self.convex_hull();
        (points.len() >= 3).then_some(Roi::Polygon { points })
    }

    /// `EllipseRoi.getParams` and `RotatedRectRoi.getParams`: the two end
    /// points ImageJ reads back from the vertices it stores, which is
    /// what it writes to files.
    pub(crate) fn axis_params(&self) -> Option<[f64; 4]> {
        let poly = self.poly()?;
        let Coords::Float(xs, ys) = &poly.coords else {
            return None;
        };
        let (fx, fy) = (poly.frame.x as f32, poly.frame.y as f32);
        match self {
            Roi::Ellipse { .. } => Some([
                f64::from(xs[36] + fx),
                f64::from(ys[36] + fy),
                f64::from(xs[0] + fx),
                f64::from(ys[0] + fy),
            ]),
            Roi::RotatedRectangle { .. } => {
                let mid = |v: &[f32], i: usize, base: f32| {
                    let next = if i < 3 { i + 1 } else { 0 };
                    f64::from(v[i] + (v[next] - v[i]) / 2.0 + base)
                };
                Some([
                    mid(xs, 3, fx),
                    mid(ys, 3, fy),
                    mid(xs, 1, fx),
                    mid(ys, 1, fy),
                ])
            }
            _ => None,
        }
    }

    /// The outline ImageJ gives this selection when it turns it into a
    /// `ShapeRoi` (`ShapeRoi.roiToShape`): an oval becomes the boundary of
    /// its mask, a rounded rectangle keeps its curves.
    pub(crate) fn shape_path(&self) -> Option<Path> {
        let f = self.frame();
        let rect = |x: f32, y: f32, w: f32, h: f32| {
            Path::from_polygons([[(x, y), (x + w, y), (x + w, y + h), (x, y + h)]])
        };
        Some(match self {
            Roi::Composite { path } => path.clone(),
            Roi::Rectangle { .. } => rect(f.x as f32, f.y as f32, f.w as f32, f.h as f32),
            Roi::RoundedRectangle {
                corner_diameter, ..
            } => {
                let steps = path::round_rect_path(
                    0.0,
                    0.0,
                    f64::from(f.w),
                    f64::from(f.h),
                    f64::from(*corner_diameter),
                );
                path::to_path(&steps, f64::from(f.x), f64::from(f.y))
            }
            Roi::Oval { .. } => Path::from_polygons([self
                .int_outline()
                .into_iter()
                .map(|(x, y)| (x as f32, y as f32))]),
            _ if self.is_area() => {
                let (xs, ys) = self.float_polygon();
                Path::from_polygons([xs.into_iter().zip(ys)])
            }
            _ => self.to_area()?.shape_path()?,
        })
    }

    /// Everything in a `width` x `height` image that this area selection
    /// does not cover: Edit > Selection > Make Inverse
    /// (`Roi.getInverse`). `None` if the selection is not an area or
    /// covers the whole image.
    ///
    /// The result is the image rectangle and this selection's outline
    /// combined by the even-odd rule, simplified to a plain selection
    /// where possible. Like ImageJ's, it keeps any part of the selection
    /// that lies outside the image.
    pub fn inverse(&self, width: usize, height: usize) -> Option<Roi> {
        if !self.is_area() {
            return None;
        }
        let (w, h) = (width as f32, height as f32);
        let image = Path::from_polygons([[(0.0, 0.0), (w, 0.0), (w, h), (0.0, h)]]);
        let f = self.frame();
        let inside = f.x >= 0
            && f.y >= 0
            && i64::from(f.x) + i64::from(f.w) <= width as i64
            && i64::from(f.y) + i64::from(f.h) <= height as i64;
        if inside && self.is_rectangular() && f.w as usize == width && f.h as usize == height {
            return None;
        }
        let shape = self.shape_path()?;
        if rectilinear(&shape) {
            // Outlines along pixel edges are exactly their masks, so the
            // inverse can be traced, which also simplifies it to a
            // rectangle or polygon where ImageJ would.
            let x0 = f.x.min(0);
            let y0 = f.y.min(0);
            let x1 = (i64::from(f.x) + i64::from(f.w)).max(width as i64);
            let y1 = (i64::from(f.y) + i64::from(f.h)).max(height as i64);
            let (w, h) = (x1 - i64::from(x0), y1 - i64::from(y0));
            if let Some(mut canvas) = Canvas::of(self, x0, y0, w, h) {
                for row in 0..height {
                    let at = (row as i64 - i64::from(y0)) as usize * canvas.w
                        + (-i64::from(x0)) as usize;
                    for v in &mut canvas.data[at..at + width] {
                        *v = if *v == 0 { 255 } else { 0 };
                    }
                }
                return canvas.roi();
            }
        }
        let mut segments = image.segments().to_vec();
        segments.extend_from_slice(shape.segments());
        let inverse = Roi::Composite {
            path: Path::new(segments),
        };
        if inside && inverse.mask(width, height).is_none() {
            return None;
        }
        Some(inverse)
    }

    /// This area combined with another: the union, intersection,
    /// difference or exclusive-or of the pixels they cover. `None` if
    /// either is not an area or the result is empty.
    ///
    /// ImageJ combines outlines geometrically (`ShapeRoi.or` and its
    /// siblings); Calipix combines masks and traces the result, so the
    /// outcome covers the same pixels but its outline follows pixel
    /// edges.
    pub fn combined(&self, other: &Roi, how: Combine) -> Option<Roi> {
        if !self.is_area() || !other.is_area() {
            return None;
        }
        let (a, b) = (self.frame(), other.frame());
        let x0 = a.x.min(b.x);
        let y0 = a.y.min(b.y);
        let x1 = (i64::from(a.x) + i64::from(a.w)).max(i64::from(b.x) + i64::from(b.w));
        let y1 = (i64::from(a.y) + i64::from(a.h)).max(i64::from(b.y) + i64::from(b.h));
        let (w, h) = (x1 - i64::from(x0), y1 - i64::from(y0));
        let mut first = Canvas::of(self, x0, y0, w, h)?;
        let second = Canvas::of(other, x0, y0, w, h)?;
        for (p, &q) in first.data.iter_mut().zip(&second.data) {
            let (p_in, q_in) = (*p != 0, q != 0);
            let keep = match how {
                Combine::Or => p_in || q_in,
                Combine::And => p_in && q_in,
                Combine::Xor => p_in != q_in,
                Combine::Not => p_in && !q_in,
            };
            *p = if keep { 255 } else { 0 };
        }
        first.roi()
    }

    /// The mask ImageJ fills when it enlarges or shrinks a selection: the
    /// selection moved to a whole-pixel position first
    /// (`Roi.setLocation(int, int)` drops the fraction of a sub-pixel
    /// polygon's corner).
    fn snapped_mask(&self) -> (Frame, Vec<u8>) {
        let original = self.frame();
        let snapped: Option<Poly> =
            self.poly()
                .and_then(|poly| match (&poly.coords, poly.fbounds) {
                    (Coords::Float(..), Some(b)) => {
                        let fb = [0.0, 0.0, b[2], b[3]];
                        Some(Poly {
                            frame: int_bounds(fb, false),
                            fbounds: Some(fb),
                            coords: poly.coords.clone(),
                        })
                    }
                    _ => None,
                });
        match snapped {
            Some(poly) => {
                let (mut w, mut h) = (poly.frame.w.max(0), poly.frame.h.max(0));
                if i64::from(w) * i64::from(h) > MAX_WORK_PIXELS {
                    (w, h) = (0, 0);
                }
                let mut data = vec![0u8; w as usize * h as usize];
                if let Coords::Float(x, y) = &poly.coords {
                    let polygon = Polygon::Float {
                        x,
                        y,
                        x_offset: 0.0,
                        y_offset: 0.0,
                    };
                    fill::fill(&polygon, w, h, Window { x: 0, y: 0, w, h }, &mut data);
                }
                (
                    Frame {
                        x: original.x,
                        y: original.y,
                        w,
                        h,
                    },
                    data,
                )
            }
            None => self.full_mask(),
        }
    }

    /// The selection grown by `pixels` in every direction, or shrunk if
    /// negative: Edit > Selection > Enlarge (`RoiEnlarger.enlarge255`).
    ///
    /// A rectangle or oval has its bounds moved out or in. Any other area
    /// is filled into a mask, every pixel within `pixels` of it (by
    /// ImageJ's 8-bit distance map, distances rounded half up) is added,
    /// or every pixel within `pixels` of its outside removed, and the
    /// result is traced. `None` if the selection is not an area, the
    /// distance is 256 or more, or nothing would be left; ImageJ leaves
    /// the selection as it was in those cases.
    pub fn enlarged(&self, pixels: i32) -> Option<Roi> {
        if !self.is_area() || pixels.abs() >= 256 {
            return None;
        }
        if pixels == 0 {
            return Some(self.clone());
        }
        let n = pixels;
        if matches!(
            self,
            Roi::Rectangle { .. } | Roi::RoundedRectangle { .. } | Roi::Oval { .. }
        ) {
            let f = self.frame();
            let (w, h) = (
                f64::from(f.w) + 2.0 * f64::from(n),
                f64::from(f.h) + 2.0 * f64::from(n),
            );
            if w <= 0.0 || h <= 0.0 {
                return None;
            }
            let (x, y) = (f64::from(f.x) - f64::from(n), f64::from(f.y) - f64::from(n));
            return Some(if matches!(self, Roi::Oval { .. }) {
                Roi::oval(x, y, w, h)
            } else {
                Roi::rectangle(x, y, w, h)
            });
        }
        let (frame, mask) = self.snapped_mask();
        let margin = if n > 0 { n + 1 } else { 1 };
        let (w, h) = (
            i64::from(frame.w) + 2 * i64::from(margin),
            i64::from(frame.h) + 2 * i64::from(margin),
        );
        if w * h > MAX_WORK_PIXELS || frame.w <= 0 || frame.h <= 0 {
            return None;
        }
        if w >= 1 << 16 || h >= 1 << 16 {
            return None; // the distance map packs coordinates into 16 bits
        }
        let (w, h) = (w as usize, h as usize);
        // Growing measures distance from the selection, shrinking from
        // its outside; `inside` is the value marking the selection.
        let (inside, outside) = if n > 0 { (0u8, 255u8) } else { (255u8, 0u8) };
        let mut work = vec![outside; w * h];
        for row in 0..frame.h as usize {
            for col in 0..frame.w as usize {
                if mask[row * frame.w as usize + col] != 0 {
                    work[(row + margin as usize) * w + col + margin as usize] = inside;
                }
            }
        }
        let edm = distance_map(&work, w, h);
        let selected: Vec<u8> = if n > 0 {
            edm.iter()
                .map(|&d| u8::from(i32::from(d) <= n) * 255)
                .collect()
        } else {
            edm.iter()
                .map(|&d| u8::from(i32::from(d) > -n) * 255)
                .collect()
        };
        from_mask(
            &selected,
            w,
            h,
            frame.x.wrapping_sub(margin),
            frame.y.wrapping_sub(margin),
        )
    }

    /// The centre ImageJ scales a selection about when asked for a
    /// centred scale in a `width` x `height` image: the centroid of the
    /// pixels an area covers, or the middle of a line's bounds.
    pub fn scale_center(&self, width: usize, height: usize) -> Option<Point> {
        if self.is_line() {
            let f = self.frame();
            return Some((
                f64::from(f.x) + (f64::from(f.w) / 2.0 + 0.5).floor(),
                f64::from(f.y) + (f64::from(f.h) / 2.0 + 0.5).floor(),
            ));
        }
        let mask = self.mask(width, height)?;
        let (mut xsum, mut ysum, mut count) = (0.0f64, 0.0f64, 0u64);
        for row in 0..mask.rect.h {
            for col in 0..mask.rect.w {
                if mask.data[row * mask.rect.w + col] != 0 {
                    xsum += (col + mask.rect.x) as f64;
                    ysum += (row + mask.rect.y) as f64;
                    count += 1;
                }
            }
        }
        Some((xsum / count as f64 + 0.5, ysum / count as f64 + 0.5))
    }

    /// The selection scaled by `x_scale` and `y_scale`: Edit > Selection
    /// > Scale (`RoiScaler.scale`).
    ///
    /// Without a `center`, coordinates are multiplied by the factors, so
    /// the selection also moves away from or towards the image origin.
    /// With one (see [`scale_center`](Self::scale_center)) it is scaled
    /// about that point and, as in ImageJ, its vertices are rounded to
    /// whole pixels. Shapes other than lines, points and composites
    /// become polygons (ovals and traced outlines become freehand
    /// selections); a line's width is multiplied by `x_scale`.
    pub fn scaled(&self, x_scale: f64, y_scale: f64, center: Option<Point>) -> Roi {
        if let Roi::Composite { path } = self {
            let f = self.frame();
            let scaled = path.mapped(|x, y| {
                (
                    (f64::from(x) * x_scale) as f32,
                    (f64::from(y) * y_scale) as f32,
                )
            });
            if center.is_none() {
                return Roi::Composite { path: scaled };
            }
            // RoiScaler.scaleShape, centred: the shape is scaled about
            // its own corner and placed so that its bounds grow evenly.
            let local = path.local(f).mapped(|x, y| {
                (
                    (f64::from(x) * x_scale) as f32,
                    (f64::from(y) * y_scale) as f32,
                )
            });
            let lf = local.frame();
            let xbase = (f64::from(f.x) - (f64::from(f.w) * x_scale - f64::from(f.w)) / 2.0) as i32;
            let ybase = (f64::from(f.y) - (f64::from(f.h) * y_scale - f64::from(f.h)) / 2.0) as i32;
            return Roi::Composite {
                path: local.translated(
                    f64::from(xbase) - f64::from(lf.x),
                    f64::from(ybase) - f64::from(lf.y),
                ),
            };
        }
        let (mut xs, mut ys) = self.transform_points();
        for (x, y) in xs.iter_mut().zip(ys.iter_mut()) {
            match center {
                Some((xc, yc)) => {
                    *x = ((f64::from(*x) - xc) * x_scale + xc + 0.5).floor() as f32;
                    *y = ((f64::from(*y) - yc) * y_scale + yc + 0.5).floor() as f32;
                }
                None => {
                    *x = (f64::from(*x) * x_scale) as f32;
                    *y = (f64::from(*y) * y_scale) as f32;
                }
            }
        }
        let width = self.line_width();
        self.rebuilt(&xs, &ys, None)
            .with_line_width(if width > 1.0 {
                width * x_scale
            } else {
                width.max(1.0)
            })
    }

    /// The centre ImageJ rotates a selection about by default
    /// (`Roi.getRotationCenter`): the middle of its bounds, or of a
    /// straight line.
    pub fn rotation_center(&self) -> Point {
        let (x, y, w, h) = self.bounds();
        (x + w / 2.0, y + h / 2.0)
    }

    /// The selection turned by `angle` degrees about `(xcenter,
    /// ycenter)`, clockwise on screen for positive angles: Edit >
    /// Selection > Rotate (`RoiRotator.rotate`).
    ///
    /// Lines, points, rotated rectangles and composites keep their kind;
    /// a rectangle becomes a polygon, an oval, ellipse or traced outline
    /// a freehand selection.
    pub fn rotated(&self, angle: f64, xcenter: f64, ycenter: f64) -> Roi {
        let theta = -angle * PI / 180.0;
        if let Roi::Composite { path } = self {
            // AffineTransform.rotate(-theta, xcenter, ycenter)
            let (sin, cos) = (-theta).sin_cos();
            return Roi::Composite {
                path: path.mapped(|x, y| {
                    let (dx, dy) = (f64::from(x) - xcenter, f64::from(y) - ycenter);
                    (
                        (xcenter + dx * cos - dy * sin) as f32,
                        (ycenter + dx * sin + dy * cos) as f32,
                    )
                }),
            };
        }
        let rotated_rect_width = match self {
            Roi::RotatedRectangle { width, .. } => Some(*width),
            _ => None,
        };
        let (mut xs, mut ys) = match (self, self.axis_params()) {
            (Roi::RotatedRectangle { .. }, Some(p)) => (
                vec![p[0] as f32, p[2] as f32],
                vec![p[1] as f32, p[3] as f32],
            ),
            _ => self.transform_points(),
        };
        for (x, y) in xs.iter_mut().zip(ys.iter_mut()) {
            let dx = f64::from(*x) - xcenter;
            let dy = ycenter - f64::from(*y);
            let radius = (dx * dx + dy * dy).sqrt();
            let a = dy.atan2(dx);
            *x = (xcenter + radius * (a + theta).cos()) as f32;
            *y = (ycenter - radius * (a + theta).sin()) as f32;
        }
        self.rebuilt(&xs, &ys, rotated_rect_width)
    }

    /// The points `RoiScaler` and `RoiRotator` transform: a straight
    /// line's end points, otherwise the outline.
    fn transform_points(&self) -> (Vec<f32>, Vec<f32>) {
        match self {
            Roi::Line { x1, y1, x2, y2, .. } => {
                (vec![*x1 as f32, *x2 as f32], vec![*y1 as f32, *y2 as f32])
            }
            _ => self.float_polygon(),
        }
    }

    /// The selection `RoiScaler` and `RoiRotator` build from transformed
    /// points.
    fn rebuilt(&self, xs: &[f32], ys: &[f32], rotated_rect_width: Option<f64>) -> Roi {
        let d = f64::from;
        let points: Vec<Point> = xs.iter().zip(ys).map(|(&x, &y)| (d(x), d(y))).collect();
        match self {
            Roi::Line { width, .. } => Roi::Line {
                x1: d(xs[0]),
                y1: d(ys[0]),
                x2: d(xs[1]),
                y2: d(ys[1]),
                width: *width,
            },
            Roi::RotatedRectangle { .. } if rotated_rect_width.is_some() => Roi::rotated_rectangle(
                d(xs[0]),
                d(ys[0]),
                d(xs[1]),
                d(ys[1]),
                rotated_rect_width.unwrap_or(1.0),
            ),
            Roi::Points { .. } => Roi::Points { points },
            Roi::Polyline { width, .. } => Roi::Polyline {
                points,
                width: *width,
            },
            Roi::Freeline { width, .. } => Roi::Freeline {
                points,
                width: *width,
            },
            Roi::Angle { .. } if points.len() == 3 => Roi::Angle {
                points: [points[0], points[1], points[2]],
            },
            Roi::Oval { .. } | Roi::Traced { .. } | Roi::Freehand { .. } | Roi::Ellipse { .. } => {
                Roi::Freehand { points }
            }
            _ => Roi::Polygon { points },
        }
    }

    /// The area a line selection covers at its width: Edit > Selection >
    /// Line to Area (`Roi.convertLineToArea`). A straight line becomes a
    /// rotated rectangle; other lines are filled segment by segment and
    /// the result traced. Widths below one pixel count as one. `None`
    /// unless this is a line.
    pub fn to_area(&self) -> Option<Roi> {
        match self {
            Roi::Line {
                x1,
                y1,
                x2,
                y2,
                width,
            } => Some(Roi::rotated_rectangle(
                f64::from(*x1 as f32),
                f64::from(*y1 as f32),
                f64::from(*x2 as f32),
                f64::from(*y2 as f32),
                width.max(1.0),
            )),
            _ if self.is_line() => self.line_area(None),
            _ => None,
        }
    }

    /// `Roi.convertLineToArea` for segmented lines. With `clip`, only the
    /// part within an image of that size is produced.
    pub(crate) fn line_area(&self, clip: Option<(usize, usize)>) -> Option<Roi> {
        if let Roi::Line { .. } = self {
            return self.to_area();
        }
        let line_width = self.line_width().max(1.0);
        let f = self.frame();
        let mut w = i64::from(jround(f64::from(f.x) + f64::from(f.w) + line_width));
        let mut h = i64::from(jround(f64::from(f.y) + f64::from(f.h) + line_width));
        if w <= 0 || h <= 0 {
            return None;
        }
        let (full_w, full_h) = (
            w.min(i64::from(i32::MAX)) as i32,
            h.min(i64::from(i32::MAX)) as i32,
        );
        if let Some((cw, ch)) = clip {
            w = w.min(cw as i64);
            h = h.min(ch as i64);
        }
        if w <= 0 || h <= 0 || w * h > MAX_WORK_PIXELS {
            return None;
        }
        let window = Window {
            x: 0,
            y: 0,
            w: w as i32,
            h: h as i32,
        };
        let mut data = vec![0u8; (w * h) as usize];
        let (px, py) = self.float_polygon();
        let n = px.len();
        if n < 2 {
            return None;
        }
        let d = f64::from;
        let radius = line_width / 2.0;
        let unit = |i: usize| {
            let (dx, dy) = (d(px[i + 1]) - d(px[i]), d(py[i + 1]) - d(py[i]));
            let l = (dx * dx + dy * dy).sqrt();
            (dx / l, dy / l)
        };
        let mut fill_polygon = |x: &[f32], y: &[f32]| {
            let polygon = Polygon::Float {
                x,
                y,
                x_offset: 0.5,
                y_offset: 0.5,
            };
            fill::fill(&polygon, full_w, full_h, window, &mut data);
        };
        let (mut dx1, mut dy1) = unit(0);
        let (mut dx0, mut dy0) = (dx1, dy1);
        let mut xfrom = d(px[0]) - 0.5 * dx1;
        let mut yfrom = d(py[0]) - 0.5 * dy1;
        for i in 1..n {
            let (mut xto, mut yto) = (d(px[i]), d(py[i]));
            if i == n - 1 {
                xto += 0.5 * dx1;
                yto += 0.5 * dy1;
            }
            let xv = [
                (xfrom + radius * dy1) as f32,
                (xfrom - radius * dy1) as f32,
                (xto - radius * dy1) as f32,
                (xto + radius * dy1) as f32,
            ];
            let yv = [
                (yfrom - radius * dx1) as f32,
                (yfrom + radius * dx1) as f32,
                (yto + radius * dx1) as f32,
                (yto - radius * dx1) as f32,
            ];
            fill_polygon(&xv, &yv);
            if i > 1 {
                // The wedge between this segment and the previous one.
                let right_turn = dx1 * dy0 > dx0 * dy1;
                let mut xt = [xfrom as f32; 3];
                let mut yt = [yfrom as f32; 3];
                if right_turn {
                    xt[1] = (xfrom - radius * dy0) as f32;
                    yt[1] = (yfrom + radius * dx0) as f32;
                    xt[2] = (xfrom - radius * dy1) as f32;
                    yt[2] = (yfrom + radius * dx1) as f32;
                    xt[0] += (0.5 * (radius * dy0 + radius * dy1)) as f32;
                    yt[0] -= (0.5 * (radius * dx0 + radius * dx1)) as f32;
                } else {
                    xt[1] = (xfrom + radius * dy0) as f32;
                    yt[1] = (yfrom - radius * dx0) as f32;
                    xt[2] = (xfrom + radius * dy1) as f32;
                    yt[2] = (yfrom - radius * dx1) as f32;
                    xt[0] -= (0.5 * (radius * dy0 + radius * dy1)) as f32;
                    yt[0] += (0.5 * (radius * dx0 + radius * dx1)) as f32;
                }
                fill_polygon(&xt, &yt);
            }
            dx0 = dx1;
            dy0 = dy1;
            xfrom = xto;
            yfrom = yto;
            if i < n - 1 {
                (dx1, dy1) = unit(i);
            }
        }
        from_mask(&data, w as usize, h as usize, 0, 0)
    }

    /// The best-fit ellipse of the pixels this area covers in a `width` x
    /// `height` image: ImageJ's `EllipseFitter`. `None` if the selection
    /// is not an area or covers no pixel.
    pub fn ellipse_fit(&self, width: usize, height: usize) -> Option<EllipseFit> {
        // The value ImageJ's EllipseFitter uses, digits and all.
        #[allow(clippy::approx_constant)]
        const HALF_PI: f64 = 1.5707963267949;
        if !self.is_area() {
            return None;
        }
        let mask = self.mask(width, height)?;
        let r = mask.rect;
        if self.is_rectangular() {
            let sqrt_pi = 1.772453851;
            let mut major = (r.w * 2) as f64 / sqrt_pi;
            let mut minor = (r.h * 2) as f64 / sqrt_pi;
            let mut angle = 0.0;
            if major < minor {
                std::mem::swap(&mut major, &mut minor);
                angle = 90.0;
            }
            return Some(EllipseFit {
                x_center: r.x as f64 + r.w as f64 / 2.0,
                y_center: r.y as f64 + r.h as f64 / 2.0,
                major,
                minor,
                angle,
            });
        }
        // EllipseFitter.computeSums
        let (mut xsum, mut ysum, mut x2sum, mut y2sum, mut xysum) = (0.0f64, 0.0, 0.0, 0.0, 0.0);
        let mut bit_count = 0u64;
        for y in 0..r.h {
            let (mut count_of_line, mut x_sum_of_line) = (0u64, 0u64);
            for x in 0..r.w {
                if mask.data[y * r.w + x] != 0 {
                    count_of_line += 1;
                    x_sum_of_line += x as u64;
                    x2sum += (x * x) as f64;
                }
            }
            xsum += x_sum_of_line as f64;
            ysum += (count_of_line * y as u64) as f64;
            let ye = y as f64;
            xysum += x_sum_of_line as f64 * ye;
            y2sum += ye * ye * count_of_line as f64;
            bit_count += count_of_line;
        }
        if bit_count == 0 {
            return None;
        }
        // EllipseFitter.getMoments
        let n = bit_count as f64;
        x2sum += 0.08333333 * n;
        y2sum += 0.08333333 * n;
        let (xm, ym) = (xsum / n, ysum / n);
        let u20 = x2sum / n - xm * xm;
        let u02 = y2sum / n - ym * ym;
        let u11 = xysum / n - xm * ym;
        // EllipseFitter.getEllipseParam
        let mut m4 = 4.0 * (u02 * u20 - u11 * u11).abs();
        if m4 < 0.000001 {
            m4 = 0.000001;
        }
        let mut a11 = u02 / m4;
        let a12 = u11 / m4;
        let mut a22 = u20 / m4;
        let mut tmp = a11 - a22;
        if tmp == 0.0 {
            tmp = 0.000001;
        }
        let mut theta = 0.5 * (2.0 * a12 / tmp).atan();
        if theta < 0.0 {
            theta += HALF_PI;
        }
        if a12 > 0.0 {
            theta += HALF_PI;
        } else if a12 == 0.0 {
            if a22 > a11 {
                theta = 0.0;
                std::mem::swap(&mut a11, &mut a22);
            } else if a11 != a22 {
                theta = HALF_PI;
            }
        }
        tmp = theta.sin();
        if tmp == 0.0 {
            tmp = 0.000001;
        }
        let z = a12 * theta.cos() / tmp;
        let mut major = (1.0 / (a22 + z).abs()).sqrt();
        let mut minor = (1.0 / (a11 - z).abs()).sqrt();
        let scale = (n / (PI * major * minor)).sqrt();
        major *= scale * 2.0;
        minor *= scale * 2.0;
        let mut angle = 180.0 * theta / PI;
        if angle == 180.0 {
            angle = 0.0;
        }
        if major < minor {
            std::mem::swap(&mut major, &mut minor);
        }
        Some(EllipseFit {
            x_center: r.x as f64 + xm + 0.5,
            y_center: r.y as f64 + ym + 0.5,
            major,
            minor,
            angle,
        })
    }

    /// The best-fit ellipse as an ellipse selection: Edit > Selection >
    /// Fit Ellipse (`Selection.createEllipse`). The ellipse is centred on
    /// the centroid of the covered pixels and has the axes and direction
    /// of [`ellipse_fit`](Self::ellipse_fit).
    pub fn fitted_ellipse(&self, width: usize, height: usize) -> Option<Roi> {
        let fit = self.ellipse_fit(width, height)?;
        // ImageStatistics centroid: mean pixel position plus a half.
        let (xc, yc) = if self.is_rectangular() {
            (fit.x_center, fit.y_center)
        } else {
            self.scale_center(width, height)?
        };
        let dx = fit.major * (fit.angle / 180.0 * PI).cos() / 2.0;
        let dy = -fit.major * (fit.angle / 180.0 * PI).sin() / 2.0;
        Some(Roi::ellipse(
            xc - dx,
            yc - dy,
            xc + dx,
            yc + dy,
            fit.minor / fit.major,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inverse_of_everything_is_nothing() {
        assert_eq!(Roi::rectangle(0.0, 0.0, 8.0, 6.0).inverse(8, 6), None);
        assert_eq!(Roi::line(0.0, 0.0, 3.0, 3.0).inverse(8, 6), None);
        let inverse = Roi::rectangle(2.0, 1.0, 3.0, 2.0).inverse(8, 6).unwrap();
        assert_eq!(inverse.mask(8, 6).unwrap().count(), 48 - 6);
    }

    #[test]
    fn combining_areas() {
        let a = Roi::rectangle(0.0, 0.0, 10.0, 10.0);
        let b = Roi::rectangle(5.0, 5.0, 10.0, 10.0);
        let count = |how| a.combined(&b, how).unwrap().mask(20, 20).unwrap().count();
        assert_eq!(count(Combine::Or), 175);
        assert_eq!(count(Combine::And), 25);
        assert_eq!(count(Combine::Xor), 150);
        assert_eq!(count(Combine::Not), 75);
        assert_eq!(
            a.combined(&b, Combine::And),
            Some(Roi::rectangle(5.0, 5.0, 5.0, 5.0))
        );
        assert_eq!(a.combined(&a, Combine::Not), None);
    }

    #[test]
    fn enlarging_a_rectangle_moves_its_bounds() {
        let roi = Roi::rectangle(5.0, 5.0, 4.0, 3.0);
        assert_eq!(roi.enlarged(2), Some(Roi::rectangle(3.0, 3.0, 8.0, 7.0)));
        assert_eq!(roi.enlarged(-1), Some(Roi::rectangle(6.0, 6.0, 2.0, 1.0)));
        assert_eq!(roi.enlarged(-2), None);
        assert_eq!(Roi::line(0.0, 0.0, 3.0, 3.0).enlarged(2), None);
    }
}
