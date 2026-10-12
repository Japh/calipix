//! Which pixels each kind of selection covers, decided as ImageJ decides
//! it (`Roi.getMask()` and its overrides).

use calipix_core::Rect;

use crate::fill::{self, Polygon, Window};
use crate::model::{Coords, Frame, jround};
use crate::path::{self, FILL_FLATNESS, Path};
use crate::{Mask, Roi};

/// The part of `frame` inside a `width` x `height` image.
fn clip(frame: Frame, width: usize, height: usize) -> Option<Rect> {
    let x0 = i64::from(frame.x).max(0);
    let y0 = i64::from(frame.y).max(0);
    let x1 = (i64::from(frame.x) + i64::from(frame.w)).min(width.min(i32::MAX as usize) as i64);
    let y1 = (i64::from(frame.y) + i64::from(frame.h)).min(height.min(i32::MAX as usize) as i64);
    (x1 > x0 && y1 > y0).then(|| {
        Rect::new(
            x0 as usize,
            y0 as usize,
            (x1 - x0) as usize,
            (y1 - y0) as usize,
        )
    })
}

/// `OvalRoi.getMask`: a pixel is selected when its centre is inside the
/// ellipse inscribed in the frame, boundary included.
fn oval_mask(frame: Frame, window: Window, out: &mut [u8]) {
    let (mut a, mut b) = (f64::from(frame.w) / 2.0, f64::from(frame.h) / 2.0);
    let (a2, b2) = (a * a, b * b);
    a -= 0.5;
    b -= 0.5;
    for row in 0..window.h {
        let yy = f64::from(row + window.y) - b;
        for col in 0..window.w {
            let xx = f64::from(col + window.x) - a;
            if xx * xx / a2 + yy * yy / b2 <= 1.0 {
                out[(row * window.w + col) as usize] = 255;
            }
        }
    }
}

/// The mask of a shape given relative to `frame`, as `ShapeRoi.getMask`
/// makes it: curves flattened finely, then filled as polygons.
fn shape_mask(local: &Path, frame: Frame, window: Window, out: &mut [u8]) {
    let (xs, ys) = path::flatten_local(&local.steps(), frame, FILL_FLATNESS, true, false);
    let polygon = Polygon::Float {
        x: &xs,
        y: &ys,
        x_offset: 0.0,
        y_offset: 0.0,
    };
    fill::fill(&polygon, frame.w, frame.h, window, out);
}

impl Roi {
    /// Fills `out` (the `window` part of the selection's frame) with the
    /// selection's mask. Returns false for a plain rectangle, which has
    /// no mask in ImageJ: every pixel of its frame is selected.
    fn local_mask(&self, frame: Frame, window: Window, out: &mut [u8]) -> bool {
        match self {
            Roi::Rectangle { .. } => return false,
            Roi::RoundedRectangle {
                corner_diameter, ..
            } => {
                // new ShapeRoi(new RoundRectangle2D.Float(x, y, width,
                // height, d, d)).getMask()
                let (x, y) = (f64::from(frame.x), f64::from(frame.y));
                let steps = path::round_rect_path(
                    x,
                    y,
                    f64::from(frame.w),
                    f64::from(frame.h),
                    f64::from(*corner_diameter),
                );
                shape_mask(&path::to_path(&steps, -x, -y), frame, window, out);
            }
            Roi::Oval { .. } => oval_mask(frame, window, out),
            Roi::Composite { path } => {
                let local = path.translated(-f64::from(frame.x), -f64::from(frame.y));
                shape_mask(&local, frame, window, out);
            }
            Roi::Points { .. } => {
                // PointRoi.getMask
                let poly = self.poly().expect("points are polygon-like");
                let dx = poly.xbase() - f64::from(frame.x);
                let dy = poly.ybase() - f64::from(frame.y);
                if let Coords::Float(xs, ys) = &poly.coords {
                    for (&x, &y) in xs.iter().zip(ys) {
                        let px = jround(f64::from(x) + dx);
                        let py = jround(f64::from(y) + dy);
                        let inside_frame = px >= 0 && py >= 0 && px < frame.w && py < frame.h;
                        let (wx, wy) = (px - window.x, py - window.y);
                        if inside_frame && wx >= 0 && wy >= 0 && wx < window.w && wy < window.h {
                            out[(wy * window.w + wx) as usize] = 255;
                        }
                    }
                }
            }
            _ => {
                // PolygonRoi.getMask
                let poly = self.poly().expect("polygon-like selection");
                let polygon = match &poly.coords {
                    Coords::Int(x, y) => Polygon::Int { x, y },
                    Coords::Float(x, y) => Polygon::Float {
                        x,
                        y,
                        x_offset: poly.xbase() - f64::from(frame.x),
                        y_offset: poly.ybase() - f64::from(frame.y),
                    },
                };
                fill::fill(&polygon, frame.w, frame.h, window, out);
            }
        }
        true
    }

    /// The whole pixels of the selection's bounding rectangle that lie
    /// within a `width` x `height` image, or `None` if there are none.
    ///
    /// The rectangle is ImageJ's integer `Roi.getBounds()`. For a
    /// rectangle or oval given with fractions, ImageJ truncates the corner
    /// and rounds the size up; other area selections get the smallest
    /// rectangle of whole pixels containing every vertex.
    pub fn pixel_bounds(&self, width: usize, height: usize) -> Option<Rect> {
        clip(self.frame(), width, height)
    }

    /// Which pixels of a `width` x `height` image the selection covers,
    /// exactly as ImageJ's `Roi.getMask()` and `ImageProcessor.fill(Roi)`
    /// decide it. Returns `None` when no pixel is covered.
    ///
    /// * Rectangle: every pixel of [`pixel_bounds`](Self::pixel_bounds).
    /// * Oval: pixel centres inside the inscribed ellipse
    ///   (`OvalRoi.getMask`).
    /// * Polygon, freehand, traced, ellipse, rotated rectangle: ImageJ's
    ///   `PolygonFiller` (even-odd rule, pixel centres).
    /// * Rounded rectangle and composite: curves flattened to within 0.01
    ///   pixel, then `PolygonFiller` (`ShapeRoi.getMask`).
    /// * Points: the pixel of each point (`PointRoi.getMask`).
    /// * Lines wider than one pixel: the area [`to_area`](Self::to_area)
    ///   converts them to. Thinner lines cover no area and give `None`;
    ///   see [`line_pixels`](Self::line_pixels) for the pixels they pass
    ///   through.
    pub fn mask(&self, width: usize, height: usize) -> Option<Mask> {
        if self.is_line() {
            if self.line_width() <= 1.0 {
                return None;
            }
            return self.line_area(Some((width, height)))?.mask(width, height);
        }
        let frame = self.frame();
        let rect = clip(frame, width, height)?;
        let window = Window {
            x: (rect.x as i64 - i64::from(frame.x)) as i32,
            y: (rect.y as i64 - i64::from(frame.y)) as i32,
            w: rect.w as i32,
            h: rect.h as i32,
        };
        let mut data = vec![0u8; rect.w * rect.h];
        if !self.local_mask(frame, window, &mut data) {
            data.fill(255);
        }
        data.iter()
            .any(|&v| v != 0)
            .then_some(Mask { rect, data })
    }

    /// The mask of the whole selection, not clipped to any image, with
    /// the position of its top-left pixel. Used where ImageJ works on the
    /// selection alone (outline of an oval, enlarging).
    pub(crate) fn full_mask(&self) -> (Frame, Vec<u8>) {
        let frame = self.frame();
        let (w, h) = (frame.w.max(0), frame.h.max(0));
        let window = Window { x: 0, y: 0, w, h };
        let mut data = vec![0u8; w as usize * h as usize];
        if !self.local_mask(frame, window, &mut data) {
            data.fill(255);
        }
        (Frame { w, h, ..frame }, data)
    }
}
