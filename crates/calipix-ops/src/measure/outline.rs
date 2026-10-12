//! What measurements need to know about a selection's outline: its
//! perimeter, Feret diameters, convex hull and bounds.
//!
//! [`outline_of`] is the one place that turns a [`Roi`] into an
//! [`Outline`]. The geometry itself (`Roi.getLength`, `getFeretValues`,
//! `getFloatConvexHull`, `getBounds`) is `calipix-roi`'s, which is the one
//! source of truth for it; this module only arranges it the way
//! `ij.plugin.filter.Analyzer` asks for it.

use calipix_core::Result;
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

    fn of(roi: &Roi, pw: f64, ph: f64) -> Feret {
        roi.feret_scaled(pw, ph).map_or(Feret::NAN, |f| Feret {
            diameter: f.max,
            angle: f.angle,
            min: f.min,
            x: f.x,
            y: f.y,
        })
    }
}

/// What kind of thing a selection is, which decides how `Analyzer`
/// measures it.
#[derive(Clone, Debug, PartialEq)]
pub enum OutlineKind {
    /// No selection: the whole image.
    Whole,
    /// An area selection.
    Area,
    /// A straight, segmented or freehand line (`Analyzer.measureLength`).
    Line,
    /// An angle selection and the angle between its arms in degrees
    /// (`Analyzer.measureAngle`).
    Angle(f64),
    /// One or more points, as `Roi.getFloatPolygon()` lists them, which is
    /// in single precision (`Analyzer.measurePoint`).
    Points(Vec<(f64, f64)>),
}

/// The outline quantities of one selection, as ImageJ's `Roi` reports them
/// to `Analyzer.saveResults`.
#[derive(Clone, Debug, PartialEq)]
pub struct Outline {
    pub kind: OutlineKind,
    /// The selection itself; `None` without one.
    pub roi: Option<Roi>,
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
            roi: None,
            perimeter: w * 2.0 + h * 2.0,
            feret: Feret::of(&Roi::rectangle(0.0, 0.0, w, h), 1.0, 1.0),
            convex_area: None,
            bounds: (0.0, 0.0, w, h),
        }
    }

    /// A single point (`new PointRoi(x, y)` in `Analyzer.measurePoint`):
    /// each point of a point selection is measured as its own selection,
    /// one that belongs to no image and so has no calibration.
    pub fn point(x: f64, y: f64) -> Outline {
        let roi = Roi::point(x, y);
        Outline {
            kind: OutlineKind::Points(vec![(x, y)]),
            perimeter: roi.length(),
            feret: Feret::of(&roi, 1.0, 1.0),
            convex_area: Some(hull_area(&roi)),
            bounds: int_bounds(&roi),
            roi: Some(roi),
        }
    }

    /// True for an area selection or no selection (`roi==null ||
    /// roi.isArea()`).
    pub fn is_area(&self) -> bool {
        matches!(self.kind, OutlineKind::Whole | OutlineKind::Area)
    }

    /// True for lines and angles (`Roi.isLine()`, which in ImageJ 1.54p
    /// includes angle selections).
    pub fn is_line(&self) -> bool {
        matches!(self.kind, OutlineKind::Line | OutlineKind::Angle(_))
    }

    /// The end points of a straight line selection.
    pub fn straight_line(&self) -> Option<((f64, f64), (f64, f64))> {
        match (&self.kind, &self.roi) {
            (OutlineKind::Line, Some(Roi::Line { x1, y1, x2, y2, .. })) => {
                Some(((*x1, *y1), (*x2, *y2)))
            }
            _ => None,
        }
    }

    /// Angle of a straight line selection to the horizontal, in degrees
    /// (`Line.getFloatAngle`, which uses the calibration when pixels are
    /// not square).
    pub fn line_angle(&self, cal: &Calibration) -> Option<f64> {
        let ((x1, y1), (x2, y2)) = self.straight_line()?;
        let dx = (x2 - x1) * cal.pixel_width;
        let dy = (y1 - y2) * cal.pixel_height;
        Some(dy.atan2(dx).to_degrees())
    }

    /// The width of a line selection as `Analyzer` and `ProfilePlot` take
    /// it: `(int)Math.round(roi.getStrokeWidth())`. 0 for other kinds.
    pub fn line_width(&self) -> i64 {
        match (&self.kind, &self.roi) {
            (OutlineKind::Line, Some(roi)) => (roi.line_width() + 0.5).floor() as i64,
            _ => 0,
        }
    }

    /// The length and width of a rotated rectangle, which `Analyzer`
    /// reports as RRLength and RRWidth: in calibrated units when pixels
    /// are square, otherwise in pixels.
    pub fn rotated_rectangle(&self, cal: &Calibration) -> Option<(f64, f64)> {
        let Some(&Roi::RotatedRectangle {
            x1,
            y1,
            x2,
            y2,
            width,
        }) = self.roi.as_ref()
        else {
            return None;
        };
        let pw = if cal.pixel_width == cal.pixel_height {
            cal.pixel_width
        } else {
            1.0
        };
        let length = ((x2 - x1).powi(2) + (y2 - y1).powi(2)).sqrt();
        Some((length * pw, width * pw))
    }
}

/// The outline quantities of `roi` on an image with calibration `cal`.
///
/// This is the seam between measurements and selection shapes: every kind
/// of selection is described through `calipix-roi`'s geometry, which
/// follows the ImageJ class behind each kind.
pub fn outline_of(roi: &Roi, cal: &Calibration) -> Result<Outline> {
    let (pw, ph) = (cal.pixel_width, cal.pixel_height);
    let kind = match roi {
        Roi::Points { .. } => OutlineKind::Points(roi.outline()),
        Roi::Angle { .. } => OutlineKind::Angle(roi.angle().unwrap_or(f64::NAN)),
        Roi::Line { .. } | Roi::Polyline { .. } | Roi::Freeline { .. } => OutlineKind::Line,
        _ => OutlineKind::Area,
    };
    Ok(Outline {
        kind,
        roi: Some(roi.clone()),
        perimeter: roi.length_scaled(pw, ph),
        feret: Feret::of(roi, pw, ph),
        convex_area: Some(hull_area(roi)),
        bounds: int_bounds(roi),
    })
}

fn int_bounds(roi: &Roi) -> (f64, f64, f64, f64) {
    let (x, y, w, h) = roi.int_bounds();
    (f64::from(x), f64::from(y), f64::from(w), f64::from(h))
}

/// `Analyzer.getArea(roi.getConvexHull())`: the area of the convex hull
/// after rounding its vertices to whole pixels (`Roi.toIntR`), summed in
/// 32-bit integers as Java does.
fn hull_area(roi: &Roi) -> f64 {
    let hull: Vec<(i32, i32)> = roi
        .convex_hull()
        .iter()
        .map(|&(x, y)| ((x + 0.5).floor() as i32, (y + 0.5).floor() as i32))
        .collect();
    let mut area = 0i32;
    for (i, &(x, y)) in hull.iter().enumerate() {
        let (px, py) = hull[if i == 0 { hull.len() - 1 } else { i - 1 }];
        area = area.wrapping_add(x.wrapping_add(px).wrapping_mul(y.wrapping_sub(py)));
    }
    (f64::from(area) / 2.0).abs()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rectangle_outline() {
        let cal = Calibration {
            pixel_width: 0.5,
            pixel_height: 0.25,
            ..Calibration::default()
        };
        let o = outline_of(&Roi::rectangle(8.0, 6.0, 20.0, 10.0), &cal).unwrap();
        assert_eq!(o.perimeter, 25.0);
        assert_eq!(o.convex_area, Some(200.0));
        assert_eq!(o.feret.min, 2.5);
        assert_eq!((o.feret.x, o.feret.y), (8.0, 6.0));
        assert_eq!(o.bounds, (8.0, 6.0, 20.0, 10.0));
        assert!(o.is_area() && !o.is_line());
        assert_eq!(o.line_angle(&cal), None);
    }

    #[test]
    fn every_kind_has_an_outline() {
        let cal = Calibration::default();
        let line = outline_of(&Roi::line(1.0, 2.0, 4.0, 6.0), &cal).unwrap();
        assert_eq!(line.kind, OutlineKind::Line);
        assert_eq!(line.perimeter, 5.0);
        assert!(line.is_line() && !line.is_area());
        assert_eq!(line.straight_line(), Some(((1.0, 2.0), (4.0, 6.0))));

        let bent = outline_of(
            &Roi::polyline(vec![(0.0, 0.0), (3.0, 4.0), (3.0, 10.0)]),
            &cal,
        )
        .unwrap();
        assert_eq!((&bent.kind, bent.perimeter), (&OutlineKind::Line, 11.0));
        assert_eq!(bent.straight_line(), None);

        let angle = outline_of(
            &Roi::Angle {
                points: [(10.0, 0.0), (0.0, 0.0), (0.0, 10.0)],
            },
            &cal,
        )
        .unwrap();
        assert_eq!(angle.kind, OutlineKind::Angle(90.0));
        assert!(angle.is_line());
        assert_eq!(angle.line_width(), 0);

        let points = outline_of(&Roi::points(vec![(1.0, 2.0), (3.5, 4.0)]), &cal).unwrap();
        assert_eq!(
            points.kind,
            OutlineKind::Points(vec![(1.0, 2.0), (3.5, 4.0)])
        );

        let wide = outline_of(&Roi::line(0.0, 0.0, 10.0, 0.0).with_line_width(4.4), &cal).unwrap();
        assert_eq!(wide.line_width(), 4);

        let turned = outline_of(&Roi::rotated_rectangle(0.0, 0.0, 3.0, 4.0, 2.0), &cal).unwrap();
        assert_eq!(turned.rotated_rectangle(&cal), Some((5.0, 2.0)));
        assert!(turned.is_area());
    }
}
