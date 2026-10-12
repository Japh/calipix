//! What is drawn over an image in its tab: for now, the outline of its
//! selection.
//!
//! This is the seam for everything that sits on top of the picture
//! (selections of every shape, their handles, overlays, scale bars). The
//! viewer widget draws the image and knows nothing of these; after it has
//! drawn, [`ImagePlace`] says where image coordinates fall on the screen,
//! and the functions here paint with that. Adding a shape means adding an
//! arm to [`selection`], not touching the viewer.

use calipix_roi::Roi;
use calipix_view::geometry::{ViewState, ViewTransform, Viewport};
use eframe::egui::{self, Color32, Rect, Stroke, StrokeKind, pos2};

/// The colour of a selection's outline: ImageJ's yellow, which few
/// scientific images contain.
pub const SELECTION: Color32 = Color32::from_rgb(255, 255, 0);
/// Drawn just outside the outline so that it also shows on a yellow or
/// white background.
const SELECTION_EDGE: Color32 = Color32::from_rgba_premultiplied(0, 0, 0, 150);

/// Where an image sits on the screen: the mapping the viewer drew it with.
#[derive(Clone, Copy, Debug)]
pub struct ImagePlace {
    transform: ViewTransform,
    pixels_per_point: f32,
    /// The part of the tab the image is drawn into.
    pub rect: Rect,
}

impl ImagePlace {
    /// The place of an image drawn into `rect` at `zoom` screen pixels per
    /// image pixel with the image point `center` in the middle, as the
    /// viewer widget reports them after drawing.
    pub fn new(rect: Rect, pixels_per_point: f32, zoom: f64, center: [f64; 2]) -> ImagePlace {
        let viewport = Viewport::from_points(rect, pixels_per_point);
        ImagePlace {
            // The same snapping the widget draws with, so that outlines
            // sit exactly on pixel edges.
            transform: ViewState { zoom, center }.snapped_transform(&viewport),
            pixels_per_point,
            rect,
        }
    }

    /// The device pixel an image coordinate falls on. Whole image
    /// coordinates are the edges between pixels.
    pub fn device(&self, point: [f64; 2]) -> [f64; 2] {
        self.transform.image_to_screen(point)
    }

    /// The length of one device pixel in the units the painter uses.
    pub fn device_pixel(&self) -> f32 {
        1.0 / self.pixels_per_point
    }

    /// The rectangle of device pixels that an image rectangle covers, in
    /// the painter's units, at least one device pixel each way.
    pub fn rect_of(&self, x: f64, y: f64, width: f64, height: f64) -> Rect {
        let [x0, y0] = self.device([x, y]).map(f64::round);
        let [x1, y1] = self.device([x + width, y + height]).map(f64::round);
        let (x1, y1) = (x1.max(x0 + 1.0), y1.max(y0 + 1.0));
        let scale = self.pixels_per_point as f64;
        Rect::from_min_max(
            pos2((x0 / scale) as f32, (y0 / scale) as f32),
            pos2((x1 / scale) as f32, (y1 / scale) as f32),
        )
    }
}

/// Draws the outline of a selection: a line one device pixel wide at any
/// zoom, along the inside edge of the selected pixels.
///
/// Only rectangles are drawn so far. Other shapes are still honoured by
/// the commands that act within a selection; they are just not shown.
pub fn selection(painter: &egui::Painter, place: &ImagePlace, roi: &Roi) {
    if !(roi.is_area() && roi.is_rectangular()) {
        return;
    }
    let (x, y, width, height) = roi.bounds();
    let outline = place.rect_of(x, y, width, height);
    let pixel = place.device_pixel();
    let painter = painter.with_clip_rect(place.rect);
    painter.rect_stroke(
        outline,
        0.0,
        Stroke::new(pixel, SELECTION_EDGE),
        StrokeKind::Outside,
    );
    painter.rect_stroke(
        outline,
        0.0,
        Stroke::new(pixel, SELECTION),
        StrokeKind::Inside,
    );
}

/// The bounds of a selection as the status bar words them.
pub fn bounds_text(roi: &Roi) -> String {
    let (x, y, width, height) = roi.bounds();
    let number = super::ui::number_text;
    format!(
        "Selection {} × {} at {}, {}",
        number(width),
        number(height),
        number(x),
        number(y)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_selection_outlines_whole_device_pixels_at_any_zoom() {
        let tab = Rect::from_min_size(pos2(10.0, 20.0), egui::vec2(400.0, 300.0));
        // A 100 x 100 image centred in the tab at 200%: its corner is 100
        // device pixels left of and above the middle of the tab.
        let place = ImagePlace::new(tab, 1.0, 2.0, [50.0, 50.0]);
        assert_eq!(place.device([0.0, 0.0]), [110.0, 70.0]);
        let outline = place.rect_of(10.0, 20.0, 30.0, 40.0);
        assert_eq!(outline.min, pos2(130.0, 110.0));
        assert_eq!(outline.size(), egui::vec2(60.0, 80.0));
        assert_eq!(place.device_pixel(), 1.0);

        // On a display with two device pixels per point the line is half
        // a point wide, and edges land on device pixels.
        let dense = ImagePlace::new(tab, 2.0, 3.0, [50.0, 50.0]);
        assert_eq!(dense.device_pixel(), 0.5);
        let outline = dense.rect_of(1.0, 1.0, 1.0, 1.0);
        assert_eq!(outline.size(), egui::vec2(1.5, 1.5));
        assert_eq!((outline.min.x * 2.0).fract(), 0.0);

        // Zoomed far out, a small selection is still a pixel across.
        let far = ImagePlace::new(tab, 1.0, 0.01, [50.0, 50.0]);
        assert_eq!(far.rect_of(10.0, 10.0, 5.0, 5.0).size(), egui::vec2(1.0, 1.0));
    }

    #[test]
    fn bounds_read_as_size_then_place() {
        let roi = Roi::rectangle(10.0, 20.0, 30.0, 40.5);
        assert_eq!(bounds_text(&roi), "Selection 30 × 40.5 at 10, 20");
    }
}
