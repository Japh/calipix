//! View geometry: zoom, pan and the mapping between screen pixels and image
//! pixels. Pure arithmetic with no dependency on egui or the GPU.
//!
//! Screen coordinates are physical pixels (not egui points), so that 100%
//! means one image pixel on one device pixel.

use imager_core::Rect;

/// Zoom steps used by the wheel and by zoom in/out, as magnification
/// factors. These are ImageJ's (`ImageCanvas.zoomLevels`).
pub const ZOOM_LEVELS: [f64; 23] = [
    1.0 / 72.0,
    1.0 / 48.0,
    1.0 / 32.0,
    1.0 / 24.0,
    1.0 / 16.0,
    1.0 / 12.0,
    1.0 / 8.0,
    1.0 / 6.0,
    1.0 / 4.0,
    1.0 / 3.0,
    1.0 / 2.0,
    0.75,
    1.0,
    1.5,
    2.0,
    3.0,
    4.0,
    6.0,
    8.0,
    12.0,
    16.0,
    24.0,
    32.0,
];

/// Largest magnification.
pub const MAX_ZOOM: f64 = 32.0;

/// A pixel centre that lies within this fraction of a texel below a texel
/// boundary is treated as lying on the boundary and shows the texel after
/// it. This makes the choice at exact ties (for example every third screen
/// pixel at 150%) independent of single-precision rounding in the shader.
pub const TIE_BIAS: f64 = 1.0 / 512.0;

/// The next zoom step above `zoom`, or `zoom` itself at the maximum.
pub fn zoom_step_in(zoom: f64) -> f64 {
    ZOOM_LEVELS
        .iter()
        .copied()
        .find(|&level| level > zoom * (1.0 + 1e-9))
        .unwrap_or(zoom.max(MAX_ZOOM))
}

/// The next zoom step below `zoom`, but not below `min_zoom`.
pub fn zoom_step_out(zoom: f64, min_zoom: f64) -> f64 {
    let next = ZOOM_LEVELS
        .iter()
        .rev()
        .copied()
        .find(|&level| level < zoom * (1.0 - 1e-9))
        .unwrap_or(ZOOM_LEVELS[0]);
    if zoom <= min_zoom {
        zoom
    } else {
        next.max(min_zoom)
    }
}

/// The part of the screen the image is drawn into, in physical pixels.
/// `min` and `size` are whole numbers.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Viewport {
    pub min: [f64; 2],
    pub size: [f64; 2],
}

impl Viewport {
    pub fn new(x: f64, y: f64, w: f64, h: f64) -> Viewport {
        Viewport {
            min: [x, y],
            size: [w, h],
        }
    }

    /// The pixel rectangle covered by an egui rectangle in points, with its
    /// edges rounded to whole pixels.
    pub fn from_points(rect: egui::Rect, pixels_per_point: f32) -> Viewport {
        let ppp = pixels_per_point as f64;
        let x0 = (rect.min.x as f64 * ppp).round();
        let y0 = (rect.min.y as f64 * ppp).round();
        let x1 = (rect.max.x as f64 * ppp).round();
        let y1 = (rect.max.y as f64 * ppp).round();
        Viewport::new(x0, y0, (x1 - x0).max(0.0), (y1 - y0).max(0.0))
    }

    pub fn center(&self) -> [f64; 2] {
        [
            self.min[0] + self.size[0] / 2.0,
            self.min[1] + self.size[1] / 2.0,
        ]
    }

    pub fn is_empty(&self) -> bool {
        self.size[0] < 1.0 || self.size[1] < 1.0
    }
}

/// Zoom and pan of one view: `zoom` screen pixels per image pixel, with the
/// image point `center` shown at the middle of the viewport.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ViewState {
    pub zoom: f64,
    pub center: [f64; 2],
}

impl ViewState {
    /// 100% with the middle of a `width` x `height` image centred.
    pub fn actual_size(width: usize, height: usize) -> ViewState {
        ViewState {
            zoom: 1.0,
            center: [width as f64 / 2.0, height as f64 / 2.0],
        }
    }

    /// The zoom at which the whole image just fits the viewport.
    pub fn fit_zoom(image: [usize; 2], viewport: &Viewport) -> f64 {
        let zx = viewport.size[0] / image[0] as f64;
        let zy = viewport.size[1] / image[1] as f64;
        zx.min(zy).clamp(1e-6, MAX_ZOOM)
    }

    /// The whole image, centred, as large as the viewport allows.
    pub fn fit(image: [usize; 2], viewport: &Viewport) -> ViewState {
        ViewState {
            zoom: ViewState::fit_zoom(image, viewport),
            center: [image[0] as f64 / 2.0, image[1] as f64 / 2.0],
        }
    }

    /// The exact (unsnapped) transform for a viewport.
    pub fn transform(&self, viewport: &Viewport) -> ViewTransform {
        let c = viewport.center();
        ViewTransform {
            zoom: self.zoom,
            origin: [
                c[0] - self.center[0] * self.zoom,
                c[1] - self.center[1] * self.zoom,
            ],
        }
    }

    /// The transform used for drawing: as [`transform`](Self::transform),
    /// with the image origin moved to the nearest whole screen pixel so
    /// that image pixels line up with device pixels at whole-number zooms.
    pub fn snapped_transform(&self, viewport: &Viewport) -> ViewTransform {
        let t = self.transform(viewport);
        ViewTransform {
            zoom: t.zoom,
            origin: [t.origin[0].round(), t.origin[1].round()],
        }
    }

    /// Changes the zoom to `new_zoom`, keeping the image point under the
    /// screen position `anchor` where it is.
    pub fn zoom_about(&mut self, new_zoom: f64, anchor: [f64; 2], viewport: &Viewport) {
        let fixed = self.transform(viewport).screen_to_image(anchor);
        let c = viewport.center();
        self.zoom = new_zoom;
        // Solve anchor = c + (fixed - center) * new_zoom for the centre.
        self.center = [
            fixed[0] - (anchor[0] - c[0]) / new_zoom,
            fixed[1] - (anchor[1] - c[1]) / new_zoom,
        ];
    }

    /// Moves the image by `delta` screen pixels.
    pub fn pan_by(&mut self, delta: [f64; 2]) {
        self.center[0] -= delta[0] / self.zoom;
        self.center[1] -= delta[1] / self.zoom;
    }

    /// Restricts the pan so the image cannot be lost: along each axis an
    /// image smaller than the viewport is centred, and a larger one always
    /// covers the viewport.
    pub fn clamp(&mut self, image: [usize; 2], viewport: &Viewport) {
        for axis in 0..2 {
            let size = image[axis] as f64;
            let half_view = viewport.size[axis] / (2.0 * self.zoom);
            self.center[axis] = if size * self.zoom <= viewport.size[axis] {
                size / 2.0
            } else {
                self.center[axis].clamp(half_view, size - half_view)
            };
        }
    }
}

/// Maps between screen pixels and image pixels:
/// `screen = origin + image * zoom`. Image pixel `(i, j)` covers the image
/// coordinates `[i, i + 1) x [j, j + 1)`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ViewTransform {
    pub zoom: f64,
    /// Screen position of the image's top-left corner.
    pub origin: [f64; 2],
}

impl ViewTransform {
    pub fn image_to_screen(&self, p: [f64; 2]) -> [f64; 2] {
        [
            self.origin[0] + p[0] * self.zoom,
            self.origin[1] + p[1] * self.zoom,
        ]
    }

    pub fn screen_to_image(&self, s: [f64; 2]) -> [f64; 2] {
        [
            (s[0] - self.origin[0]) / self.zoom,
            (s[1] - self.origin[1]) / self.zoom,
        ]
    }

    /// The image pixel under a screen position, if it is inside a
    /// `width` x `height` image.
    pub fn pixel_at(&self, s: [f64; 2], width: usize, height: usize) -> Option<(usize, usize)> {
        let p = self.screen_to_image(s);
        let (x, y) = (p[0].floor(), p[1].floor());
        (x >= 0.0 && y >= 0.0 && x < width as f64 && y < height as f64)
            .then_some((x as usize, y as usize))
    }

    /// The image pixel shown by the device pixel whose top-left corner is
    /// at `(px, py)`, when samples are taken every `step` image pixels.
    ///
    /// This is the definition the shader implements: the pixel centre is
    /// mapped to image coordinates, and the sample is the one at the
    /// largest multiple of `step` not beyond it (after [`TIE_BIAS`]).
    pub fn displayed_pixel(
        &self,
        px: f64,
        py: f64,
        step: usize,
        width: usize,
        height: usize,
    ) -> Option<(usize, usize)> {
        let scale = self.zoom * step as f64;
        let u = ((px + 0.5 - self.origin[0]) / scale + TIE_BIAS).floor();
        let v = ((py + 0.5 - self.origin[1]) / scale + TIE_BIAS).floor();
        if u < 0.0 || v < 0.0 {
            return None;
        }
        let (x, y) = (u as usize * step, v as usize * step);
        (x < width && y < height).then_some((x, y))
    }

    /// The image pixels at least partly inside the viewport, or `None` if
    /// the image is entirely outside it.
    pub fn visible_rect(&self, viewport: &Viewport, width: usize, height: usize) -> Option<Rect> {
        let a = self.screen_to_image(viewport.min);
        let b = self.screen_to_image([
            viewport.min[0] + viewport.size[0],
            viewport.min[1] + viewport.size[1],
        ]);
        let x0 = a[0].floor().max(0.0);
        let y0 = a[1].floor().max(0.0);
        let x1 = b[0].ceil().min(width as f64);
        let y1 = b[1].ceil().min(height as f64);
        (x1 > x0 && y1 > y0).then(|| {
            Rect::new(
                x0 as usize,
                y0 as usize,
                (x1 - x0) as usize,
                (y1 - y0) as usize,
            )
        })
    }
}

/// The decimation level for a zoom: samples are taken every `2^level`
/// image pixels, the largest power of two that still gives at least one
/// sample per screen pixel. Level 0 (every sample) from 100% up.
pub fn level_for_zoom(zoom: f64) -> u8 {
    if zoom >= 1.0 || zoom <= 0.0 || zoom.is_nan() {
        return 0;
    }
    // The small tolerance keeps exact powers of two (50%, 25%, ...) on the
    // level they name despite rounding in the logarithm.
    ((1.0 / zoom).log2() + 1e-9)
        .floor()
        .clamp(0.0, MAX_LEVEL as f64) as u8
}

/// Coarsest decimation level: one sample in 65536.
pub const MAX_LEVEL: u8 = 16;

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn close(a: [f64; 2], b: [f64; 2], tol: f64) -> bool {
        (a[0] - b[0]).abs() <= tol && (a[1] - b[1]).abs() <= tol
    }

    #[test]
    fn zoom_steps_walk_the_table() {
        assert_eq!(zoom_step_in(1.0), 1.5);
        assert_eq!(zoom_step_in(0.4), 0.5);
        assert_eq!(zoom_step_in(32.0), 32.0);
        assert_eq!(zoom_step_out(1.0, 0.0), 0.75);
        assert_eq!(zoom_step_out(1.2, 0.0), 1.0);
        assert_eq!(zoom_step_out(1.0 / 72.0, 0.0), 1.0 / 72.0);
        // A huge image may go below the smallest table entry, down to fit.
        assert_eq!(zoom_step_out(0.5, 0.45), 0.45);
        assert_eq!(zoom_step_out(0.45, 0.45), 0.45);
        // In then out returns to the same step.
        for &z in &ZOOM_LEVELS[1..ZOOM_LEVELS.len() - 1] {
            assert_eq!(zoom_step_out(zoom_step_in(z), 0.0), z);
        }
    }

    #[test]
    fn levels_follow_zoom() {
        assert_eq!(level_for_zoom(4.0), 0);
        assert_eq!(level_for_zoom(1.0), 0);
        assert_eq!(level_for_zoom(0.75), 0);
        assert_eq!(level_for_zoom(0.5), 1);
        assert_eq!(level_for_zoom(1.0 / 3.0), 1);
        assert_eq!(level_for_zoom(0.25), 2);
        assert_eq!(level_for_zoom(1.0 / 72.0), 6);
        assert_eq!(level_for_zoom(0.0), 0);
    }

    #[test]
    fn clamp_centres_small_images_and_keeps_large_ones_covering() {
        let vp = Viewport::new(10.0, 20.0, 400.0, 300.0);
        // Smaller than the viewport on both axes: centred whatever the pan.
        let mut s = ViewState {
            zoom: 1.0,
            center: [-500.0, 900.0],
        };
        s.clamp([100, 80], &vp);
        assert_eq!(s.center, [50.0, 40.0]);
        // Larger on both axes: the viewport stays inside the image.
        let mut s = ViewState {
            zoom: 2.0,
            center: [-500.0, 9000.0],
        };
        s.clamp([1000, 800], &vp);
        assert_eq!(s.center, [100.0, 800.0 - 75.0]);
        let t = s.transform(&vp);
        assert_eq!(t.screen_to_image(vp.min)[0], 0.0);
        assert_eq!(t.screen_to_image([410.0, 320.0])[1], 800.0);
        // Mixed: wide image, short viewport.
        let mut s = ViewState {
            zoom: 1.0,
            center: [0.0, 0.0],
        };
        s.clamp([1000, 100], &vp);
        assert_eq!(s.center, [200.0, 50.0]);
    }

    #[test]
    fn fit_shows_the_whole_image() {
        let vp = Viewport::new(0.0, 0.0, 800.0, 600.0);
        let s = ViewState::fit([1600, 600], &vp);
        assert_eq!(s.zoom, 0.5);
        let t = s.transform(&vp);
        assert_eq!(
            t.visible_rect(&vp, 1600, 600),
            Some(Rect::new(0, 0, 1600, 600))
        );
        assert_eq!(t.image_to_screen([0.0, 0.0]), [0.0, 150.0]);
    }

    #[test]
    fn visible_rect_is_clipped_to_the_image() {
        let vp = Viewport::new(0.0, 0.0, 100.0, 100.0);
        let t = ViewTransform {
            zoom: 2.0,
            origin: [-31.0, 40.0],
        };
        // x: image 15.5..65.5 visible -> pixels 15..66; y: starts at screen 40.
        assert_eq!(
            t.visible_rect(&vp, 1000, 20),
            Some(Rect::new(15, 0, 51, 20))
        );
        let off = ViewTransform {
            zoom: 1.0,
            origin: [500.0, 0.0],
        };
        assert_eq!(off.visible_rect(&vp, 50, 50), None);
    }

    #[test]
    fn displayed_pixel_snaps_to_the_step() {
        let t = ViewTransform {
            zoom: 0.25,
            origin: [0.0, 0.0],
        };
        // Screen pixel 3 has its centre at image 14; with step 4 it shows 12.
        assert_eq!(t.displayed_pixel(3.0, 0.0, 4, 100, 100), Some((12, 0)));
        assert_eq!(t.displayed_pixel(25.0, 0.0, 4, 100, 100), None);
        assert_eq!(t.displayed_pixel(-1.0, 0.0, 4, 100, 100), None);
        // A centre exactly on a boundary shows the pixel after it.
        let t = ViewTransform {
            zoom: 1.5,
            origin: [0.0, 0.0],
        };
        assert_eq!(t.displayed_pixel(1.0, 1.0, 1, 100, 100), Some((1, 1)));
    }

    proptest! {
        #[test]
        fn screen_image_round_trip(
            zoom in 0.01f64..32.0,
            ox in -1e5f64..1e5, oy in -1e5f64..1e5,
            x in -1e5f64..1e5, y in -1e5f64..1e5,
        ) {
            let t = ViewTransform { zoom, origin: [ox, oy] };
            let back = t.screen_to_image(t.image_to_screen([x, y]));
            prop_assert!(close(back, [x, y], 1e-6));
            let back = t.image_to_screen(t.screen_to_image([x, y]));
            prop_assert!(close(back, [x, y], 1e-6));
        }

        #[test]
        fn zoom_about_keeps_the_anchor_fixed(
            zoom in 0.01f64..32.0, new_zoom in 0.01f64..32.0,
            cx in -5000f64..5000.0, cy in -5000f64..5000.0,
            ax in 0f64..800.0, ay in 0f64..600.0,
        ) {
            let vp = Viewport::new(30.0, 40.0, 800.0, 600.0);
            let anchor = [vp.min[0] + ax, vp.min[1] + ay];
            let mut s = ViewState { zoom, center: [cx, cy] };
            let before = s.transform(&vp).screen_to_image(anchor);
            s.zoom_about(new_zoom, anchor, &vp);
            prop_assert_eq!(s.zoom, new_zoom);
            let t = s.transform(&vp);
            prop_assert!(close(t.screen_to_image(anchor), before, 1e-6));
            prop_assert!(close(t.image_to_screen(before), anchor, 1e-6));
        }

        #[test]
        fn clamp_never_loses_the_image(
            zoom in 0.01f64..32.0,
            cx in -1e6f64..1e6, cy in -1e6f64..1e6,
            w in 1usize..30000, h in 1usize..30000,
            vw in 1u32..3000, vh in 1u32..3000,
        ) {
            let vp = Viewport::new(0.0, 0.0, vw as f64, vh as f64);
            let mut s = ViewState { zoom, center: [cx, cy] };
            s.clamp([w, h], &vp);
            let t = s.transform(&vp);
            for (axis, size) in [(0, w), (1, h)] {
                let lo = t.origin[axis];
                let hi = lo + size as f64 * zoom;
                let (v0, v1) = (vp.min[axis], vp.min[axis] + vp.size[axis]);
                if size as f64 * zoom <= vp.size[axis] {
                    // Wholly visible and centred.
                    prop_assert!(lo >= v0 - 1e-6 && hi <= v1 + 1e-6);
                    prop_assert!(((lo - v0) - (v1 - hi)).abs() < 1e-6);
                } else {
                    // Covers the viewport: no background shows on this axis.
                    prop_assert!(lo <= v0 + 1e-6 && hi >= v1 - 1e-6);
                }
            }
            // Clamping twice changes nothing more.
            let once = s;
            s.clamp([w, h], &vp);
            prop_assert!(close(s.center, once.center, 1e-9));
        }

        #[test]
        fn pan_moves_the_image_by_the_drag(
            zoom in 0.01f64..32.0,
            dx in -500f64..500.0, dy in -500f64..500.0,
        ) {
            let vp = Viewport::new(0.0, 0.0, 640.0, 480.0);
            let mut s = ViewState { zoom, center: [100.0, 100.0] };
            let before = s.transform(&vp).image_to_screen([7.0, 9.0]);
            s.pan_by([dx, dy]);
            let after = s.transform(&vp).image_to_screen([7.0, 9.0]);
            prop_assert!(close([after[0] - before[0], after[1] - before[1]], [dx, dy], 1e-6));
        }

        #[test]
        fn displayed_pixel_agrees_with_pixel_at_at_full_resolution(
            zoom in 0.3f64..32.0,
            ox in -200i32..200, oy in -200i32..200,
            px in 0u32..400, py in 0u32..400,
        ) {
            let t = ViewTransform { zoom, origin: [ox as f64, oy as f64] };
            let centre = [px as f64 + 0.5, py as f64 + 0.5];
            let exact = t.pixel_at(centre, 300, 300);
            let shown = t.displayed_pixel(px as f64, py as f64, 1, 300, 300);
            // They differ only when the centre is within the tie bias of a
            // pixel boundary.
            if exact != shown {
                let p = t.screen_to_image(centre);
                let near = |v: f64| (v + TIE_BIAS).floor() != v.floor();
                prop_assert!(near(p[0]) || near(p[1]));
            }
        }
    }
}
