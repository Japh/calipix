//! What the pointer and the keyboard do to the selection of an image:
//! the tools of [`super::tools`] joined to the commands that record what
//! they did.
//!
//! The discipline is that of dragging a display range. While the pointer
//! is down nothing is recorded: the gesture is state here, and the viewer
//! draws it. When it ends it becomes one command (Specify Selection,
//! Translate Selection or Select None), so that each gesture is one step
//! in History and can be undone, repeated in a macro or made by an agent.
//! A gesture that is cancelled or changes nothing runs no command.

use calipix_command::{Args, DatasetId};
use calipix_core::ImageMeta;
use calipix_roi::Roi;

use super::tools::{self, Aim, Commit, Draft, Interaction, Pointer, Step, Tool};
use super::{AppState, Status};

/// What to draw over an image for its selection this frame.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CanvasView {
    /// The selection: the image's own, or the one a gesture is making.
    pub selection: Option<Roi>,
    /// A shape being added to or taken from the selection.
    pub extra: Option<Roi>,
    /// An outline still being drawn.
    pub draft: Option<Draft>,
    /// The handle being dragged.
    pub handle: Option<usize>,
}

/// A key that acts on the selection while the image has the keyboard.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum CanvasKey {
    /// Gives up the shape being drawn or dragged.
    Escape,
    /// Finishes a polygon or segmented line.
    Enter,
    /// Takes back the last vertex of a shape being drawn; otherwise as
    /// [`Delete`](Self::Delete).
    Backspace,
    /// Removes the regions picked in the list.
    Delete,
    /// An arrow: moves the selection by a pixel, or with `resize` (Alt)
    /// makes a rectangle or oval a pixel larger or smaller.
    Arrow { dx: i8, dy: i8, resize: bool },
}

impl AppState {
    /// The tool the pointer uses on images.
    pub fn tool(&self) -> Tool {
        self.tool
    }

    /// Chooses a tool. A shape being drawn with another is given up.
    pub fn set_tool(&mut self, tool: Tool) {
        if self.tool != tool {
            self.interaction = None;
        }
        self.tool = tool;
        self.status = Status::Done(format!("{}: {}", tool.title(), tool.hint()));
    }

    fn selection_of(&self, id: DatasetId) -> Option<&Roi> {
        self.session.workspace.get(id).ok()?.roi.as_ref()
    }

    fn size_of(&self, id: DatasetId) -> Option<(usize, usize)> {
        let meta = self.session.workspace.image(id).ok()?.meta();
        Some((meta.width(), meta.height()))
    }

    /// What to draw for the selection of image `id`: its own, unless a
    /// gesture is under way or its command has yet to finish.
    pub fn canvas_view(&self, id: DatasetId) -> CanvasView {
        let own = || self.selection_of(id).cloned();
        if let Some((image, interaction)) = &self.interaction
            && *image == id
        {
            let shown = interaction.shown();
            return CanvasView {
                selection: shown.selection.unwrap_or_else(own),
                extra: shown.extra,
                draft: shown.draft,
                handle: shown.handle,
            };
        }
        if let Some((image, selection)) = &self.pending_selection
            && *image == id
        {
            return CanvasView {
                selection: selection.clone(),
                ..CanvasView::default()
            };
        }
        CanvasView {
            selection: own(),
            ..CanvasView::default()
        }
    }

    /// True while a shape is being drawn, moved or reshaped.
    pub fn gesture_active(&self) -> bool {
        self.interaction.is_some()
    }

    /// True while a polygon or segmented line is being made click by
    /// click.
    pub fn drawing_path(&self) -> bool {
        self.interaction
            .as_ref()
            .is_some_and(|(_, interaction)| interaction.is_path())
    }

    /// What a press at `p` on image `id` would do, for the cursor.
    pub fn aim(&self, id: DatasetId, p: &Pointer) -> Aim {
        if self.drawing_path() {
            return Aim::Draw;
        }
        tools::aim(self.tool, self.selection_of(id), p)
    }

    /// The primary button went down on image `id`.
    pub fn pointer_pressed(&mut self, id: DatasetId, p: Pointer) {
        let Some(size) = self.size_of(id) else {
            return;
        };
        // A polygon under way on this image takes the press as a vertex.
        if let Some((image, interaction)) = &mut self.interaction
            && *image == id
            && interaction.is_path()
        {
            if let Step::Done(commit) = interaction.press(p) {
                self.interaction = None;
                self.carry_out(id, commit, &p);
            }
            return;
        }
        let interaction = Interaction::begin(self.tool, self.selection_of(id), size, p);
        self.interaction = Some((id, interaction));
    }

    /// The pointer moved over image `id` with the primary button down.
    pub fn pointer_dragged(&mut self, id: DatasetId, p: Pointer) {
        if let Some((image, interaction)) = &mut self.interaction
            && *image == id
        {
            interaction.drag(p);
        }
    }

    /// The pointer moved over image `id` with no button down.
    pub fn pointer_hovered(&mut self, id: DatasetId, p: Pointer) {
        if let Some((image, interaction)) = &mut self.interaction
            && *image == id
        {
            interaction.hover(p);
        }
    }

    /// The primary button came up.
    pub fn pointer_released(&mut self, id: DatasetId, p: Pointer) {
        let Some((image, interaction)) = &mut self.interaction else {
            return;
        };
        if *image != id {
            return;
        }
        if let Step::Done(commit) = interaction.release(p) {
            self.interaction = None;
            self.carry_out(id, commit, &p);
        }
    }

    /// Gives up the gesture under way. Returns false if there was none.
    pub fn cancel_gesture(&mut self) -> bool {
        self.interaction.take().is_some()
    }

    /// Runs the command a finished gesture asks for.
    fn carry_out(&mut self, id: DatasetId, commit: Option<Commit>, p: &Pointer) {
        let Some(commit) = commit else {
            return;
        };
        let input = || Args::new().with("input", id);
        let current = self.selection_of(id).cloned();
        match commit {
            Commit::Set(roi) => {
                if current.as_ref() != Some(&roi) {
                    let args = input().with("selection", roi.clone());
                    self.commit_selection(id, "select.set", args, Some(roi));
                }
            }
            Commit::Translate { dx, dy } => {
                if let Some(current) = current {
                    let args = input().with("x_offset", dx).with("y_offset", dy);
                    let moved = current.translated(dx, dy);
                    self.commit_selection(id, "select.translate", args, Some(moved));
                }
            }
            Commit::Clear => {
                if current.is_some() {
                    self.commit_selection(id, "select.none", input(), None);
                }
            }
            Commit::Click(at) => {
                let picked = (self.tool == Tool::Pointer)
                    .then(|| self.region_at(id, at, p.reach))
                    .flatten();
                match picked {
                    Some(index) => self.pick_region(id, index),
                    None => {
                        self.regions.selected.remove(&id);
                        if current.is_some() {
                            self.commit_selection(id, "select.none", input(), None);
                        }
                    }
                }
            }
        }
    }

    /// Starts the command that makes a gesture's result permanent, and
    /// keeps that result on show until the command has run.
    pub(super) fn commit_selection(
        &mut self,
        id: DatasetId,
        command: &str,
        args: Args,
        shown: Option<Roi>,
    ) {
        self.pending_selection = Some((id, shown));
        // If something else is running this says so and starts nothing.
        self.start(command, &args);
        let started = self
            .task
            .as_ref()
            .is_some_and(|task| task.spec.id == command);
        if !started {
            self.pending_selection = None;
        }
    }

    /// Handles a key for the active image. Returns false if the key has
    /// nothing to act on here, and so is free to be a shortcut.
    pub fn canvas_key(&mut self, key: CanvasKey) -> bool {
        let Some(id) = self.session.workspace.active() else {
            return false;
        };
        match key {
            CanvasKey::Escape => {
                let cancelled = self.cancel_gesture();
                if cancelled {
                    self.status = Status::Done("Cancelled".to_string());
                }
                cancelled
            }
            CanvasKey::Enter => {
                let Some((image, interaction)) = &mut self.interaction else {
                    return false;
                };
                let image = *image;
                if let Step::Done(commit) = interaction.finish() {
                    self.interaction = None;
                    let p = Pointer::new([0.0, 0.0], 1.0);
                    self.carry_out(image, commit, &p);
                }
                true
            }
            CanvasKey::Backspace if self.drawing_path() => {
                if let Some((_, interaction)) = &mut self.interaction
                    && !interaction.undo_vertex()
                {
                    self.interaction = None;
                }
                true
            }
            CanvasKey::Backspace | CanvasKey::Delete => {
                if self.gesture_active() || self.selected_regions(id).is_empty() {
                    return false;
                }
                self.delete_selected_regions();
                true
            }
            CanvasKey::Arrow { dx, dy, resize } => {
                if self.gesture_active() {
                    // Not while the selection is in the hand.
                    return true;
                }
                let Some(roi) = self.selection_of(id).cloned() else {
                    return false;
                };
                let (dx, dy) = (f64::from(dx), f64::from(dy));
                let input = Args::new().with("input", id);
                if resize {
                    // ImageJ: Alt and an arrow resize a rectangle or oval.
                    if let Some(resized) = tools::nudged_size(&roi, dx, dy) {
                        self.start_or_queue("select.set", input.with("selection", resized));
                    }
                } else {
                    let args = input.with("x_offset", dx).with("y_offset", dy);
                    self.start_or_queue("select.translate", args);
                }
                true
            }
        }
    }

    /// Starts a command, or lines it up behind the one that is running:
    /// keys come faster than commands finish.
    fn start_or_queue(&mut self, command: &str, args: Args) {
        if self.task.is_some() {
            self.pending.push_back((command.to_string(), args));
        } else {
            self.start(command, &args);
        }
    }

    /// What the status bar says about the selection of the active image,
    /// or about the shape a gesture is making of it.
    pub fn selection_summary(&self) -> Option<String> {
        let dataset = self.active()?;
        let meta = dataset.image.meta();
        if let Some((image, interaction)) = &self.interaction
            && *image == dataset.id
        {
            if let Some(roi) = interaction.measured() {
                return Some(describe(&roi, meta).summary);
            }
            if interaction.is_path() {
                return Some("Click the next point".to_string());
            }
        }
        let view = self.canvas_view(dataset.id);
        view.selection.map(|roi| describe(&roi, meta).summary)
    }
}

/// A selection in words and numbers.
#[derive(Clone, Debug, PartialEq)]
pub struct SelectionInfo {
    /// One line for the status bar.
    pub summary: String,
    /// Name and value rows for the Info panel.
    pub rows: Vec<(&'static str, String)>,
}

fn number(value: f64) -> String {
    if !value.is_finite() {
        return "–".to_string();
    }
    let text = format!("{value:.2}");
    let text = text.trim_end_matches('0').trim_end_matches('.');
    if text == "-0" { "0" } else { text }.to_string()
}

/// What kind of selection this is, in the words the tools use.
pub fn kind_title(roi: &Roi) -> &'static str {
    match roi {
        Roi::Rectangle { .. } => "Rectangle",
        Roi::RoundedRectangle { .. } => "Rounded rectangle",
        Roi::Oval { .. } => "Oval",
        Roi::Polygon { .. } => "Polygon",
        Roi::Freehand { .. } => "Freehand",
        Roi::Traced { .. } => "Traced outline",
        Roi::Line { .. } => "Line",
        Roi::Polyline { .. } => "Segmented line",
        Roi::Freeline { .. } => "Freehand line",
        Roi::Angle { .. } => "Angle",
        Roi::Points { points } if points.len() == 1 => "Point",
        Roi::Points { .. } => "Points",
        Roi::Ellipse { .. } => "Ellipse",
        Roi::RotatedRectangle { .. } => "Rotated rectangle",
        Roi::Composite { .. } => "Composite",
        _ => "Selection",
    }
}

/// Describes a selection the way ImageJ's status bar does while one is
/// drawn: where it is and how large, the length and angle of a line, the
/// number of points; sizes also in the image's units if it has any.
pub fn describe(roi: &Roi, meta: &ImageMeta) -> SelectionInfo {
    let axes = meta.axes();
    let (x_axis, y_axis) = (&axes[0], &axes[1]);
    let unit = match (&x_axis.unit, &y_axis.unit) {
        (Some(x), Some(y)) if x == y => Some(x.as_str()),
        _ => None,
    };
    let kind = kind_title(roi);
    let (x, y, width, height) = roi.bounds();
    let mut rows = vec![("Selection", kind.to_string())];
    let length = |scaled: bool| {
        if scaled {
            roi.length_scaled(x_axis.scale, y_axis.scale)
        } else {
            roi.length()
        }
    };
    let length_text = || match unit {
        Some(unit) => format!(
            "{} {unit} ({} pixels)",
            number(length(true)),
            number(length(false))
        ),
        None => format!("{} pixels", number(length(false))),
    };
    let summary = match roi {
        Roi::Points { points } => {
            rows.push(("Points", points.len().to_string()));
            match points.as_slice() {
                [(px, py)] => {
                    rows.push(("Position", format!("{}, {}", number(*px), number(*py))));
                    format!("Point at {}, {}", number(*px), number(*py))
                }
                many => format!("{} points", many.len()),
            }
        }
        Roi::Line { x1, y1, x2, y2, .. } => {
            let angle = roi.angle().unwrap_or(0.0);
            rows.push((
                "From, to",
                format!(
                    "{}, {} to {}, {}",
                    number(*x1),
                    number(*y1),
                    number(*x2),
                    number(*y2)
                ),
            ));
            rows.push(("Length", length_text()));
            rows.push(("Angle", format!("{}°", number(angle))));
            format!("Line   length {}   angle {}°", length_text(), number(angle))
        }
        Roi::Angle { .. } => {
            let angle = roi.angle().unwrap_or(0.0);
            rows.push(("Angle", format!("{}°", number(angle))));
            format!("Angle {}°", number(angle))
        }
        Roi::Polyline { points, .. } | Roi::Freeline { points, .. } => {
            rows.push(("Points", points.len().to_string()));
            rows.push(("Length", length_text()));
            format!(
                "{kind}   {} points   length {}",
                points.len(),
                length_text()
            )
        }
        _ => {
            let place = format!("{}, {}", number(x), number(y));
            let size = format!("{} × {}", number(width), number(height));
            rows.push(("Position", place.clone()));
            let mut summary = format!("{kind} {size} at {place}");
            let mut size_row = format!("{size} pixels");
            if let Some(unit) = unit {
                let scaled = format!(
                    "{} × {} {unit}",
                    number(width * x_axis.scale),
                    number(height * y_axis.scale)
                );
                summary.push_str(&format!("   ({scaled})"));
                size_row = format!("{scaled} ({size} pixels)");
            }
            rows.push(("Size", size_row));
            if let Some(points) = roi.vertices() {
                rows.push(("Vertices", points.len().to_string()));
            }
            summary
        }
    };
    if roi.line_width() > 1.0 {
        rows.push(("Line width", format!("{} pixels", number(roi.line_width()))));
    }
    SelectionInfo { summary, rows }
}

#[cfg(test)]
mod tests {
    use super::*;
    use calipix_core::{Axis, AxisKind, DType};

    fn plain() -> ImageMeta {
        ImageMeta::plane(DType::U8, 100, 80).unwrap()
    }

    fn calibrated() -> ImageMeta {
        ImageMeta::new(
            DType::U8,
            vec![
                (Axis::calibrated(AxisKind::X, 0.5, "µm"), 100),
                (Axis::calibrated(AxisKind::Y, 0.5, "µm"), 80),
            ],
        )
        .unwrap()
    }

    #[test]
    fn selections_are_described_as_the_status_bar_words_them() {
        let summary = |roi: &Roi, meta: &ImageMeta| describe(roi, meta).summary;
        let rect = Roi::rectangle(10.0, 20.0, 30.0, 40.5);
        assert_eq!(summary(&rect, &plain()), "Rectangle 30 × 40.5 at 10, 20");
        assert_eq!(
            summary(&rect, &calibrated()),
            "Rectangle 30 × 40.5 at 10, 20   (15 × 20.25 µm)"
        );
        let line = Roi::line(10.0, 10.0, 40.0, 50.0);
        assert_eq!(
            summary(&line, &plain()),
            "Line   length 50 pixels   angle -53.13°"
        );
        assert_eq!(
            summary(&line, &calibrated()),
            "Line   length 25 µm (50 pixels)   angle -53.13°"
        );
        assert_eq!(summary(&Roi::point(3.0, 4.5), &plain()), "Point at 3, 4.5");
        let points = Roi::points(vec![(1.0, 1.0), (2.0, 2.0), (3.0, 3.0)]);
        assert_eq!(summary(&points, &plain()), "3 points");
        let polyline = Roi::polyline(vec![(0.0, 0.0), (3.0, 4.0), (3.0, 10.0)]);
        assert_eq!(
            summary(&polyline, &plain()),
            "Segmented line   3 points   length 11 pixels"
        );
        let polygon = Roi::polygon(vec![(10.0, 10.0), (50.0, 10.0), (10.0, 40.0)]);
        assert_eq!(summary(&polygon, &plain()), "Polygon 40 × 30 at 10, 10");
        let info = describe(&polygon, &plain());
        assert!(info.rows.contains(&("Vertices", "3".to_string())));
        assert!(info.rows.contains(&("Size", "40 × 30 pixels".to_string())));
    }
}
