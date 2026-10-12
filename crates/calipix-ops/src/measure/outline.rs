//! What measurements need to know about a selection's outline: its
//! perimeter, Feret diameters and convex hull.
//!
//! [`outline_of`] is the one place that turns a [`Roi`] into an
//! [`Outline`]. It handles rectangles today; the other shapes plug in there
//! as `calipix-roi` gains them, using the constructors below, which already
//! reproduce ImageJ for polygons, ovals, lines and points given the
//! outline's vertices.

use calipix_core::{Error, Result};
use calipix_roi::Roi;

use super::Calibration;

/// Feret values in the order of the first five entries of
/// `Roi.getFeretValues()`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Feret {
    /// The largest distance between two points of the outline.
    pub diameter: f64,
    /// Angle of that diameter, from 0 to 180 degrees.
    pub angle: f64,
    /// The smallest caliper width.
    pub min: f64,
    /// Where the diameter starts.
    pub x: f64,
    pub y: f64,
}

impl Feret {
    const NAN: Feret = Feret {
        diameter: f64::NAN,
        angle: f64::NAN,
        min: f64::NAN,
        x: f64::NAN,
        y: f64::NAN,
    };
}

/// What kind of thing a selection is, which decides how `Analyzer`
/// measures it.
#[derive(Clone, Debug, PartialEq)]
pub enum OutlineKind {
    /// No selection: the whole image.
    Whole,
    /// An area selection.
    Area,
    /// A straight line from `(x1, y1)` to `(x2, y2)`.
    Line { x1: f64, y1: f64, x2: f64, y2: f64 },
    /// One or more points.
    Points(Vec<(f64, f64)>),
}

/// The outline quantities of one selection, as ImageJ's `Roi` reports them
/// to `Analyzer.saveResults`.
#[derive(Clone, Debug, PartialEq)]
pub struct Outline {
    pub kind: OutlineKind,
    /// `Roi.getLength()`: the perimeter of an area or the length of a line,
    /// in calibrated units.
    pub perimeter: f64,
    /// `Roi.getFeretValues()`.
    pub feret: Feret,
    /// Area of the convex hull in pixels (`Analyzer.getArea` of
    /// `Roi.getConvexHull()`); `None` uses the pixel count, as ImageJ does
    /// without a selection.
    pub convex_area: Option<f64>,
    /// `Roi.getBounds()`: x, y, width, height in pixels.
    pub bounds: (f64, f64, f64, f64),
}

impl Outline {
    /// No selection on a `width` x `height` image. ImageJ then reports the
    /// perimeter and Feret values of the image rectangle in pixels,
    /// whatever the calibration.
    pub fn whole(width: usize, height: usize) -> Outline {
        let (w, h) = (width as f64, height as f64);
        Outline {
            kind: OutlineKind::Whole,
            perimeter: w * 2.0 + h * 2.0,
            feret: feret(&rectangle_points(0.0, 0.0, w, h), 1.0, 1.0),
            convex_area: None,
            bounds: (0.0, 0.0, w, h),
        }
    }

    /// A rectangle (`ij.gui.Roi`).
    pub fn rectangle(x: f64, y: f64, width: f64, height: f64, cal: &Calibration) -> Outline {
        let (pw, ph) = (cal.pixel_width, cal.pixel_height);
        let points = rectangle_points(x, y, width, height);
        Outline {
            kind: OutlineKind::Area,
            perimeter: 2.0 * width * pw + 2.0 * height * ph,
            feret: feret(&points, pw, ph),
            convex_area: Some(hull_area(&points)),
            bounds: (x, y, width, height),
        }
    }

    /// An area whose outline is the polygon `points`
    /// (`Roi.getFloatPolygon("")`, without a repeated closing point), with
    /// the perimeter ImageJ gives for the shape (`Roi.getLength()`, which
    /// differs between polygons, traced and freehand outlines).
    pub fn polygon(
        points: &[(f32, f32)],
        perimeter: f64,
        bounds: (f64, f64, f64, f64),
        cal: &Calibration,
    ) -> Outline {
        Outline {
            kind: OutlineKind::Area,
            perimeter,
            feret: feret(points, cal.pixel_width, cal.pixel_height),
            convex_area: Some(hull_area(points)),
            bounds,
        }
    }

    /// An oval inscribed in the given rectangle (`ij.gui.OvalRoi`), whose
    /// outline polygon is `points`. The perimeter is Ramanujan's
    /// approximation and the Feret values are the ellipse's own axes.
    pub fn oval(
        x: f64,
        y: f64,
        width: f64,
        height: f64,
        points: &[(f32, f32)],
        cal: &Calibration,
    ) -> Outline {
        let (pw, ph) = (cal.pixel_width, cal.pixel_height);
        let (a, b) = (0.5 * width * pw, 0.5 * height * ph);
        let tall = ph * height > pw * width;
        Outline {
            kind: OutlineKind::Area,
            perimeter: std::f64::consts::PI
                * (3.0 * (a + b) - ((3.0 * a + b) * (a + 3.0 * b)).sqrt()),
            feret: Feret {
                diameter: if tall { height * ph } else { width * pw },
                angle: if tall { 90.0 } else { 0.0 },
                min: if tall { width * pw } else { height * ph },
                x: (x + if tall { 0.5 * width } else { 0.0 }) * pw,
                y: (y + if tall { height } else { 0.5 * height }) * ph,
            },
            convex_area: Some(hull_area(points)),
            bounds: (x, y, width, height),
        }
    }

    /// A straight line one pixel wide (`ij.gui.Line`).
    pub fn line(x1: f64, y1: f64, x2: f64, y2: f64, cal: &Calibration) -> Outline {
        let (pw, ph) = (cal.pixel_width, cal.pixel_height);
        let points = [(x1 as f32, y1 as f32), (x2 as f32, y2 as f32)];
        Outline {
            kind: OutlineKind::Line { x1, y1, x2, y2 },
            perimeter: (((x2 - x1) * pw).powi(2) + ((y2 - y1) * ph).powi(2)).sqrt(),
            feret: feret(&points, pw, ph),
            convex_area: Some(hull_area(&points)),
            bounds: line_bounds(&[(x1, y1), (x2, y2)]),
        }
    }

    /// A single point (`ij.gui.PointRoi`). Each point of a multi-point
    /// selection is measured as its own single point.
    pub fn point(x: f64, y: f64) -> Outline {
        let points = [(x as f32, y as f32)];
        Outline {
            kind: OutlineKind::Points(vec![(x, y)]),
            perimeter: 0.0,
            feret: feret(&points, 1.0, 1.0),
            convex_area: Some(0.0),
            bounds: (x, y, 0.0, 0.0),
        }
    }

    /// True for an area selection or no selection (`roi==null ||
    /// roi.isArea()`).
    pub fn is_area(&self) -> bool {
        matches!(self.kind, OutlineKind::Whole | OutlineKind::Area)
    }

    /// Angle of a line selection to the horizontal, in degrees
    /// (`Roi.getFloatAngle`).
    pub fn line_angle(&self, cal: &Calibration) -> Option<f64> {
        match self.kind {
            OutlineKind::Line { x1, y1, x2, y2 } => {
                let dx = (x2 - x1) * cal.pixel_width;
                let dy = (y1 - y2) * cal.pixel_height;
                Some(dy.atan2(dx).to_degrees())
            }
            _ => None,
        }
    }
}

/// The outline quantities of `roi` on an image with calibration `cal`.
///
/// This is the seam between measurements and selection shapes. Each shape
/// `calipix-roi` adds needs one arm here, built from:
///
/// * polygons, traced and freehand areas: [`Outline::polygon`] with the
///   shape's vertices and its ImageJ perimeter;
/// * ovals and ellipses: [`Outline::oval`];
/// * straight lines: [`Outline::line`];
/// * points: [`Outline::point`] per point.
pub fn outline_of(roi: &Roi, cal: &Calibration) -> Result<Outline> {
    #[allow(unreachable_patterns)] // `Roi` gains shapes in another crate.
    match *roi {
        Roi::Rectangle {
            x,
            y,
            width,
            height,
        } => Ok(Outline::rectangle(x, y, width, height, cal)),
        _ => Err(Error::Unsupported(format!(
            "the outline of the selection `{roi}` cannot be measured yet"
        ))),
    }
}

fn rectangle_points(x: f64, y: f64, w: f64, h: f64) -> [(f32, f32); 4] {
    let (x0, y0, x1, y1) = (x as f32, y as f32, (x + w) as f32, (y + h) as f32);
    [(x0, y0), (x1, y0), (x1, y1), (x0, y1)]
}

/// `Roi.setIntBounds` for lines and points: the pixels drawing would touch.
fn line_bounds(points: &[(f64, f64)]) -> (f64, f64, f64, f64) {
    let fold = |f: fn(f64, f64) -> f64, pick: fn(&(f64, f64)) -> f64| {
        points.iter().map(pick).reduce(f).unwrap_or(0.0)
    };
    let (min_x, max_x) = (fold(f64::min, |p| p.0), fold(f64::max, |p| p.0));
    let (min_y, max_y) = (fold(f64::min, |p| p.1), fold(f64::max, |p| p.1));
    let x = (min_x + 0.5).floor();
    let y = (min_y + 0.5).floor();
    (x, y, (max_x + 1.5).floor() - x, (max_y + 1.5).floor() - y)
}

/// `FloatPolygon.getConvexHull`: the convex hull of `points` by gift
/// wrapping, counter-clockwise on screen, starting from the leftmost of the
/// topmost points and without collinear points.
pub fn convex_hull(points: &[(f32, f32)]) -> Vec<(f32, f32)> {
    let n = points.len();
    if n == 0 {
        return Vec::new();
    }
    let smallest_y = points.iter().map(|p| p.1).fold(f32::MAX, f32::min);
    let mut smallest_x = f32::MAX;
    let mut p1 = 0;
    for (i, &(x, y)) in points.iter().enumerate() {
        if y == smallest_y && x < smallest_x {
            smallest_x = x;
            p1 = i;
        }
    }
    let start = p1;
    let mut hull: Vec<(f32, f32)> = Vec::new();
    let next = |i: usize| if i + 1 == n { 0 } else { i + 1 };
    loop {
        let (x1, y1) = (points[p1].0 as f64, points[p1].1 as f64);
        // The first point after p1 that differs from it is the candidate.
        let mut p2 = p1;
        let (mut x2, mut y2) = (0.0, 0.0);
        loop {
            p2 = next(p2);
            if p2 == p1 {
                break;
            }
            x2 = points[p2].0 as f64;
            y2 = points[p2].1 as f64;
            if !(x2 == x1 && y2 == y1) {
                break;
            }
        }
        if p2 != p1 {
            let mut p3 = next(p2);
            loop {
                let (x3, y3) = (points[p3].0 as f64, points[p3].1 as f64);
                // Positive if the path p1, p2, p3 bends clockwise at p2.
                let determinant = x1 * (y2 - y3) - y1 * (x2 - x3) + (y3 * x2 - y2 * x3);
                let mut further = false;
                if determinant == 0.0 && p3 != p2 {
                    let d2 = (x2 - x1).powi(2) + (y2 - y1).powi(2);
                    let d3 = (x3 - x1).powi(2) + (y3 - y1).powi(2);
                    further = d3 > d2;
                }
                if determinant > 0.0 || further {
                    x2 = x3;
                    y2 = y3;
                    p2 = p3;
                }
                p3 = next(p3);
                if p3 == p1 {
                    break;
                }
            }
        }
        hull.push((x1 as f32, y1 as f32));
        if p2 == p1 {
            break; // every point is the same point
        }
        p1 = p2;
        if hull.len() > 1 && points[p1] == hull[0] {
            break; // back at the start through a duplicate of it
        }
        if p1 == start {
            break;
        }
    }
    hull
}

/// `Analyzer.getArea(roi.getConvexHull())`: the area of the convex hull
/// after rounding its vertices to whole pixels, summed in integers.
pub fn hull_area(points: &[(f32, f32)]) -> f64 {
    let hull: Vec<(i64, i64)> = convex_hull(points)
        .iter()
        .map(|&(x, y)| ((x + 0.5).floor() as i64, (y + 0.5).floor() as i64))
        .collect();
    let mut area = 0i64;
    for (i, &(x, y)) in hull.iter().enumerate() {
        let (px, py) = hull[if i == 0 { hull.len() - 1 } else { i - 1 }];
        area += (x + px) * (y - py);
    }
    (area as f64 / 2.0).abs()
}

/// `Roi.getFeretValues()` for the outline `points` and pixel size
/// `pw` x `ph`: the largest distance between two vertices of the convex
/// hull, its angle and starting point (the end with the smaller X, in
/// pixels), and the smallest caliper width.
pub fn feret(points: &[(f32, f32)], pw: f64, ph: f64) -> Feret {
    let hull: Vec<(f64, f64)> = convex_hull(points)
        .iter()
        .map(|&(x, y)| (x as f64, y as f64))
        .collect();
    if hull.is_empty() {
        return Feret::NAN;
    }
    let (mut p1, mut p2) = (0, 0);
    let mut diameter_sqr = 0.0;
    for i in 0..hull.len() {
        for j in i + 1..hull.len() {
            let dx = (hull[i].0 - hull[j].0) * pw;
            let dy = (hull[i].1 - hull[j].1) * ph;
            let d = dx * dx + dy * dy;
            if d > diameter_sqr {
                diameter_sqr = d;
                p1 = i;
                p2 = j;
            }
        }
    }
    if hull[p1].0 > hull[p2].0 {
        std::mem::swap(&mut p1, &mut p2);
    }
    let ((xf1, yf1), (xf2, yf2)) = (hull[p1], hull[p2]);
    let mut angle = ((yf1 - yf2) * ph).atan2((xf2 - xf1) * pw).to_degrees();
    if angle < 0.0 {
        angle += 180.0;
    }

    // For each side of the hull, the distance of the vertex furthest from
    // it; the smallest of these is the minimum caliper width.
    let (mut x0, mut y0) = hull[hull.len() - 1];
    let mut min_feret = f64::MAX;
    for &(x, y) in &hull {
        let (xprev, yprev) = (x0, y0);
        (x0, y0) = (x, y);
        let mut xnorm = (y0 - yprev) * ph;
        let mut ynorm = (xprev - x0) * pw;
        let normalization = 1.0 / (xnorm * xnorm + ynorm * ynorm).sqrt();
        xnorm *= normalization * pw;
        ynorm *= normalization * ph;
        let mut max_dist = 0.0;
        for &(x1, y1) in &hull {
            let dist = (x1 - x0) * xnorm + (y1 - y0) * ynorm;
            if dist > max_dist {
                max_dist = dist;
            }
        }
        if max_dist < min_feret {
            min_feret = max_dist;
        }
    }
    Feret {
        diameter: diameter_sqr.sqrt(),
        angle,
        min: min_feret,
        x: xf1,
        y: yf1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hull_drops_inner_and_collinear_points() {
        let points = [
            (0.0, 0.0),
            (2.0, 0.0),
            (4.0, 0.0),
            (4.0, 4.0),
            (2.0, 2.0),
            (0.0, 4.0),
        ];
        let hull = convex_hull(&points);
        assert_eq!(hull, [(0.0, 0.0), (0.0, 4.0), (4.0, 4.0), (4.0, 0.0)]);
        assert_eq!(hull_area(&points), 16.0);
        assert_eq!(convex_hull(&[(3.0, 1.0), (3.0, 1.0)]), [(3.0, 1.0)]);
        assert!(convex_hull(&[]).is_empty());
    }

    #[test]
    fn rectangle_outline() {
        let cal = Calibration {
            pixel_width: 0.5,
            pixel_height: 0.25,
            ..Calibration::default()
        };
        let o = Outline::rectangle(8.0, 6.0, 20.0, 10.0, &cal);
        assert_eq!(o.perimeter, 25.0);
        assert_eq!(o.convex_area, Some(200.0));
        assert_eq!(o.feret.min, 2.5);
        assert_eq!((o.feret.x, o.feret.y), (8.0, 6.0));
        assert!(o.is_area());
        assert_eq!(o.line_angle(&cal), None);
    }

    #[test]
    fn only_rectangles_have_outlines_so_far() {
        let roi = Roi::rectangle(1.0, 2.0, 3.0, 4.0);
        let o = outline_of(&roi, &Calibration::default()).unwrap();
        assert_eq!(o.bounds, (1.0, 2.0, 3.0, 4.0));
        assert_eq!(o.perimeter, 14.0);
    }
}
