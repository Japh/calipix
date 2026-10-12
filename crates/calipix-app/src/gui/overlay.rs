//! What is drawn over an image in its tab: its selection, the shape a
//! tool is making, and its saved regions.
//!
//! This is the seam for everything that sits on top of the picture. The
//! viewer widget draws the image and knows nothing of these; after it has
//! drawn, [`ImagePlace`] says where image coordinates fall on the screen,
//! and the functions here paint with that.
//!
//! # How shapes are drawn
//!
//! - Outlines are one device pixel wide at any zoom, on whole device
//!   pixels, over a darker line three pixels wide: the pair shows on
//!   bright and on dark images alike.
//! - The selection is ImageJ's yellow, with handles (small white squares)
//!   where it can be reshaped. Regions are drawn in their own colour, or
//!   cyan, without handles; those picked in the list are drawn heavier.
//! - Lines and points name pixel centres, so they are drawn half a pixel
//!   down and right of their coordinates; a line wider than a pixel shows
//!   its width as a translucent band.
//! - Regions outside the visible part of the image are skipped, and one
//!   too small to make out is drawn as a dot, so thousands cost little.

use std::collections::BTreeSet;
use std::time::Instant;

use calipix_roi::{Roi, RoiEntry};
use calipix_view::geometry::{ViewState, ViewTransform, Viewport};
use eframe::egui::{self, Color32, Mesh, Pos2, Rect, Shape, Stroke, StrokeKind, pos2, vec2};

use crate::state::tools::{Draft, drawn_offset, handles};
use crate::state::{CanvasView, RegionLabels};

/// The colour of a selection's outline: ImageJ's yellow, which few
/// scientific images contain.
pub const SELECTION: Color32 = Color32::from_rgb(255, 255, 0);
/// The colour of a region that has none of its own: the cyan of
/// ImageJ's ROI Manager.
pub const REGION: Color32 = Color32::from_rgb(0, 255, 255);
/// Drawn under every outline so that it also shows on a background of
/// its own colour.
const EDGE: Color32 = Color32::from_rgba_premultiplied(0, 0, 0, 150);
/// Side of a handle, in points.
const HANDLE: f32 = 7.0;
/// A region smaller than this many device pixels is drawn as a dot.
const DOT_BELOW: f32 = 4.0;
/// A region smaller than this many points gets no label.
const LABEL_FROM: f32 = 14.0;
/// With more regions than this in view, each is drawn more plainly.
const CROWDED_FROM: usize = 600;

/// Where an image sits on the screen: the mapping the viewer drew it with.
#[derive(Clone, Copy, Debug, PartialEq)]
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

    /// Magnification: device pixels per image pixel.
    pub fn zoom(&self) -> f64 {
        self.transform.zoom
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

    /// Where an image coordinate is on the screen, in the painter's units.
    pub fn screen(&self, point: [f64; 2]) -> Pos2 {
        let [x, y] = self.device(point);
        let scale = self.pixels_per_point as f64;
        pos2((x / scale) as f32, (y / scale) as f32)
    }

    /// As [`screen`](Self::screen), moved to the middle of the device
    /// pixel it falls on, where a line one pixel wide is sharp.
    pub fn crisp(&self, point: [f64; 2]) -> Pos2 {
        let [x, y] = self.device(point);
        let scale = self.pixels_per_point as f64;
        let middle = |v: f64| (((v + 1e-6).floor() + 0.5) / scale) as f32;
        pos2(middle(x), middle(y))
    }

    /// The image coordinate under a position on the screen.
    pub fn image(&self, at: Pos2) -> [f64; 2] {
        let scale = self.pixels_per_point as f64;
        self.transform
            .screen_to_image([at.x as f64 * scale, at.y as f64 * scale])
    }

    /// A distance on the screen, in points, as image pixels.
    pub fn reach(&self, points: f32) -> f64 {
        (points * self.pixels_per_point) as f64 / self.transform.zoom
    }

    /// The part of the image plane that is on screen, as `[left, top,
    /// right, bottom]` in image coordinates.
    pub fn visible(&self) -> [f64; 4] {
        let [x0, y0] = self.image(self.rect.min);
        let [x1, y1] = self.image(self.rect.max);
        [x0, y0, x1, y1]
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

/// One closed or open run of an outline, in image coordinates as drawn.
struct Run {
    points: Vec<[f64; 2]>,
    closed: bool,
}

/// The outline of a shape as runs of straight segments. `zoom` decides
/// how finely an oval is divided. Point selections have no outline.
fn runs(roi: &Roi, zoom: f64) -> Vec<Run> {
    let closed = |points: Vec<(f64, f64)>| Run {
        points: points.into_iter().map(|(x, y)| [x, y]).collect(),
        closed: true,
    };
    match roi {
        Roi::Rectangle {
            x,
            y,
            width,
            height,
        } => vec![Run {
            points: vec![
                [*x, *y],
                [x + width, *y],
                [x + width, y + height],
                [*x, y + height],
            ],
            closed: true,
        }],
        Roi::Oval {
            x,
            y,
            width,
            height,
        } => {
            let (rx, ry) = (width / 2.0, height / 2.0);
            let (cx, cy) = (x + rx, y + ry);
            // A vertex about every four device pixels of the outline.
            let around = std::f64::consts::PI * (rx + ry) * zoom;
            let steps = ((around / 4.0) as usize).clamp(12, 256);
            let points = (0..steps)
                .map(|step| {
                    let angle = std::f64::consts::TAU * step as f64 / steps as f64;
                    [cx + rx * angle.cos(), cy + ry * angle.sin()]
                })
                .collect();
            vec![Run {
                points,
                closed: true,
            }]
        }
        Roi::Points { .. } => Vec::new(),
        Roi::Composite { path } => path.polygons().into_iter().map(closed).collect(),
        Roi::Polygon { points } | Roi::Freehand { points } | Roi::Traced { points } => {
            vec![closed(points.clone())]
        }
        _ if roi.is_line() => {
            let offset = drawn_offset(roi);
            vec![Run {
                points: roi
                    .vertices()
                    .unwrap_or_default()
                    .into_iter()
                    .map(|(x, y)| [x + offset, y + offset])
                    .collect(),
                closed: false,
            }]
        }
        // Rounded and rotated rectangles, ellipses: as ImageJ flattens them.
        _ => vec![closed(roi.outline())],
    }
}

/// How an outline is stroked.
#[derive(Clone, Copy)]
struct Pen {
    color: Color32,
    /// Width in device pixels.
    width: f32,
    /// Leave out the dark line underneath, and draw points as dots.
    plain: bool,
}

/// Adds the strokes of one run: the dark line under it, then the line.
fn stroke_run(shapes: &mut Vec<Shape>, place: &ImagePlace, run: &Run, pen: Pen) {
    let mut points: Vec<Pos2> = Vec::with_capacity(run.points.len());
    for point in &run.points {
        let at = place.crisp(*point);
        if points.last() != Some(&at) {
            points.push(at);
        }
    }
    if run.closed && points.len() > 1 && points.first() == points.last() {
        points.pop();
    }
    let pixel = place.device_pixel();
    match points.len() {
        0 => {}
        1 => {
            // Too small to be more than a dot.
            let dot = Rect::from_center_size(points[0], vec2(pixel, pixel) * 3.0);
            shapes.push(Shape::rect_filled(dot, 0.0, pen.color));
        }
        _ => {
            let under = Stroke::new((pen.width + 2.0) * pixel, EDGE);
            let over = Stroke::new(pen.width * pixel, pen.color);
            let closed = run.closed && points.len() > 2;
            let line = |points: Vec<Pos2>, stroke: Stroke| {
                if closed {
                    Shape::closed_line(points, stroke)
                } else {
                    Shape::line(points, stroke)
                }
            };
            if !pen.plain {
                shapes.push(line(points.clone(), under));
            }
            shapes.push(line(points, over));
        }
    }
}

/// The translucent band that shows how wide a line selection is.
fn width_band(shapes: &mut Vec<Shape>, place: &ImagePlace, roi: &Roi, color: Color32) {
    let width = roi.line_width();
    if !roi.is_line() || width <= 1.0 {
        return;
    }
    let band = (width * place.zoom()) as f32 * place.device_pixel();
    if band < 3.0 * place.device_pixel() {
        return;
    }
    for run in runs(roi, place.zoom()) {
        let points: Vec<Pos2> = run.points.iter().map(|p| place.screen(*p)).collect();
        shapes.push(Shape::line(
            points,
            Stroke::new(band, color.gamma_multiply(0.3)),
        ));
    }
}

/// Fills what the runs enclose an odd number of times, so that holes
/// stay open: one strip per device pixel row within `clip`.
fn fill_runs(place: &ImagePlace, all: &[Run], clip: Rect, color: Color32) -> Option<Mesh> {
    let scale = place.pixels_per_point;
    let polygons: Vec<Vec<[f64; 2]>> = all
        .iter()
        .filter(|run| run.closed && run.points.len() > 2)
        .map(|run| run.points.iter().map(|p| place.device(*p)).collect())
        .collect();
    let rows = polygons.iter().flatten().map(|p| p[1]);
    let top = rows.clone().fold(f64::INFINITY, f64::min);
    let bottom = rows.fold(f64::NEG_INFINITY, f64::max);
    if !(top.is_finite() && bottom.is_finite()) {
        return None;
    }
    let first = top.floor().max((clip.top() * scale).floor() as f64) as i64;
    let last = bottom.ceil().min((clip.bottom() * scale).ceil() as f64) as i64;
    let (left, right) = ((clip.left() * scale) as f64, (clip.right() * scale) as f64);
    let mut mesh = Mesh::default();
    let mut crossings: Vec<f64> = Vec::new();
    for row in first..last {
        let y = row as f64 + 0.5;
        crossings.clear();
        for polygon in &polygons {
            for index in 0..polygon.len() {
                let (a, b) = (polygon[index], polygon[(index + 1) % polygon.len()]);
                if (a[1] <= y) != (b[1] <= y) {
                    crossings.push(a[0] + (y - a[1]) / (b[1] - a[1]) * (b[0] - a[0]));
                }
            }
        }
        crossings.sort_by(f64::total_cmp);
        for pair in crossings.chunks_exact(2) {
            let (x0, x1) = (pair[0].max(left), pair[1].min(right));
            if x1 > x0 {
                let strip = Rect::from_min_max(
                    pos2(x0 as f32 / scale, row as f32 / scale),
                    pos2(x1 as f32 / scale, (row + 1) as f32 / scale),
                );
                mesh.add_colored_rect(strip, color);
            }
        }
    }
    (!mesh.is_empty()).then_some(mesh)
}

/// A handle: a small white square edged in black, or yellow while it is
/// being dragged or the pointer is on it.
fn handle(shapes: &mut Vec<Shape>, place: &ImagePlace, at: [f64; 2], lit: bool) {
    let pixel = place.device_pixel();
    // An odd number of device pixels, so that it has a middle.
    let side = ((HANDLE / pixel / 2.0).floor() * 2.0 + 1.0) * pixel;
    let square = Rect::from_center_size(place.crisp(at), vec2(side, side));
    let fill = if lit { SELECTION } else { Color32::WHITE };
    shapes.push(Shape::rect_filled(square, 0.0, fill));
    shapes.push(Shape::rect_stroke(
        square,
        0.0,
        Stroke::new(pixel, Color32::BLACK),
        StrokeKind::Inside,
    ));
}

/// A point of a point selection, after ImageJ's default marker: a white
/// cross edged in black with a dot of the selection's colour in its
/// middle.
fn point_marker(shapes: &mut Vec<Shape>, place: &ImagePlace, at: [f64; 2], color: Color32) {
    let pixel = place.device_pixel();
    let centre = place.crisp(at);
    let arm = 7.0 * pixel;
    let (across, down) = (vec2(arm, 0.0), vec2(0.0, arm));
    for (stroke, more) in [
        (Stroke::new(3.0 * pixel, Color32::BLACK), pixel),
        (Stroke::new(pixel, Color32::WHITE), 0.0),
    ] {
        let (longer_x, longer_y) = (vec2(more, 0.0), vec2(0.0, more));
        shapes.push(Shape::line_segment(
            [centre - across - longer_x, centre + across + longer_x],
            stroke,
        ));
        shapes.push(Shape::line_segment(
            [centre - down - longer_y, centre + down + longer_y],
            stroke,
        ));
    }
    let dot = Rect::from_center_size(centre, vec2(5.0, 5.0) * pixel);
    shapes.push(Shape::rect_filled(dot, 0.0, Color32::BLACK));
    shapes.push(Shape::rect_filled(dot.shrink(pixel), 0.0, color));
}

/// A label on a dark plate, so that it reads on any image. `anchor` says
/// which point of the label sits at `at`.
fn label(
    painter: &egui::Painter,
    at: Pos2,
    anchor: egui::Align2,
    text: &str,
    font: &egui::FontId,
    strong: bool,
) {
    let galley = painter.layout_no_wrap(text.to_string(), font.clone(), Color32::WHITE);
    let plate = anchor
        .anchor_size(at, galley.size() + vec2(6.0, 2.0))
        .translate(vec2(0.0, 0.0));
    let plate = Rect::from_min_size(plate.min.round(), plate.size());
    let fill = if strong {
        Color32::from_rgba_unmultiplied(0, 0, 0, 225)
    } else {
        Color32::from_rgba_unmultiplied(0, 0, 0, 165)
    };
    painter.rect_filled(plate, 3.0, fill);
    painter.galley(plate.min + vec2(3.0, 1.0), galley, Color32::WHITE);
}

/// Draws one shape: its outline, and for points their markers.
fn shape(shapes: &mut Vec<Shape>, place: &ImagePlace, roi: &Roi, pen: Pen) {
    width_band(shapes, place, roi, pen.color);
    if let Roi::Rectangle {
        x,
        y,
        width,
        height,
    } = roi
        && pen.width <= 1.0
    {
        // Along the inside edge of the selected pixels, exactly.
        let outline = place.rect_of(*x, *y, *width, *height);
        let pixel = place.device_pixel();
        if !pen.plain {
            shapes.push(Shape::rect_stroke(
                outline,
                0.0,
                Stroke::new(pixel, EDGE),
                StrokeKind::Outside,
            ));
        }
        shapes.push(Shape::rect_stroke(
            outline,
            0.0,
            Stroke::new(pixel, pen.color),
            StrokeKind::Inside,
        ));
        return;
    }
    if let Roi::Points { points } = roi {
        for (x, y) in points {
            let at = [x + 0.5, y + 0.5];
            if pen.plain {
                let pixel = place.device_pixel();
                let dot = Rect::from_center_size(place.crisp(at), vec2(3.0, 3.0) * pixel);
                shapes.push(Shape::rect_filled(dot, 0.0, pen.color));
            } else {
                point_marker(shapes, place, at, pen.color);
            }
        }
        return;
    }
    for run in runs(roi, place.zoom()) {
        stroke_run(shapes, place, &run, pen);
    }
}

/// Draws the selection of an image as `view` describes it, with its
/// handles and whatever a tool is in the middle of drawing. `hovered` is
/// the handle under the pointer.
pub fn selection(
    painter: &egui::Painter,
    place: &ImagePlace,
    view: &CanvasView,
    hovered: Option<usize>,
) {
    let painter = painter.with_clip_rect(place.rect);
    let pen = Pen {
        color: SELECTION,
        width: 1.0,
        plain: false,
    };
    let mut shapes = Vec::new();
    if let Some(roi) = &view.selection {
        shape(&mut shapes, place, roi, pen);
    }
    if let Some(extra) = &view.extra {
        shape(&mut shapes, place, extra, pen);
    }
    if let Some(draft) = &view.draft {
        draw_draft(&mut shapes, place, draft, pen);
    }
    // Handles go on top of every line. A point's marker is its handle.
    if let Some(roi) = &view.selection
        && !roi.is_points()
    {
        for (index, at) in handles(roi).into_iter().enumerate() {
            let lit = view.handle == Some(index) || hovered == Some(index);
            handle(&mut shapes, place, at, lit);
        }
    }
    painter.extend(shapes);
    // The points of a multi-point selection are numbered, as in ImageJ.
    if let Some(Roi::Points { points }) = &view.selection
        && points.len() > 1
    {
        let font = egui::FontId::proportional(11.0);
        for (index, (x, y)) in points.iter().enumerate() {
            let at = place.crisp([x + 0.5, y + 0.5]) + vec2(6.0, 5.0);
            let number = (index + 1).to_string();
            label(&painter, at, egui::Align2::LEFT_TOP, &number, &font, false);
        }
    }
}

/// An unfinished outline: the segments laid down so far, the rubber band
/// to the pointer, and a square on each vertex placed by a click.
fn draw_draft(shapes: &mut Vec<Shape>, place: &ImagePlace, draft: &Draft, pen: Pen) {
    let offset = if draft.line { 0.5 } else { 0.0 };
    let as_drawn = |p: &(f64, f64)| [p.0 + offset, p.1 + offset];
    let mut points: Vec<[f64; 2]> = draft.points.iter().map(as_drawn).collect();
    let placed = points.len();
    points.extend(draft.hover.as_ref().map(as_drawn));
    stroke_run(
        shapes,
        place,
        &Run {
            points: points.clone(),
            closed: false,
        },
        pen,
    );
    // Where the outline would close if it were finished now.
    if draft.closed
        && points.len() > 2
        && let (Some(first), Some(last)) = (points.first(), points.last())
    {
        let closing = Stroke::new(place.device_pixel(), pen.color.gamma_multiply(0.45));
        shapes.push(Shape::line_segment(
            [place.crisp(*last), place.crisp(*first)],
            closing,
        ));
    }
    // Freehand outlines have a vertex per pixel: too many to mark.
    if draft.hover.is_some() || placed <= 2 {
        for (index, at) in points.iter().take(placed).enumerate() {
            let closes = draft.finishing && (index == 0 || index + 1 == placed);
            handle(shapes, place, *at, closes);
        }
    }
}

/// What drawing the regions of an image took, for measuring.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct RegionStats {
    /// Regions the image has.
    pub regions: usize,
    /// Regions on screen that were drawn as outlines.
    pub outlined: usize,
    /// Regions on screen too small to be more than a dot.
    pub dots: usize,
    /// Labels drawn; others gave way to them.
    pub labels: usize,
    /// Time spent working out what to draw.
    pub micros: u64,
}

/// Which parts of the image's tab have a label on them already.
struct Taken {
    area: Rect,
    columns: usize,
    rows: usize,
    cells: Vec<bool>,
}

impl Taken {
    /// Side of a cell in points: finer than a label, so that labels may
    /// stand close but not on each other.
    const CELL: f32 = 6.0;

    fn new(area: Rect) -> Taken {
        let columns = (area.width() / Taken::CELL).ceil().max(1.0) as usize;
        let rows = (area.height() / Taken::CELL).ceil().max(1.0) as usize;
        Taken {
            area,
            columns,
            rows,
            cells: vec![false; columns * rows],
        }
    }

    /// Takes the cells under `rect` if none of them is taken. Returns
    /// false, taking nothing, if the label would lie on another or
    /// outside the tab.
    fn claim(&mut self, rect: Rect) -> bool {
        if !self.area.intersects(rect) {
            return false;
        }
        let cell = |v: f32, origin: f32, count: usize| {
            (((v - origin) / Taken::CELL).floor().max(0.0) as usize).min(count - 1)
        };
        let (c0, c1) = (
            cell(rect.left(), self.area.left(), self.columns),
            cell(rect.right(), self.area.left(), self.columns),
        );
        let (r0, r1) = (
            cell(rect.top(), self.area.top(), self.rows),
            cell(rect.bottom(), self.area.top(), self.rows),
        );
        let cells = || (r0..=r1).flat_map(|row| (c0..=c1).map(move |column| (row, column)));
        if cells().any(|(row, column)| self.cells[row * self.columns + column]) {
            return false;
        }
        for (row, column) in cells() {
            self.cells[row * self.columns + column] = true;
        }
        true
    }
}

/// The bounds of each region in image coordinates as drawn, worked out
/// once per change to the list: `[left, top, right, bottom]`.
#[derive(Debug, Default)]
pub struct RegionBounds {
    /// What they were worked out for.
    key: Option<u64>,
    bounds: Vec<[f32; 4]>,
}

impl RegionBounds {
    /// The bounds of `entries`, worked out again only if `version` has
    /// changed since the last call.
    pub fn of(&mut self, entries: &[RoiEntry], version: u64) -> &[[f32; 4]] {
        if self.key != Some(version) || self.bounds.len() != entries.len() {
            self.key = Some(version);
            self.bounds = entries.iter().map(|entry| bounds(&entry.roi)).collect();
        }
        &self.bounds
    }
}

fn bounds(roi: &Roi) -> [f32; 4] {
    let (x, y, width, height) = roi.bounds();
    // Lines and points are drawn half a pixel further on, and a wide line
    // reaches half its width to either side.
    let offset = drawn_offset(roi);
    let margin = roi.line_width() / 2.0;
    [
        (x + offset - margin) as f32,
        (y + offset - margin) as f32,
        (x + width + offset + margin) as f32,
        (y + height + offset + margin) as f32,
    ]
}

/// The colour a region is outlined in.
pub fn region_color(entry: &RoiEntry) -> Color32 {
    match entry.props.stroke_color {
        Some(color) => {
            let [r, g, b, a] = color.rgba();
            Color32::from_rgba_unmultiplied(r, g, b, a)
        }
        None => REGION,
    }
}

/// What to draw of an image's regions.
pub struct RegionLook<'a> {
    pub entries: &'a [RoiEntry],
    /// From [`RegionBounds::of`].
    pub bounds: &'a [[f32; 4]],
    /// The regions picked in the list.
    pub picked: &'a BTreeSet<usize>,
    pub labels: RegionLabels,
}

/// Draws the regions of an image that `on_plane` says belong on the
/// plane on view.
pub fn regions(
    painter: &egui::Painter,
    place: &ImagePlace,
    look: &RegionLook<'_>,
    on_plane: impl Fn(&RoiEntry) -> bool,
) -> RegionStats {
    let started = Instant::now();
    let painter = painter.with_clip_rect(place.rect);
    let mut stats = RegionStats {
        regions: look.entries.len(),
        ..RegionStats::default()
    };
    let [left, top, right, bottom] = place.visible().map(|v| v as f32);
    let pixel = place.device_pixel();
    let zoom = place.zoom() as f32;
    let mut shapes = Vec::new();
    let mut labels: Vec<(Pos2, usize, bool)> = Vec::new();
    // With very many regions in view each is drawn more plainly: without
    // the dark line under its outline, and a point as a dot. Halving the
    // work keeps a crowded overview quick to move around in.
    let in_view = look
        .bounds
        .iter()
        .filter(|[x0, y0, x1, y1]| !(*x1 < left || *y1 < top || *x0 > right || *y0 > bottom))
        .count();
    let crowded = in_view > CROWDED_FROM;
    // Picked regions are drawn last, over their neighbours.
    for pass in [false, true] {
        for (index, entry) in look.entries.iter().enumerate() {
            let picked = look.picked.contains(&index);
            if picked != pass {
                continue;
            }
            let Some(&[x0, y0, x1, y1]) = look.bounds.get(index) else {
                continue;
            };
            if x1 < left || y1 < top || x0 > right || y0 > bottom || !on_plane(entry) {
                continue;
            }
            let color = region_color(entry);
            let (across, down) = ((x1 - x0) * zoom, (y1 - y0) * zoom);
            if across < DOT_BELOW && down < DOT_BELOW && !entry.roi.is_points() {
                let middle = [f64::from(x0 + x1) / 2.0, f64::from(y0 + y1) / 2.0];
                let side = if picked { 5.0 } else { 3.0 };
                let dot = Rect::from_center_size(place.crisp(middle), vec2(side, side) * pixel);
                shapes.push(Shape::rect_filled(dot, 0.0, color));
                stats.dots += 1;
                continue;
            }
            if let Some(fill) = entry.props.fill_color {
                let [r, g, b, a] = fill.rgba();
                let fill = Color32::from_rgba_unmultiplied(r, g, b, a);
                let all = runs(&entry.roi, place.zoom());
                if let Some(mesh) = fill_runs(place, &all, place.rect, fill) {
                    shapes.push(Shape::mesh(mesh));
                }
            }
            let pen = Pen {
                color,
                width: if picked { 3.0 } else { 1.0 },
                plain: crowded && !picked,
            };
            shape(&mut shapes, place, &entry.roi, pen);
            stats.outlined += 1;
            let large_enough = across.max(down) * pixel >= LABEL_FROM;
            if look.labels != RegionLabels::None && (large_enough || picked) {
                let middle = [f64::from(x0 + x1) / 2.0, f64::from(y0 + y1) / 2.0];
                labels.push((place.screen(middle), index, picked));
            }
        }
    }
    painter.extend(shapes);
    let font = egui::FontId::proportional(11.5);
    // A label gives way to one already placed where it would go, so that
    // crowded regions do not bury the image under their labels. Those of
    // picked regions are placed first.
    let mut taken = Taken::new(place.rect);
    labels.sort_by_key(|(_, _, picked)| !picked);
    for (at, index, picked) in labels {
        let entry = &look.entries[index];
        let text = match (look.labels, &entry.props.name) {
            (RegionLabels::Names, Some(name)) if !name.trim().is_empty() => name.clone(),
            _ => (index + 1).to_string(),
        };
        // Near enough the size the text will have, without laying it out.
        let size = vec2(text.chars().count() as f32 * 6.5 + 10.0, 18.0);
        if taken.claim(Rect::from_center_size(at, size)) {
            let middle = egui::Align2::CENTER_CENTER;
            label(&painter, at, middle, &text, &font, picked);
            stats.labels += 1;
        }
    }
    stats.micros = started.elapsed().as_micros() as u64;
    stats
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tab() -> Rect {
        Rect::from_min_size(pos2(10.0, 20.0), egui::vec2(400.0, 300.0))
    }

    #[test]
    fn a_selection_outlines_whole_device_pixels_at_any_zoom() {
        // A 100 x 100 image centred in the tab at 200%: its corner is 100
        // device pixels left of and above the middle of the tab.
        let place = ImagePlace::new(tab(), 1.0, 2.0, [50.0, 50.0]);
        assert_eq!(place.device([0.0, 0.0]), [110.0, 70.0]);
        let outline = place.rect_of(10.0, 20.0, 30.0, 40.0);
        assert_eq!(outline.min, pos2(130.0, 110.0));
        assert_eq!(outline.size(), egui::vec2(60.0, 80.0));
        assert_eq!(place.device_pixel(), 1.0);

        // On a display with two device pixels per point the line is half
        // a point wide, and edges land on device pixels.
        let dense = ImagePlace::new(tab(), 2.0, 3.0, [50.0, 50.0]);
        assert_eq!(dense.device_pixel(), 0.5);
        let outline = dense.rect_of(1.0, 1.0, 1.0, 1.0);
        assert_eq!(outline.size(), egui::vec2(1.5, 1.5));
        assert_eq!((outline.min.x * 2.0).fract(), 0.0);

        // Zoomed far out, a small selection is still a pixel across.
        let far = ImagePlace::new(tab(), 1.0, 0.01, [50.0, 50.0]);
        assert_eq!(
            far.rect_of(10.0, 10.0, 5.0, 5.0).size(),
            egui::vec2(1.0, 1.0)
        );
    }

    #[test]
    fn positions_map_both_ways_and_lines_sit_on_pixel_middles() {
        let place = ImagePlace::new(tab(), 1.0, 2.0, [50.0, 50.0]);
        assert_eq!(place.screen([10.0, 20.0]), pos2(130.0, 110.0));
        assert_eq!(place.image(pos2(130.0, 110.0)), [10.0, 20.0]);
        assert_eq!(place.image(pos2(131.0, 113.0)), [10.5, 21.5]);
        // A vertex on a pixel edge is drawn in the device pixel after it.
        assert_eq!(place.crisp([10.0, 20.0]), pos2(130.5, 110.5));
        assert_eq!(place.crisp([10.4, 20.4]), pos2(130.5, 110.5));
        // Six points reach three image pixels at 200%.
        assert_eq!(place.reach(6.0), 3.0);
        assert_eq!(place.visible(), [-50.0, -25.0, 150.0, 125.0]);
        let dense = ImagePlace::new(tab(), 2.0, 4.0, [50.0, 50.0]);
        assert_eq!(dense.reach(6.0), 3.0);
        assert_eq!((dense.crisp([10.0, 20.0]).x * 2.0).fract(), 0.5);
    }

    #[test]
    fn every_shape_has_an_outline_where_it_is_drawn() {
        let run_of = |roi: &Roi| runs(roi, 1.0);
        let rect = run_of(&Roi::rectangle(1.0, 2.0, 3.0, 4.0));
        assert_eq!(
            (rect.len(), rect[0].points.len(), rect[0].closed),
            (1, 4, true)
        );
        // A line runs through pixel centres.
        let line = run_of(&Roi::line(1.0, 2.0, 5.0, 2.0));
        assert_eq!(line[0].points, vec![[1.5, 2.5], [5.5, 2.5]]);
        assert!(!line[0].closed);
        // An oval gets more vertices the larger it is on screen.
        let oval = Roi::oval(0.0, 0.0, 40.0, 20.0);
        let coarse = runs(&oval, 0.5)[0].points.len();
        let fine = runs(&oval, 8.0)[0].points.len();
        assert!(
            coarse >= 12 && fine > coarse && fine <= 256,
            "{coarse} {fine}"
        );
        let extent = runs(&oval, 8.0)[0]
            .points
            .iter()
            .fold([f64::MAX, f64::MIN], |[low, high], p| {
                [low.min(p[0]), high.max(p[0])]
            });
        assert_eq!(extent, [0.0, 40.0]);
        // A square with a square hole: two runs.
        let holed: Roi = "composite M 0,0 L 10,0 L 10,10 L 0,10 Z M 3,3 L 7,3 L 7,7 L 3,7 Z"
            .parse()
            .unwrap();
        assert_eq!(run_of(&holed).len(), 2);
        assert!(run_of(&Roi::point(1.0, 1.0)).is_empty());
        for text in [
            "rounded-rectangle 2 2 20 10 6",
            "ellipse 5 5 30 20 0.5",
            "rotated-rectangle 5 5 30 20 8",
            "polygon 1,1 9,1 5,8",
            "freehand 1,1 9,1 5,8",
            "polyline 1,1 9,1 5,8",
            "angle 1,1 9,1 5,8",
        ] {
            let roi: Roi = text.parse().unwrap();
            assert!(run_of(&roi)[0].points.len() >= 3, "{text}");
        }
    }

    #[test]
    fn a_hole_is_left_unfilled() {
        let place = ImagePlace::new(tab(), 1.0, 2.0, [50.0, 50.0]);
        let holed: Roi = "composite M 0,0 L 10,0 L 10,10 L 0,10 Z M 3,3 L 7,3 L 7,7 L 3,7 Z"
            .parse()
            .unwrap();
        let mesh = fill_runs(&place, &runs(&holed, 2.0), place.rect, Color32::RED).unwrap();
        // Twenty device rows; the eight through the hole are two strips.
        assert_eq!(mesh.vertices.len() / 4, 12 + 8 * 2);
        let covers = |x: f32, y: f32| {
            mesh.vertices
                .chunks_exact(4)
                .any(|quad| Rect::from_min_max(quad[0].pos, quad[3].pos).contains(pos2(x, y)))
        };
        let inside = place.screen([1.5, 5.0]);
        let hole = place.screen([5.0, 5.0]);
        assert!(covers(inside.x, inside.y + 0.25));
        assert!(!covers(hole.x, hole.y + 0.25));
    }

    #[test]
    fn bounds_are_kept_until_the_regions_change() {
        let entries = vec![
            RoiEntry::new(Roi::rectangle(10.0, 20.0, 30.0, 40.0)),
            RoiEntry::new(Roi::line(1.0, 2.0, 5.0, 2.0).with_line_width(4.0)),
        ];
        let mut kept = RegionBounds::default();
        assert_eq!(kept.of(&entries, 1)[0], [10.0, 20.0, 40.0, 60.0]);
        // A line four pixels wide, through pixel centres.
        assert_eq!(kept.of(&entries, 1)[1], [-0.5, 0.5, 7.5, 4.5]);
        let moved = vec![RoiEntry::new(Roi::rectangle(0.0, 0.0, 1.0, 1.0))];
        assert_eq!(kept.of(&moved, 2), &[[0.0, 0.0, 1.0, 1.0]]);
    }

    #[test]
    fn a_label_gives_way_to_one_already_there() {
        let mut taken = Taken::new(tab());
        let at = |x: f32, y: f32| Rect::from_center_size(pos2(x, y), vec2(30.0, 18.0));
        assert!(taken.claim(at(100.0, 100.0)));
        assert!(!taken.claim(at(110.0, 105.0)));
        assert!(taken.claim(at(100.0, 130.0)));
        assert!(taken.claim(at(140.0, 100.0)));
        // Outside the tab there is nothing to label.
        assert!(!taken.claim(at(900.0, 900.0)));
    }

    #[test]
    fn regions_keep_their_colour_or_take_the_default() {
        let mut entry = RoiEntry::new(Roi::rectangle(0.0, 0.0, 1.0, 1.0));
        assert_eq!(region_color(&entry), REGION);
        entry.props.stroke_color = Some(calipix_roi::Color::rgb(255, 0, 128));
        assert_eq!(region_color(&entry), Color32::from_rgb(255, 0, 128));
    }
}
