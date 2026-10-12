//! The selection tools: what pressing, dragging and releasing the pointer
//! on an image does to its selection.
//!
//! Everything here is plain geometry with no toolkit in it, so each
//! gesture is tested as a sequence of pointer positions. An
//! [`Interaction`] lives from a press until the gesture is over. While it
//! lasts it only says what to draw ([`Interaction::shown`]); when it ends
//! it yields at most one [`Commit`], which the application runs as a
//! command. A gesture that is cancelled, or ends where it began, yields
//! nothing, and so leaves no trace in History.
//!
//! # Coordinates
//!
//! Pointer positions are in image coordinates as the image is drawn:
//! pixel `(x, y)` covers the square from `(x, y)` to `(x + 1, y + 1)`.
//! Areas use those coordinates directly. Lines and points name pixel
//! centres, as in ImageJ, so their coordinates are half a pixel less than
//! where they are drawn.
//!
//! # Whole pixels or fractions
//!
//! ImageJ's rule, from `Roi.grow`, `Line`, `PolygonRoi` and `PointRoi`:
//! rectangles and ovals always lie on pixel edges; every other shape has
//! whole-number coordinates unless the image is magnified beyond 150%
//! (`Roi.magnificationForSubPixel`), where the pointer's position is kept
//! with its fraction. Fractions are kept to a hundredth of a pixel, which
//! is finer than one screen pixel at the largest zoom.

use calipix_roi::{Combine, Point, Roi};

/// A tool of the tool strip.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Tool {
    /// Moves and reshapes the selection, and picks regions. Draws nothing.
    #[default]
    Pointer,
    Rectangle,
    Oval,
    Polygon,
    Freehand,
    Line,
    Polyline,
    Point,
}

impl Tool {
    /// Every tool, in the order of the tool strip.
    pub const ALL: [Tool; 8] = [
        Tool::Pointer,
        Tool::Rectangle,
        Tool::Oval,
        Tool::Polygon,
        Tool::Freehand,
        Tool::Line,
        Tool::Polyline,
        Tool::Point,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Tool::Pointer => "Pointer",
            Tool::Rectangle => "Rectangle",
            Tool::Oval => "Oval",
            Tool::Polygon => "Polygon",
            Tool::Freehand => "Freehand",
            Tool::Line => "Line",
            Tool::Polyline => "Segmented Line",
            Tool::Point => "Point",
        }
    }

    /// Stable `snake_case` name, used in command ids.
    pub fn key(self) -> &'static str {
        match self {
            Tool::Pointer => "pointer",
            Tool::Rectangle => "rectangle",
            Tool::Oval => "oval",
            Tool::Polygon => "polygon",
            Tool::Freehand => "freehand",
            Tool::Line => "line",
            Tool::Polyline => "segmented_line",
            Tool::Point => "point",
        }
    }

    pub fn from_key(key: &str) -> Option<Tool> {
        Tool::ALL.into_iter().find(|tool| tool.key() == key)
    }

    /// The command that makes this the active tool.
    pub fn command_id(self) -> String {
        format!("tool.{}", self.key())
    }

    /// The key that chooses the tool.
    pub fn shortcut(self) -> &'static str {
        match self {
            Tool::Pointer => "V",
            Tool::Rectangle => "R",
            Tool::Oval => "O",
            Tool::Polygon => "G",
            Tool::Freehand => "F",
            Tool::Line => "L",
            Tool::Polyline => "S",
            Tool::Point => "P",
        }
    }

    /// How to use the tool, for its tooltip and the status bar.
    pub fn hint(self) -> &'static str {
        match self {
            Tool::Pointer => {
                "Drag a selection to move it, or a handle to reshape it. Click a region to select it"
            }
            Tool::Rectangle => "Drag to select a rectangle. Shift makes it square, Alt centres it",
            Tool::Oval => "Drag to select an oval. Shift makes it a circle, Alt centres it",
            Tool::Polygon => {
                "Click each corner; click the first corner or double-click to finish. Esc cancels"
            }
            Tool::Freehand => "Drag to draw the outline of an area",
            Tool::Line => "Drag to draw a line. Shift keeps it to steps of 45°",
            Tool::Polyline => "Click each point of the line; double-click to finish. Esc cancels",
            Tool::Point => "Click to mark a point. Shift adds points, Alt removes one",
        }
    }
}

/// Which labels are drawn on the regions of an image.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RegionLabels {
    None,
    /// Each region's number in the list.
    #[default]
    Numbers,
    /// Each region's name, or its number if it has none.
    Names,
}

impl RegionLabels {
    pub const ALL: [RegionLabels; 3] = [
        RegionLabels::None,
        RegionLabels::Numbers,
        RegionLabels::Names,
    ];

    pub fn title(self) -> &'static str {
        match self {
            RegionLabels::None => "No Region Labels",
            RegionLabels::Numbers => "Region Numbers",
            RegionLabels::Names => "Region Names",
        }
    }

    /// What choosing it does, for the command palette.
    pub fn summary(self) -> &'static str {
        match self {
            RegionLabels::None => "Draws regions on images without labels.",
            RegionLabels::Numbers => "Labels each region on the image with its number in the list.",
            RegionLabels::Names => {
                "Labels each region on the image with its name, or its number if it has none."
            }
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            RegionLabels::None => "none",
            RegionLabels::Numbers => "numbers",
            RegionLabels::Names => "names",
        }
    }
}

/// The modifier keys held down.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Mods {
    pub shift: bool,
    pub alt: bool,
}

impl Mods {
    pub const NONE: Mods = Mods {
        shift: false,
        alt: false,
    };
    pub const SHIFT: Mods = Mods {
        shift: true,
        alt: false,
    };
    pub const ALT: Mods = Mods {
        shift: false,
        alt: true,
    };
}

/// The pointer over an image.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pointer {
    /// Where it is, in image coordinates as drawn.
    pub at: [f64; 2],
    /// Magnification: screen pixels per image pixel.
    pub zoom: f64,
    /// How far from a handle or a line, in image pixels, still grabs it.
    pub reach: f64,
    pub mods: Mods,
}

impl Pointer {
    /// A pointer at `at` on an image shown at `zoom`, grabbing what is
    /// within six screen pixels.
    pub fn new(at: [f64; 2], zoom: f64) -> Pointer {
        Pointer {
            at,
            zoom,
            reach: 6.0 / zoom,
            mods: Mods::NONE,
        }
    }

    pub fn with(mut self, mods: Mods) -> Pointer {
        self.mods = mods;
        self
    }
}

/// The magnification beyond which ImageJ keeps fractions of a pixel:
/// `Roi.magnificationForSubPixel`.
pub const SUB_PIXEL_ZOOM: f64 = 1.5;

fn fine(zoom: f64) -> bool {
    zoom > SUB_PIXEL_ZOOM
}

fn hundredth(v: f64) -> f64 {
    let rounded = (v * 100.0).round() / 100.0;
    // Not `-0`, which would be written into the selection's text.
    if rounded == 0.0 { 0.0 } else { rounded }
}

/// The pixel edge nearest the pointer: `ImageCanvas.offScreenX2`.
fn snap_edge(at: [f64; 2], size: (f64, f64)) -> Point {
    (
        at[0].clamp(0.0, size.0).round(),
        at[1].clamp(0.0, size.1).round(),
    )
}

/// A vertex of an area outline under the pointer.
fn snap_area(p: &Pointer, size: (f64, f64)) -> Point {
    let (x, y) = (p.at[0].clamp(0.0, size.0), p.at[1].clamp(0.0, size.1));
    if fine(p.zoom) {
        (hundredth(x), hundredth(y))
    } else {
        (x.round(), y.round())
    }
}

/// A point of a line or point selection under the pointer: the centre of
/// the pixel it is on, or its exact place when magnified.
fn snap_centre(p: &Pointer, size: (f64, f64)) -> Point {
    let limit = |v: f64, max: f64| (v - 0.5).clamp(0.0, (max - 1.0).max(0.0));
    let (x, y) = (limit(p.at[0], size.0), limit(p.at[1], size.1));
    if fine(p.zoom) {
        (hundredth(x), hundredth(y))
    } else {
        (x.round(), y.round())
    }
}

/// `to` moved so that the line from `from` runs along an axis or a
/// diagonal, whichever is nearest: what Shift does to a line.
fn constrain_45(from: Point, to: Point) -> Point {
    let (dx, dy) = (to.0 - from.0, to.1 - from.1);
    let (ax, ay) = (dx.abs(), dy.abs());
    // tan(22.5°) and tan(67.5°) divide the three cases.
    if ay <= ax * 0.414_213_562 {
        (to.0, from.1)
    } else if ay >= ax * 2.414_213_562 {
        (from.0, to.1)
    } else {
        (to.0, from.1 + ax * dy.signum())
    }
}

/// How far the positions of a selection's vertices are from where they
/// are drawn: half a pixel for lines and points, nothing for areas.
pub fn drawn_offset(roi: &Roi) -> f64 {
    if roi.is_area() { 0.0 } else { 0.5 }
}

/// Where a selection can be grabbed to reshape it, as drawn: the corners
/// and edge middles of a rectangle or oval (clockwise from the top left),
/// the vertices of a polygon or line, the points of a point selection.
/// Shapes drawn freehand or made by commands have none; they can still be
/// moved.
pub fn handles(roi: &Roi) -> Vec<[f64; 2]> {
    match roi {
        Roi::Rectangle {
            x,
            y,
            width,
            height,
        }
        | Roi::Oval {
            x,
            y,
            width,
            height,
        } => {
            let (x0, y0, x1, y1) = (*x, *y, x + width, y + height);
            let (xm, ym) = ((x0 + x1) / 2.0, (y0 + y1) / 2.0);
            vec![
                [x0, y0],
                [xm, y0],
                [x1, y0],
                [x1, ym],
                [x1, y1],
                [xm, y1],
                [x0, y1],
                [x0, ym],
            ]
        }
        Roi::Polygon { .. }
        | Roi::Polyline { .. }
        | Roi::Angle { .. }
        | Roi::Points { .. }
        | Roi::Line { .. } => {
            let offset = drawn_offset(roi);
            roi.vertices()
                .unwrap_or_default()
                .into_iter()
                .map(|(x, y)| [x + offset, y + offset])
                .collect()
        }
        _ => Vec::new(),
    }
}

/// What of a selection is under the pointer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hit {
    /// A handle, by its place in [`handles`].
    Handle(usize),
    /// The selection itself: inside an area, or on a line.
    Inside,
    Outside,
}

fn distance_to_segment(p: [f64; 2], a: [f64; 2], b: [f64; 2]) -> f64 {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let length2 = dx * dx + dy * dy;
    let t = if length2 > 0.0 {
        (((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / length2).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let (cx, cy) = (a[0] + t * dx, a[1] + t * dy);
    ((p[0] - cx).powi(2) + (p[1] - cy).powi(2)).sqrt()
}

/// What of `roi` a pointer at `at` would grab, reaching `reach` image
/// pixels.
pub fn hit(roi: &Roi, at: [f64; 2], reach: f64) -> Hit {
    let nearest = handles(roi)
        .into_iter()
        .enumerate()
        .map(|(index, h)| (index, (h[0] - at[0]).abs().max((h[1] - at[1]).abs())))
        .filter(|(_, distance)| *distance <= reach)
        .min_by(|a, b| a.1.total_cmp(&b.1));
    if let Some((index, _)) = nearest {
        return Hit::Handle(index);
    }
    let inside = if roi.is_area() {
        roi.contains(at[0].floor() as i32, at[1].floor() as i32)
    } else if roi.is_line() {
        let points: Vec<[f64; 2]> = roi
            .vertices()
            .unwrap_or_default()
            .into_iter()
            .map(|(x, y)| [x + 0.5, y + 0.5])
            .collect();
        let within = reach.max(roi.line_width() / 2.0);
        points
            .windows(2)
            .any(|pair| distance_to_segment(at, pair[0], pair[1]) <= within)
    } else {
        false
    };
    if inside { Hit::Inside } else { Hit::Outside }
}

/// The same kind of selection with other vertices.
fn with_vertices(roi: &Roi, points: Vec<Point>) -> Roi {
    match roi {
        Roi::Polygon { .. } => Roi::Polygon { points },
        Roi::Polyline { width, .. } => Roi::Polyline {
            points,
            width: *width,
        },
        Roi::Points { .. } => Roi::Points { points },
        Roi::Angle { .. } => match points.as_slice() {
            [a, b, c] => Roi::Angle {
                points: [*a, *b, *c],
            },
            _ => roi.clone(),
        },
        Roi::Line { width, .. } => match points.as_slice() {
            [a, b] => Roi::Line {
                x1: a.0,
                y1: a.1,
                x2: b.0,
                y2: b.1,
                width: *width,
            },
            _ => roi.clone(),
        },
        other => other.clone(),
    }
}

/// The fewest vertices a selection of this kind can have.
fn least_vertices(roi: &Roi) -> usize {
    match roi {
        Roi::Polygon { .. } => 3,
        Roi::Polyline { .. } => 2,
        Roi::Points { .. } => 1,
        _ => usize::MAX,
    }
}

/// A rectangle or oval with these bounds.
fn boxed(oval: bool, x: f64, y: f64, width: f64, height: f64) -> Roi {
    if oval {
        Roi::oval(x, y, width, height)
    } else {
        Roi::rectangle(x, y, width, height)
    }
}

/// What a finished gesture asks for. Each becomes one command.
#[derive(Clone, Debug, PartialEq)]
pub enum Commit {
    /// Make this the selection.
    Set(Roi),
    /// Move the selection by whole pixels.
    Translate { dx: f64, dy: f64 },
    /// Remove the selection.
    Clear,
    /// The image was clicked where there is nothing of the selection:
    /// that picks a region there, or failing that removes the selection.
    Click([f64; 2]),
}

/// What a pointer event did to an interaction.
#[derive(Clone, Debug, PartialEq)]
pub enum Step {
    /// The gesture goes on.
    Continue,
    /// The gesture is over, with this to carry out, if anything.
    Done(Option<Commit>),
}

#[derive(Clone, Debug, PartialEq)]
enum Mode {
    /// A press that grabs nothing and draws nothing.
    Idle,
    /// A rectangle or oval dragged out from `anchor`.
    Box {
        oval: bool,
        anchor: Point,
        roi: Option<Roi>,
    },
    Line {
        from: Point,
        to: Point,
    },
    /// An outline drawn with the button held down.
    Free {
        points: Vec<Point>,
    },
    /// A polygon or segmented line made click by click.
    Path {
        line: bool,
        points: Vec<Point>,
        hover: Option<Point>,
    },
    Move {
        from: [f64; 2],
        by: (f64, f64),
    },
    /// A handle being dragged: `start` is the shape when it was pressed,
    /// `roi` the shape now. `remove` takes the vertex out instead.
    Reshape {
        handle: usize,
        start: Roi,
        roi: Roi,
        remove: bool,
    },
}

/// An unfinished outline, to be drawn with a rubber band to the pointer.
#[derive(Clone, Debug, PartialEq)]
pub struct Draft {
    /// The vertices placed so far.
    pub points: Vec<Point>,
    /// Where the next vertex would go.
    pub hover: Option<Point>,
    /// Vertices name pixel centres (a line) rather than pixel edges.
    pub line: bool,
    /// The outline will be closed when finished.
    pub closed: bool,
    /// The pointer is where a click would finish the outline.
    pub finishing: bool,
}

/// What to draw while a gesture lasts.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Shown {
    /// Drawn in place of the image's selection: `Some(None)` hides it.
    pub selection: Option<Option<Roi>>,
    /// A shape being added to or taken from the selection, drawn with it.
    pub extra: Option<Roi>,
    pub draft: Option<Draft>,
    /// The handle being dragged.
    pub handle: Option<usize>,
}

/// One gesture on one image, from the press that starts it to the event
/// that ends it.
#[derive(Clone, Debug, PartialEq)]
pub struct Interaction {
    mode: Mode,
    /// The image's selection when the gesture began.
    base: Option<Roi>,
    /// How a new area joins the selection: Shift at the press adds it,
    /// Alt takes it away.
    combine: Option<Combine>,
    /// Width and height of the image.
    size: (f64, f64),
}

impl Interaction {
    /// Starts a gesture with a press of the primary button.
    pub fn begin(
        tool: Tool,
        selection: Option<&Roi>,
        size: (usize, usize),
        p: Pointer,
    ) -> Interaction {
        let size = (size.0 as f64, size.1 as f64);
        let draws_area = matches!(
            tool,
            Tool::Rectangle | Tool::Oval | Tool::Polygon | Tool::Freehand
        );
        let combine = match selection {
            Some(roi) if draws_area && roi.is_area() && p.mods.shift => Some(Combine::Or),
            Some(roi) if draws_area && roi.is_area() && p.mods.alt => Some(Combine::Not),
            _ => None,
        };
        let mut interaction = Interaction {
            mode: Mode::Idle,
            base: selection.cloned(),
            combine,
            size,
        };
        let grabbed = selection.map_or(Hit::Outside, |roi| hit(roi, p.at, p.reach));
        interaction.mode = match (tool, selection, grabbed) {
            (Tool::Point, _, _) => interaction.begin_point(selection, grabbed, &p),
            _ if combine.is_some() => interaction.begin_drawing(tool, &p),
            (_, Some(roi), Hit::Handle(handle)) => Mode::Reshape {
                handle,
                start: roi.clone(),
                roi: roi.clone(),
                // ImageJ: Alt-click on a vertex deletes it.
                remove: p.mods.alt
                    && roi
                        .vertices()
                        .is_some_and(|v| v.len() > least_vertices(roi)),
            },
            (_, Some(_), Hit::Inside) => Mode::Move {
                from: p.at,
                by: (0.0, 0.0),
            },
            _ => interaction.begin_drawing(tool, &p),
        };
        interaction
    }

    fn begin_drawing(&self, tool: Tool, p: &Pointer) -> Mode {
        match tool {
            Tool::Pointer | Tool::Point => Mode::Idle,
            Tool::Rectangle | Tool::Oval => Mode::Box {
                oval: tool == Tool::Oval,
                anchor: snap_edge(p.at, self.size),
                roi: None,
            },
            Tool::Line => {
                let from = snap_centre(p, self.size);
                Mode::Line { from, to: from }
            }
            Tool::Freehand => Mode::Free {
                points: vec![snap_area(p, self.size)],
            },
            Tool::Polygon => Mode::Path {
                line: false,
                points: vec![snap_area(p, self.size)],
                hover: None,
            },
            Tool::Polyline => Mode::Path {
                line: true,
                points: vec![snap_centre(p, self.size)],
                hover: None,
            },
        }
    }

    /// The point tool: a click marks a point, replacing the selection;
    /// Shift adds to the points there are, Alt on a point removes it, and
    /// a point can be dragged.
    fn begin_point(&self, selection: Option<&Roi>, grabbed: Hit, p: &Pointer) -> Mode {
        let existing = match selection {
            Some(Roi::Points { points }) => Some(points),
            _ => None,
        };
        let at = snap_centre(p, self.size);
        match (existing, grabbed) {
            (Some(points), Hit::Handle(handle)) => Mode::Reshape {
                handle,
                start: Roi::points(points.clone()),
                roi: Roi::points(points.clone()),
                remove: p.mods.alt && points.len() > 1,
            },
            (Some(points), _) if p.mods.shift => {
                let mut points = points.clone();
                points.push(at);
                Mode::Reshape {
                    handle: points.len() - 1,
                    start: Roi::points(points.clone()),
                    roi: Roi::points(points),
                    remove: false,
                }
            }
            _ => Mode::Reshape {
                handle: 0,
                start: Roi::point(at.0, at.1),
                roi: Roi::point(at.0, at.1),
                remove: false,
            },
        }
    }

    /// True while a polygon or segmented line is being made, which goes
    /// on between presses.
    pub fn is_path(&self) -> bool {
        matches!(self.mode, Mode::Path { .. })
    }

    /// A further press of the primary button, while a polygon or
    /// segmented line is being made: it adds a vertex, or finishes the
    /// shape when it lands on the first vertex or again on the last (the
    /// second click of a double-click).
    pub fn press(&mut self, p: Pointer) -> Step {
        let size = self.size;
        let Mode::Path {
            line,
            points,
            hover,
        } = &mut self.mode
        else {
            return Step::Continue;
        };
        let line = *line;
        let least = if line { 2 } else { 3 };
        let offset = if line { 0.5 } else { 0.0 };
        let near = |v: &Point| {
            (v.0 + offset - p.at[0])
                .abs()
                .max((v.1 + offset - p.at[1]).abs())
                <= p.reach
        };
        let on_first = !line && points.first().is_some_and(near);
        let on_last = points.last().is_some_and(near);
        // Before there are enough vertices to finish, a click near one
        // is simply the next vertex.
        if (on_first || on_last) && points.len() >= least {
            return Step::Done(self.finish_path());
        }
        let mut at = if line {
            snap_centre(&p, size)
        } else {
            snap_area(&p, size)
        };
        if p.mods.shift
            && let Some(&last) = points.last()
        {
            at = constrain_45(last, at);
        }
        if points.last() != Some(&at) {
            points.push(at);
        }
        *hover = None;
        Step::Continue
    }

    fn finish_path(&self) -> Option<Commit> {
        let Mode::Path { line, points, .. } = &self.mode else {
            return None;
        };
        let least = if *line { 2 } else { 3 };
        if points.len() < least {
            return None;
        }
        if *line {
            Some(Commit::Set(Roi::polyline(points.clone())))
        } else {
            self.commit_area(Roi::polygon(points.clone()))
        }
    }

    /// A new area as the selection, or combined with the one there was.
    fn commit_area(&self, area: Roi) -> Option<Commit> {
        let commit = match (self.combine, &self.base) {
            (Some(how), Some(base)) => match base.combined(&area, how) {
                Some(combined) => Commit::Set(combined),
                // Everything was taken away.
                None if how == Combine::Not => Commit::Clear,
                None => return None,
            },
            _ => Commit::Set(area),
        };
        match (&commit, &self.base) {
            (Commit::Set(roi), Some(base)) if roi == base => None,
            _ => Some(commit),
        }
    }

    /// The pointer moved with the button down.
    pub fn drag(&mut self, p: Pointer) {
        let size = self.size;
        let combining = self.combine.is_some();
        match &mut self.mode {
            Mode::Idle | Mode::Path { .. } => {}
            Mode::Box { oval, anchor, roi } => {
                let to = snap_edge(p.at, size);
                // While adding to or taking from a selection the modifier
                // says which, and does not constrain the shape.
                let mods = if combining { Mods::NONE } else { p.mods };
                *roi = dragged_box(*oval, *anchor, to, mods, size);
            }
            Mode::Line { from, to } => {
                *to = snap_centre(&p, size);
                if p.mods.shift {
                    *to = constrain_45(*from, *to);
                }
            }
            Mode::Free { points } => {
                let at = snap_area(&p, size);
                if points.last() != Some(&at) {
                    points.push(at);
                }
            }
            Mode::Move { from, by } => {
                let (mut dx, mut dy) = (p.at[0] - from[0], p.at[1] - from[1]);
                // ImageJ's `Roi.move`: Shift keeps to one axis.
                if p.mods.shift {
                    if dx.abs() > dy.abs() {
                        dy = 0.0;
                    } else {
                        dx = 0.0;
                    }
                }
                let (mut dx, mut dy) = (dx.round(), dy.round());
                // A rectangle stays within the image, as in ImageJ.
                if let Some(Roi::Rectangle {
                    x,
                    y,
                    width,
                    height,
                }) = &self.base
                    && *width <= size.0
                    && *height <= size.1
                {
                    dx = dx.clamp(-x.floor(), (size.0 - (x + width)).floor().max(-x.floor()));
                    dy = dy.clamp(-y.floor(), (size.1 - (y + height)).floor().max(-y.floor()));
                }
                *by = (dx + 0.0, dy + 0.0);
            }
            Mode::Reshape {
                handle,
                start,
                roi,
                remove,
            } => {
                if !*remove {
                    *roi = reshaped(start, *handle, &p, size);
                }
            }
        }
    }

    /// The pointer moved with no button down: the rubber band of a
    /// polygon or segmented line follows it.
    pub fn hover(&mut self, p: Pointer) {
        let size = self.size;
        if let Mode::Path {
            line,
            points,
            hover,
        } = &mut self.mode
        {
            let mut at = if *line {
                snap_centre(&p, size)
            } else {
                snap_area(&p, size)
            };
            if p.mods.shift
                && let Some(&last) = points.last()
            {
                at = constrain_45(last, at);
            }
            *hover = Some(at);
        }
    }

    /// The primary button was released.
    pub fn release(&mut self, p: Pointer) -> Step {
        self.drag(p);
        let commit = match &self.mode {
            Mode::Path { .. } => return Step::Continue,
            Mode::Idle => Some(Commit::Click(p.at)),
            Mode::Box { roi, .. } => match roi {
                Some(roi) => self.commit_area(roi.clone()),
                // Pressed and released in one place: a click.
                None => Some(Commit::Click(p.at)),
            },
            Mode::Line { from, to } if from == to => Some(Commit::Click(p.at)),
            Mode::Line { from, to } => Some(Commit::Set(Roi::line(from.0, from.1, to.0, to.1))),
            Mode::Free { points } if points.len() < 3 => Some(Commit::Click(p.at)),
            Mode::Free { points } => self.commit_area(Roi::Freehand {
                points: points.clone(),
            }),
            Mode::Move { by, .. } if *by == (0.0, 0.0) => None,
            Mode::Move { by, .. } => Some(Commit::Translate { dx: by.0, dy: by.1 }),
            Mode::Reshape {
                handle,
                roi,
                remove,
                ..
            } => {
                let roi = if *remove {
                    let mut points = roi.vertices().unwrap_or_default();
                    if *handle < points.len() {
                        points.remove(*handle);
                    }
                    with_vertices(roi, points)
                } else {
                    roi.clone()
                };
                (self.base.as_ref() != Some(&roi)).then_some(Commit::Set(roi))
            }
        };
        Step::Done(commit)
    }

    /// Enter: finishes a polygon or segmented line that has enough
    /// vertices.
    pub fn finish(&mut self) -> Step {
        match self.finish_path() {
            Some(commit) => Step::Done(Some(commit)),
            None => Step::Continue,
        }
    }

    /// Backspace: takes back the last vertex of a polygon or segmented
    /// line. Returns false if that leaves nothing, and the gesture is over.
    pub fn undo_vertex(&mut self) -> bool {
        match &mut self.mode {
            Mode::Path { points, .. } => {
                points.pop();
                !points.is_empty()
            }
            _ => true,
        }
    }

    /// What to draw for the gesture as it stands.
    pub fn shown(&self) -> Shown {
        let combining = self.combine.is_some();
        // A new shape takes the place of the selection unless it is being
        // combined with it.
        let replacing = |roi: Option<Roi>| {
            if combining {
                Shown {
                    extra: roi,
                    ..Shown::default()
                }
            } else {
                Shown {
                    selection: Some(roi),
                    ..Shown::default()
                }
            }
        };
        match &self.mode {
            Mode::Idle => Shown::default(),
            Mode::Box { roi, .. } => replacing(roi.clone()),
            Mode::Line { from, to } => {
                replacing((from != to).then(|| Roi::line(from.0, from.1, to.0, to.1)))
            }
            Mode::Free { points } => Shown {
                draft: Some(Draft {
                    points: points.clone(),
                    hover: None,
                    line: false,
                    closed: false,
                    finishing: false,
                }),
                ..replacing(None)
            },
            Mode::Path {
                line,
                points,
                hover,
            } => {
                let least = if *line { 2 } else { 3 };
                let finishing = points.len() >= least
                    && hover.is_some_and(|h| {
                        Some(&h) == points.last() || (!*line && Some(&h) == points.first())
                    });
                Shown {
                    draft: Some(Draft {
                        points: points.clone(),
                        hover: *hover,
                        line: *line,
                        closed: !*line,
                        finishing,
                    }),
                    ..replacing(None)
                }
            }
            Mode::Move { by, .. } => Shown {
                selection: self
                    .base
                    .as_ref()
                    .map(|base| Some(base.translated(by.0, by.1))),
                ..Shown::default()
            },
            Mode::Reshape { handle, roi, .. } => Shown {
                selection: Some(Some(roi.clone())),
                handle: Some(*handle),
                ..Shown::default()
            },
        }
    }

    /// The shape the gesture would give if it ended now, for the status
    /// bar's running account of its size.
    pub fn measured(&self) -> Option<Roi> {
        let shown = self.shown();
        if let Some(draft) = shown.draft {
            let mut points = draft.points;
            points.extend(draft.hover.filter(|h| Some(h) != points.last()));
            return match (draft.line, points.len()) {
                (_, 0 | 1) => None,
                (true, _) | (false, 2) => Some(Roi::polyline(points)),
                (false, _) => Some(Roi::polygon(points)),
            };
        }
        shown.extra.or(shown.selection.flatten())
    }
}

/// The rectangle or oval dragged from `anchor` to `to`, or `None` while
/// it has no width or no height.
fn dragged_box(oval: bool, anchor: Point, to: Point, mods: Mods, size: (f64, f64)) -> Option<Roi> {
    let (mut dx, mut dy) = (to.0 - anchor.0, to.1 - anchor.1);
    if mods.shift {
        // A square or circle, as large as the longer side of the drag.
        let mut side = dx.abs().max(dy.abs());
        if !oval && !mods.alt {
            // A rectangle stays within the image.
            let room_x = if dx < 0.0 {
                anchor.0
            } else {
                size.0 - anchor.0
            };
            let room_y = if dy < 0.0 {
                anchor.1
            } else {
                size.1 - anchor.1
            };
            side = side.min(room_x).min(room_y);
        }
        let sign = |v: f64| if v < 0.0 { -1.0 } else { 1.0 };
        (dx, dy) = (side * sign(dx), side * sign(dy));
    }
    let (x, y, width, height) = if mods.alt {
        // Centred on where the drag began.
        (
            anchor.0 - dx.abs(),
            anchor.1 - dy.abs(),
            dx.abs() * 2.0,
            dy.abs() * 2.0,
        )
    } else {
        (
            anchor.0.min(anchor.0 + dx),
            anchor.1.min(anchor.1 + dy),
            dx.abs(),
            dy.abs(),
        )
    };
    (width > 0.0 && height > 0.0).then(|| boxed(oval, x, y, width, height))
}

/// `base` with one handle moved to the pointer.
fn reshaped(base: &Roi, handle: usize, p: &Pointer, size: (f64, f64)) -> Roi {
    match base {
        Roi::Rectangle {
            x,
            y,
            width,
            height,
        }
        | Roi::Oval {
            x,
            y,
            width,
            height,
        } => {
            let oval = matches!(base, Roi::Oval { .. });
            // An oval may reach past the image; a rectangle may not.
            let to = if oval {
                (p.at[0].round(), p.at[1].round())
            } else {
                snap_edge(p.at, size)
            };
            let (mut x0, mut y0, mut x1, mut y1) = (*x, *y, x + width, y + height);
            // Clockwise from the top left: which edges the handle moves.
            let (left, top, right, bottom) = match handle {
                0 => (true, true, false, false),
                1 => (false, true, false, false),
                2 => (false, true, true, false),
                3 => (false, false, true, false),
                4 => (false, false, true, true),
                5 => (false, false, false, true),
                6 => (true, false, false, true),
                _ => (true, false, false, false),
            };
            if left {
                x0 = to.0;
            }
            if right {
                x1 = to.0;
            }
            if top {
                y0 = to.1;
            }
            if bottom {
                y1 = to.1;
            }
            let corner = (left || right) && (top || bottom);
            if p.mods.shift && corner {
                // Square, growing from the opposite corner.
                let side = (x1 - x0).abs().max((y1 - y0).abs());
                if left {
                    x0 = x1 - side * if x0 > x1 { -1.0 } else { 1.0 };
                } else {
                    x1 = x0 + side * if x1 < x0 { -1.0 } else { 1.0 };
                }
                if top {
                    y0 = y1 - side * if y0 > y1 { -1.0 } else { 1.0 };
                } else {
                    y1 = y0 + side * if y1 < y0 { -1.0 } else { 1.0 };
                }
            }
            let (width, height) = ((x1 - x0).abs(), (y1 - y0).abs());
            if width < 1.0 || height < 1.0 {
                // Never thinner than a pixel.
                return base.clone();
            }
            boxed(oval, x0.min(x1), y0.min(y1), width, height)
        }
        _ => {
            let Some(mut points) = base.vertices() else {
                return base.clone();
            };
            if handle >= points.len() {
                return base.clone();
            }
            let mut at = if base.is_area() {
                snap_area(p, size)
            } else {
                snap_centre(p, size)
            };
            if p.mods.shift
                && let Roi::Line { .. } = base
            {
                at = constrain_45(points[1 - handle], at);
            }
            points[handle] = at;
            with_vertices(base, points)
        }
    }
}

/// The selection resized by one pixel with an arrow key and Alt, as
/// ImageJ's `Roi.nudgeCorner` does: rectangles and ovals grow or shrink
/// at their right and bottom edges. `None` for other shapes, or where it
/// would leave nothing.
pub fn nudged_size(roi: &Roi, dx: f64, dy: f64) -> Option<Roi> {
    let (x, y, width, height) = match roi {
        Roi::Rectangle {
            x,
            y,
            width,
            height,
        }
        | Roi::Oval {
            x,
            y,
            width,
            height,
        } => (*x, *y, *width, *height),
        _ => return None,
    };
    let (width, height) = (width + dx, height + dy);
    (width >= 1.0 && height >= 1.0)
        .then(|| boxed(matches!(roi, Roi::Oval { .. }), x, y, width, height))
}

/// What the pointer would do if pressed, for choosing a cursor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Aim {
    /// Nothing in particular.
    Nothing,
    /// Start drawing a shape.
    Draw,
    /// Move the selection.
    Move,
    /// Drag a handle, by its place in [`handles`].
    Handle(usize),
}

/// What a press at `p` would do with `tool` and this selection.
pub fn aim(tool: Tool, selection: Option<&Roi>, p: &Pointer) -> Aim {
    let grabbed = selection.map_or(Hit::Outside, |roi| hit(roi, p.at, p.reach));
    let draws_area = matches!(
        tool,
        Tool::Rectangle | Tool::Oval | Tool::Polygon | Tool::Freehand
    );
    let combining =
        draws_area && selection.is_some_and(Roi::is_area) && (p.mods.shift || p.mods.alt);
    match (tool, grabbed) {
        (Tool::Point, Hit::Handle(handle)) if selection.is_some_and(Roi::is_points) => {
            Aim::Handle(handle)
        }
        (Tool::Point, _) => Aim::Draw,
        _ if combining => Aim::Draw,
        (_, Hit::Handle(handle)) => Aim::Handle(handle),
        (_, Hit::Inside) => Aim::Move,
        (Tool::Pointer, _) => Aim::Nothing,
        _ => Aim::Draw,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SIZE: (usize, usize) = (100, 80);

    fn at(x: f64, y: f64) -> Pointer {
        Pointer::new([x, y], 1.0)
    }

    fn zoomed(x: f64, y: f64, zoom: f64) -> Pointer {
        Pointer::new([x, y], zoom)
    }

    /// A press, moves with the button down and a release.
    fn gesture(tool: Tool, selection: Option<&Roi>, path: &[Pointer]) -> Option<Commit> {
        let mut interaction = Interaction::begin(tool, selection, SIZE, path[0]);
        for p in &path[1..path.len() - 1] {
            interaction.drag(*p);
        }
        match interaction.release(*path.last().unwrap()) {
            Step::Done(commit) => commit,
            Step::Continue => panic!("the gesture did not end with the release"),
        }
    }

    /// Clicks, each a press and a release in one place.
    fn clicks(tool: Tool, selection: Option<&Roi>, at: &[Pointer]) -> (Interaction, Step) {
        let mut interaction = Interaction::begin(tool, selection, SIZE, at[0]);
        let mut step = interaction.release(at[0]);
        for p in &at[1..] {
            interaction.hover(*p);
            step = interaction.press(*p);
            if step == Step::Continue {
                step = interaction.release(*p);
            }
        }
        (interaction, step)
    }

    fn text(commit: Option<Commit>) -> String {
        match commit {
            Some(Commit::Set(roi)) => roi.to_string(),
            other => format!("{other:?}"),
        }
    }

    #[test]
    fn dragging_makes_a_rectangle_on_pixel_edges() {
        let made = gesture(
            Tool::Rectangle,
            None,
            &[at(10.3, 20.6), at(25.0, 30.0), at(40.4, 60.5)],
        );
        assert_eq!(text(made), "rectangle 10 21 30 40");
        // Dragged up and to the left, it is the same rectangle.
        let made = gesture(Tool::Rectangle, None, &[at(40.4, 60.5), at(10.3, 20.6)]);
        assert_eq!(text(made), "rectangle 10 21 30 40");
        // Even magnified, a rectangle lies on pixel edges.
        let made = gesture(
            Tool::Rectangle,
            None,
            &[zoomed(10.3, 20.6, 8.0), zoomed(40.4, 60.6, 8.0)],
        );
        assert_eq!(text(made), "rectangle 10 21 30 40");
        // And it ends at the image's edge.
        let made = gesture(Tool::Rectangle, None, &[at(90.0, 70.0), at(130.0, 95.0)]);
        assert_eq!(text(made), "rectangle 90 70 10 10");
    }

    #[test]
    fn shift_makes_squares_and_alt_centres() {
        let shift = Mods::SHIFT;
        let made = gesture(
            Tool::Rectangle,
            None,
            &[at(10.0, 10.0), at(40.0, 25.0).with(shift)],
        );
        assert_eq!(text(made), "rectangle 10 10 30 30");
        // A square that would leave the image stops at its edge.
        let made = gesture(
            Tool::Rectangle,
            None,
            &[at(10.0, 60.0), at(60.0, 65.0).with(shift)],
        );
        assert_eq!(text(made), "rectangle 10 60 20 20");
        let made = gesture(
            Tool::Oval,
            None,
            &[at(50.0, 40.0), at(60.0, 45.0).with(Mods::ALT)],
        );
        assert_eq!(text(made), "oval 40 35 20 10");
        let both = Mods {
            shift: true,
            alt: true,
        };
        let made = gesture(
            Tool::Oval,
            None,
            &[at(50.0, 40.0), at(60.0, 45.0).with(both)],
        );
        assert_eq!(text(made), "oval 40 30 20 20");
    }

    #[test]
    fn a_click_clears_and_a_gesture_that_ends_where_it_began_does_nothing() {
        let rect = Roi::rectangle(10.0, 10.0, 20.0, 20.0);
        for tool in [
            Tool::Pointer,
            Tool::Rectangle,
            Tool::Oval,
            Tool::Freehand,
            Tool::Line,
        ] {
            // Outside the selection: a click on the empty image.
            let made = gesture(tool, Some(&rect), &[at(60.0, 60.0), at(60.0, 60.0)]);
            assert_eq!(made, Some(Commit::Click([60.0, 60.0])), "{tool:?}");
            // Inside it, pressed and released: nothing at all.
            let made = gesture(tool, Some(&rect), &[at(20.0, 20.0), at(20.2, 20.3)]);
            assert_eq!(made, None, "{tool:?}");
        }
        // A handle dragged out and back again.
        let made = gesture(
            Tool::Pointer,
            Some(&rect),
            &[at(30.0, 30.0), at(44.0, 41.0), at(30.2, 29.9)],
        );
        assert_eq!(made, None);
    }

    #[test]
    fn dragging_inside_moves_by_whole_pixels() {
        let oval = Roi::oval(10.0, 10.0, 20.0, 20.0);
        let mut interaction =
            Interaction::begin(Tool::Rectangle, Some(&oval), SIZE, at(20.0, 20.0));
        interaction.drag(at(25.4, 17.8));
        assert_eq!(
            interaction.shown().selection,
            Some(Some(Roi::oval(15.0, 8.0, 20.0, 20.0)))
        );
        assert_eq!(
            interaction.release(at(32.6, 14.2)),
            Step::Done(Some(Commit::Translate { dx: 13.0, dy: -6.0 }))
        );
        // Shift keeps the move to one axis.
        let made = gesture(
            Tool::Pointer,
            Some(&oval),
            &[at(20.0, 20.0), at(32.6, 14.2).with(Mods::SHIFT)],
        );
        assert_eq!(made, Some(Commit::Translate { dx: 13.0, dy: 0.0 }));
        // A rectangle cannot be dragged out of the image.
        let rect = Roi::rectangle(10.0, 10.0, 20.0, 20.0);
        let made = gesture(
            Tool::Pointer,
            Some(&rect),
            &[at(20.0, 20.0), at(-50.0, 500.0)],
        );
        assert_eq!(
            made,
            Some(Commit::Translate {
                dx: -10.0,
                dy: 50.0
            })
        );
    }

    #[test]
    fn handles_are_found_and_reshape() {
        let rect = Roi::rectangle(10.0, 20.0, 30.0, 40.0);
        assert_eq!(handles(&rect).len(), 8);
        assert_eq!(hit(&rect, [10.0, 20.0], 3.0), Hit::Handle(0));
        assert_eq!(hit(&rect, [42.0, 58.5], 3.0), Hit::Handle(4));
        assert_eq!(hit(&rect, [25.0, 21.0], 3.0), Hit::Handle(1));
        assert_eq!(hit(&rect, [25.0, 40.0], 3.0), Hit::Inside);
        assert_eq!(hit(&rect, [60.0, 40.0], 3.0), Hit::Outside);
        // Zoomed in, the reach is a fraction of a pixel.
        assert_eq!(hit(&rect, [12.0, 22.0], 0.2), Hit::Inside);

        // The bottom right corner, dragged.
        let made = gesture(
            Tool::Pointer,
            Some(&rect),
            &[at(40.0, 60.0), at(50.4, 70.2)],
        );
        assert_eq!(text(made), "rectangle 10 20 40 50");
        // The left edge, dragged past the right one: the rectangle flips.
        let made = gesture(
            Tool::Pointer,
            Some(&rect),
            &[at(10.0, 40.0), at(55.0, 12.0)],
        );
        assert_eq!(text(made), "rectangle 40 20 15 40");
        // A corner with Shift keeps it square.
        let made = gesture(
            Tool::Rectangle,
            Some(&rect),
            &[at(40.0, 60.0), at(50.0, 70.0).with(Mods::SHIFT)],
        );
        assert_eq!(text(made), "rectangle 10 20 50 50");
        // An oval has the same handles.
        let oval = Roi::oval(10.0, 20.0, 30.0, 40.0);
        let made = gesture(
            Tool::Pointer,
            Some(&oval),
            &[at(25.0, 20.0), at(25.0, 10.0)],
        );
        assert_eq!(text(made), "oval 10 10 30 50");
    }

    #[test]
    fn vertices_are_dragged_and_removed() {
        let polygon = Roi::polygon(vec![(10.0, 10.0), (50.0, 10.0), (50.0, 40.0), (10.0, 40.0)]);
        assert_eq!(hit(&polygon, [50.5, 39.0], 3.0), Hit::Handle(2));
        let made = gesture(
            Tool::Pointer,
            Some(&polygon),
            &[at(50.0, 40.0), at(60.2, 55.7)],
        );
        assert_eq!(text(made), "polygon 10,10 50,10 60,56 10,40");
        let made = gesture(
            Tool::Pointer,
            Some(&polygon),
            &[at(50.0, 40.0).with(Mods::ALT), at(50.0, 40.0)],
        );
        assert_eq!(text(made), "polygon 10,10 50,10 10,40");
        // A triangle cannot lose a corner: Alt then just drags.
        let triangle = Roi::polygon(vec![(10.0, 10.0), (50.0, 10.0), (10.0, 40.0)]);
        let made = gesture(
            Tool::Pointer,
            Some(&triangle),
            &[at(50.0, 10.0).with(Mods::ALT), at(50.0, 10.0)],
        );
        assert_eq!(made, None);

        // A line's ends are at pixel centres, half a pixel from where
        // its coordinates say.
        let line = Roi::line(10.0, 10.0, 40.0, 10.0);
        assert_eq!(handles(&line), vec![[10.5, 10.5], [40.5, 10.5]]);
        assert_eq!(hit(&line, [25.0, 11.0], 2.0), Hit::Inside);
        assert_eq!(hit(&line, [25.0, 20.0], 2.0), Hit::Outside);
        let made = gesture(Tool::Line, Some(&line), &[at(40.5, 10.5), at(60.9, 30.2)]);
        assert_eq!(text(made), "line 10 10 60 30");
        let made = gesture(
            Tool::Line,
            Some(&line),
            &[at(40.5, 10.5), at(60.9, 14.2).with(Mods::SHIFT)],
        );
        assert_eq!(text(made), "line 10 10 60 10");
    }

    #[test]
    fn lines_snap_to_pixel_centres_until_magnified() {
        let made = gesture(Tool::Line, None, &[at(10.7, 20.2), at(40.9, 30.6)]);
        assert_eq!(text(made), "line 10 20 40 30");
        // At exactly 150% ImageJ still uses whole pixels; beyond it, not.
        let made = gesture(
            Tool::Line,
            None,
            &[zoomed(10.7, 20.2, 1.5), zoomed(40.9, 30.6, 1.5)],
        );
        assert_eq!(text(made), "line 10 20 40 30");
        let made = gesture(
            Tool::Line,
            None,
            &[zoomed(10.7, 20.2, 2.0), zoomed(40.914, 30.6, 2.0)],
        );
        assert_eq!(text(made), "line 10.2 19.7 40.41 30.1");
        // Shift: horizontal, vertical or diagonal.
        let shift = Mods::SHIFT;
        let made = gesture(
            Tool::Line,
            None,
            &[at(10.5, 10.5), at(40.5, 14.5).with(shift)],
        );
        assert_eq!(text(made), "line 10 10 40 10");
        let made = gesture(
            Tool::Line,
            None,
            &[at(10.5, 10.5), at(13.5, 44.5).with(shift)],
        );
        assert_eq!(text(made), "line 10 10 10 44");
        let made = gesture(
            Tool::Line,
            None,
            &[at(10.5, 40.5), at(30.5, 14.5).with(shift)],
        );
        assert_eq!(text(made), "line 10 40 30 20");
    }

    #[test]
    fn a_polygon_is_clicked_corner_by_corner() {
        let corners = [
            at(10.0, 10.0),
            at(50.2, 10.4),
            at(50.0, 40.0),
            at(10.0, 40.0),
        ];
        // Finished by clicking the first corner again.
        let mut all = corners.to_vec();
        all.push(at(11.0, 9.0));
        let (interaction, step) = clicks(Tool::Polygon, None, &all);
        assert_eq!(
            step,
            Step::Done(Some(Commit::Set(Roi::polygon(vec![
                (10.0, 10.0),
                (50.0, 10.0),
                (50.0, 40.0),
                (10.0, 40.0)
            ]))))
        );
        assert!(interaction.is_path());
        // Finished by a double-click: a second click on the last corner.
        let mut all = corners.to_vec();
        all.push(at(10.0, 40.0));
        let (_, step) = clicks(Tool::Polygon, None, &all);
        assert!(
            matches!(step, Step::Done(Some(Commit::Set(Roi::Polygon { points }))) if points.len() == 4)
        );
        // Two corners are not a polygon: a double-click there goes on.
        let (mut interaction, step) = clicks(
            Tool::Polygon,
            None,
            &[at(10.0, 10.0), at(50.0, 10.0), at(50.0, 10.0)],
        );
        assert_eq!(step, Step::Continue);
        assert_eq!(interaction.finish(), Step::Continue);
        // Enter finishes once there are three.
        interaction.press(at(50.0, 40.0));
        assert!(matches!(interaction.finish(), Step::Done(Some(_))));
    }

    #[test]
    fn a_polygon_in_progress_has_a_rubber_band_and_can_step_back() {
        let (mut interaction, _) = clicks(
            Tool::Polygon,
            Some(&Roi::rectangle(80.0, 60.0, 5.0, 5.0)),
            &[at(10.0, 10.0), at(50.0, 10.0), at(50.0, 40.0)],
        );
        interaction.hover(at(30.3, 60.8));
        let shown = interaction.shown();
        // The old selection is out of the way while the new one is made.
        assert_eq!(shown.selection, Some(None));
        let draft = shown.draft.unwrap();
        assert_eq!(draft.points.len(), 3);
        assert_eq!(draft.hover, Some((30.0, 61.0)));
        assert!(draft.closed && !draft.line && !draft.finishing);
        assert_eq!(
            interaction.measured(),
            Some(Roi::polygon(vec![
                (10.0, 10.0),
                (50.0, 10.0),
                (50.0, 40.0),
                (30.0, 61.0)
            ]))
        );
        interaction.hover(at(10.0, 10.0));
        assert!(interaction.shown().draft.unwrap().finishing);

        assert!(interaction.undo_vertex());
        assert!(interaction.undo_vertex());
        assert_eq!(interaction.shown().draft.unwrap().points.len(), 1);
        assert!(!interaction.undo_vertex());
    }

    #[test]
    fn a_segmented_line_ends_with_a_double_click() {
        let (_, step) = clicks(
            Tool::Polyline,
            None,
            &[
                at(10.5, 10.5),
                at(30.5, 20.5),
                at(50.5, 10.5),
                at(50.5, 10.5),
            ],
        );
        assert_eq!(
            step,
            Step::Done(Some(Commit::Set(Roi::polyline(vec![
                (10.0, 10.0),
                (30.0, 20.0),
                (50.0, 10.0)
            ]))))
        );
        // Clicking the first point again does not close a line.
        let (_, step) = clicks(
            Tool::Polyline,
            None,
            &[at(10.5, 10.5), at(30.5, 20.5), at(10.5, 10.5)],
        );
        assert_eq!(step, Step::Continue);
    }

    #[test]
    fn polygons_keep_fractions_only_when_magnified() {
        let corners = |zoom: f64| {
            [
                zoomed(10.26, 10.74, zoom),
                zoomed(50.5, 10.5, zoom),
                zoomed(50.5, 40.5, zoom),
                zoomed(50.5, 40.5, zoom),
            ]
        };
        let (_, step) = clicks(Tool::Polygon, None, &corners(1.0));
        assert_eq!(
            step,
            Step::Done(Some(Commit::Set(Roi::polygon(vec![
                (10.0, 11.0),
                (51.0, 11.0),
                (51.0, 41.0)
            ]))))
        );
        let (_, step) = clicks(Tool::Polygon, None, &corners(4.0));
        assert_eq!(
            step,
            Step::Done(Some(Commit::Set(Roi::polygon(vec![
                (10.26, 10.74),
                (50.5, 10.5),
                (50.5, 40.5)
            ]))))
        );
    }

    #[test]
    fn freehand_follows_the_pointer() {
        let made = gesture(
            Tool::Freehand,
            None,
            &[
                at(10.0, 10.0),
                at(20.0, 10.2),
                at(20.1, 10.1),
                at(20.0, 20.0),
                at(10.0, 20.0),
            ],
        );
        assert_eq!(text(made), "freehand 10,10 20,10 20,20 10,20");
        let mut interaction = Interaction::begin(Tool::Freehand, None, SIZE, at(10.0, 10.0));
        interaction.drag(at(20.0, 10.0));
        let draft = interaction.shown().draft.unwrap();
        assert_eq!((draft.points.len(), draft.closed), (2, false));
    }

    #[test]
    fn points_are_marked_added_moved_and_removed() {
        let made = gesture(Tool::Point, None, &[at(10.7, 20.2), at(10.7, 20.2)]);
        assert_eq!(text(made), "points 10,20");
        let one = Roi::point(10.0, 20.0);
        // A plain click elsewhere replaces the point; Shift adds one.
        let made = gesture(Tool::Point, Some(&one), &[at(40.5, 50.5), at(40.5, 50.5)]);
        assert_eq!(text(made), "points 40,50");
        let shift = Mods::SHIFT;
        let made = gesture(
            Tool::Point,
            Some(&one),
            &[at(40.5, 50.5).with(shift), at(40.5, 50.5).with(shift)],
        );
        assert_eq!(text(made), "points 10,20 40,50");
        let two = Roi::points(vec![(10.0, 20.0), (40.0, 50.0)]);
        // Dragging a point moves it; Alt-click removes it.
        let made = gesture(Tool::Point, Some(&two), &[at(40.5, 50.5), at(45.5, 55.5)]);
        assert_eq!(text(made), "points 10,20 45,55");
        let made = gesture(
            Tool::Point,
            Some(&two),
            &[at(10.5, 20.5).with(Mods::ALT), at(10.5, 20.5)],
        );
        assert_eq!(text(made), "points 40,50");
        // Magnified, a point keeps its place within the pixel.
        let made = gesture(
            Tool::Point,
            None,
            &[zoomed(10.7, 20.2, 8.0), zoomed(10.7, 20.2, 8.0)],
        );
        assert_eq!(text(made), "points 10.2,19.7");
        // Clicking the same point again changes nothing.
        let made = gesture(Tool::Point, Some(&one), &[at(10.5, 20.5), at(10.5, 20.5)]);
        assert_eq!(made, None);
    }

    #[test]
    fn shift_adds_to_an_area_and_alt_takes_away() {
        let rect = Roi::rectangle(10.0, 10.0, 20.0, 20.0);
        let shift = Mods::SHIFT;
        let mut interaction = Interaction::begin(
            Tool::Rectangle,
            Some(&rect),
            SIZE,
            at(15.0, 15.0).with(shift),
        );
        interaction.drag(at(50.0, 25.0).with(shift));
        let shown = interaction.shown();
        // Both are on show: the selection as it is and the piece to add.
        assert_eq!(shown.selection, None);
        assert_eq!(shown.extra, Some(Roi::rectangle(15.0, 15.0, 35.0, 10.0)));
        let Step::Done(Some(Commit::Set(joined))) = interaction.release(at(50.0, 25.0).with(shift))
        else {
            panic!("nothing was added");
        };
        assert_eq!(joined.mask(100, 80).unwrap().count(), 400 + 200);

        let made = gesture(
            Tool::Rectangle,
            Some(&rect),
            &[at(0.0, 0.0).with(Mods::ALT), at(60.0, 60.0).with(Mods::ALT)],
        );
        assert_eq!(made, Some(Commit::Clear));
        let Some(Commit::Set(holed)) = gesture(
            Tool::Oval,
            Some(&rect),
            &[
                at(15.0, 15.0).with(Mods::ALT),
                at(25.0, 25.0).with(Mods::ALT),
            ],
        ) else {
            panic!("nothing was taken away");
        };
        assert!(holed.mask(100, 80).unwrap().count() < 400);
        assert!(!holed.contains(20, 20) && holed.contains(11, 11));
    }

    #[test]
    fn the_pointer_aims_at_what_it_would_grab() {
        let rect = Roi::rectangle(10.0, 10.0, 20.0, 20.0);
        assert_eq!(aim(Tool::Pointer, Some(&rect), &at(20.0, 20.0)), Aim::Move);
        assert_eq!(
            aim(Tool::Oval, Some(&rect), &at(30.0, 30.0)),
            Aim::Handle(4)
        );
        assert_eq!(aim(Tool::Oval, Some(&rect), &at(60.0, 60.0)), Aim::Draw);
        assert_eq!(
            aim(Tool::Pointer, Some(&rect), &at(60.0, 60.0)),
            Aim::Nothing
        );
        assert_eq!(aim(Tool::Pointer, None, &at(60.0, 60.0)), Aim::Nothing);
        assert_eq!(
            aim(Tool::Oval, Some(&rect), &at(20.0, 20.0).with(Mods::SHIFT)),
            Aim::Draw
        );
        assert_eq!(aim(Tool::Point, Some(&rect), &at(20.0, 20.0)), Aim::Draw);
    }

    #[test]
    fn alt_and_an_arrow_resize_boxes() {
        let rect = Roi::rectangle(10.0, 10.0, 20.0, 20.0);
        assert_eq!(
            nudged_size(&rect, 1.0, 0.0),
            Some(Roi::rectangle(10.0, 10.0, 21.0, 20.0))
        );
        assert_eq!(
            nudged_size(&Roi::rectangle(0.0, 0.0, 1.0, 5.0), -1.0, 0.0),
            None
        );
        assert_eq!(nudged_size(&Roi::line(0.0, 0.0, 5.0, 5.0), 1.0, 0.0), None);
    }

    #[test]
    fn tools_have_distinct_names_and_keys() {
        let mut seen = std::collections::BTreeSet::new();
        for tool in Tool::ALL {
            assert!(seen.insert(tool.shortcut()), "{tool:?}");
            assert!(seen.insert(tool.key()), "{tool:?}");
            assert_eq!(Tool::from_key(tool.key()), Some(tool));
            assert!(!tool.hint().is_empty());
        }
    }
}
