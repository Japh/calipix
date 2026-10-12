//! The regions of the active image as the Regions panel and the viewer
//! show them: which are picked in the list, whether they are drawn on
//! the image and how they are labelled.
//!
//! The regions themselves belong to the image (`Dataset::overlay`):
//! commands change them and undo restores them. What is kept here is only
//! how they are being looked at.

use std::collections::{BTreeMap, BTreeSet};

use calipix_command::{Args, DatasetId};
use calipix_core::{AxisKind, ColorModel, ImageMeta};
use calipix_roi::{RoiEntry, RoiProps};

use super::tables::row_ranges;
use super::tools::{Hit, RegionLabels, hit};
use super::{AppState, Panel, RowClick, UiAction, view};

/// A region's name being typed in the list.
#[derive(Clone, Debug, PartialEq)]
pub struct Renaming {
    pub image: DatasetId,
    /// The region's place in the list, counted from 0.
    pub index: usize,
    pub text: String,
    /// The field has yet to be given the keyboard.
    pub fresh: bool,
}

#[derive(Debug, Default)]
pub struct Regions {
    /// The regions picked in the list of each image, counted from 0.
    pub(super) selected: BTreeMap<DatasetId, BTreeSet<usize>>,
    /// The region clicked last, which Shift+click extends from.
    pub(super) anchor: Option<usize>,
    /// Regions are not drawn on images.
    pub(super) hidden: bool,
    pub(super) labels: RegionLabels,
    /// True while the list is what the keyboard acts on.
    pub(super) attention: bool,
    /// Set when the list is drawn, cleared when that is asked about.
    pub(super) drawn: bool,
    /// A number per image that changes whenever its regions do.
    pub(super) versions: BTreeMap<DatasetId, u64>,
    pub(super) next_version: u64,
    pub(super) renaming: Option<Renaming>,
}

/// True if a region with these properties belongs on the plane at
/// `position` (one index per axis beyond X and Y) of an image.
///
/// As in ImageJ, a region either belongs to every plane, or names a
/// channel, slice and frame (each from 1, or 0 for any), or names one
/// position in the stack, counting channels fastest, then slices, then
/// frames. A channel is not held against a region while all channels are
/// shown together.
pub fn on_plane(props: &RoiProps, meta: &ImageMeta, position: &[usize]) -> bool {
    let rgb = meta.color == ColorModel::Rgb;
    let at = |kind: AxisKind| {
        meta.axis_index(&kind)
            .and_then(|axis| position.get(axis - 2).copied())
            .unwrap_or(0) as i64
            + 1
    };
    let (channel, slice, frame) = (
        if rgb { 1 } else { at(AxisKind::Channel) },
        at(AxisKind::Z),
        at(AxisKind::Time),
    );
    let together = view::shown_together(meta);
    let matches = |wanted: i32, shown: i64| wanted <= 0 || i64::from(wanted) == shown;
    if props.channel > 0 || props.slice > 0 || props.frame > 0 {
        return (together || matches(props.channel, channel))
            && matches(props.slice, slice)
            && matches(props.frame, frame);
    }
    if props.position <= 0 {
        return true;
    }
    let channels = if rgb {
        1
    } else {
        meta.size(&AxisKind::Channel) as i64
    };
    let slices = meta.size(&AxisKind::Z) as i64;
    let wanted = i64::from(props.position);
    if together {
        // Any channel of the right slice and frame will do.
        let plane = (frame - 1) * slices + (slice - 1);
        return (wanted - 1) / channels == plane;
    }
    wanted == ((frame - 1) * slices + (slice - 1)) * channels + channel
}

/// What a region is called in the list and on the image: its name, or
/// failing that its number.
pub fn region_title(entry: &RoiEntry, index: usize) -> String {
    match &entry.props.name {
        Some(name) if !name.trim().is_empty() => name.clone(),
        _ => format!("Region {}", index + 1),
    }
}

impl AppState {
    /// The regions of the active image.
    pub fn regions(&self) -> &[RoiEntry] {
        self.active().map_or(&[], |dataset| &dataset.overlay)
    }

    /// Whether regions are drawn on images.
    pub fn regions_shown(&self) -> bool {
        !self.regions.hidden
    }

    pub fn region_labels(&self) -> RegionLabels {
        self.regions.labels
    }

    /// A number that changes whenever the regions of image `id` do, so
    /// that what is worked out from them can be kept between frames.
    pub fn regions_version(&self, id: DatasetId) -> u64 {
        self.regions.versions.get(&id).copied().unwrap_or(0)
    }

    /// The regions of image `id` picked in the list, counted from 0.
    pub fn selected_regions(&self, id: DatasetId) -> &BTreeSet<usize> {
        static NONE: BTreeSet<usize> = BTreeSet::new();
        self.regions.selected.get(&id).unwrap_or(&NONE)
    }

    /// The topmost region of image `id` under a position on the plane on
    /// view, reaching `reach` pixels to catch a line or a point.
    pub fn region_at(&self, id: DatasetId, at: [f64; 2], reach: f64) -> Option<usize> {
        if self.regions.hidden {
            return None;
        }
        let dataset = self.session.workspace.get(id).ok()?;
        let meta = dataset.image.meta();
        let position = self.position(id);
        dataset
            .overlay
            .iter()
            .enumerate()
            .rev()
            .find(|(_, entry)| {
                on_plane(&entry.props, meta, &position)
                    && hit(&entry.roi, at, reach) != Hit::Outside
            })
            .map(|(index, _)| index)
    }

    /// Picks one region and makes it the selection, as a click on it in
    /// the list or on the image does.
    ///
    /// The selection belongs to the image and is restored by undo, so it
    /// is changed by a command (Select Region) like every other change to
    /// it. Nothing runs if the selection already has the region's shape.
    pub fn pick_region(&mut self, id: DatasetId, index: usize) {
        let Ok(dataset) = self.session.workspace.get(id) else {
            return;
        };
        let Some(entry) = dataset.overlay.get(index) else {
            return;
        };
        let same = dataset.roi.as_ref() == Some(&entry.roi);
        let roi = entry.roi.clone();
        self.regions.selected.insert(id, BTreeSet::from([index]));
        self.regions.anchor = Some(index);
        if !same {
            let args = Args::new().with("input", id).with("region", index + 1);
            self.commit_selection(id, "regions.select", args, Some(roi));
        }
    }

    /// Picks a region of the active image as a click on its row does.
    /// A plain click also makes it the selection; with Shift or Ctrl the
    /// click only changes which regions are picked.
    pub fn click_region(&mut self, index: usize, click: RowClick) {
        let Some(dataset) = self.active() else {
            return;
        };
        let (id, count) = (dataset.id, dataset.overlay.len());
        if index >= count {
            return;
        }
        self.set_regions_attention(true);
        let anchor = self.regions.anchor;
        let selected = self.regions.selected.entry(id).or_default();
        match (click.extend, click.toggle, anchor) {
            (true, _, Some(anchor)) if anchor < count => {
                if !click.toggle {
                    selected.clear();
                }
                selected.extend(anchor.min(index)..=anchor.max(index));
            }
            (_, true, _) => {
                if !selected.remove(&index) {
                    selected.insert(index);
                }
                self.regions.anchor = Some(index);
            }
            _ => self.pick_region(id, index),
        }
    }

    pub fn select_all_regions(&mut self) {
        if let Some(dataset) = self.active() {
            let (id, count) = (dataset.id, dataset.overlay.len());
            self.regions.selected.insert(id, (0..count).collect());
            self.regions.anchor = None;
        }
    }

    pub fn clear_region_selection(&mut self) {
        if let Some(id) = self.session.workspace.active() {
            self.regions.selected.remove(&id);
        }
        self.regions.anchor = None;
    }

    /// The picked regions of the active image as command arguments: the
    /// image, and `regions` if any are picked.
    pub fn region_args(&self) -> Args {
        let Some(id) = self.session.workspace.active() else {
            return Args::new();
        };
        let args = Args::new().with("input", id);
        let selected = self.selected_regions(id);
        if selected.is_empty() {
            args
        } else {
            args.with("regions", row_ranges(selected))
        }
    }

    /// Removes the picked regions from the active image, as a command.
    pub fn delete_selected_regions(&mut self) {
        let Some(id) = self.session.workspace.active() else {
            return;
        };
        if self.selected_regions(id).is_empty() {
            return self.note("Delete Regions: Click a region to select it first");
        }
        let args = self.region_args();
        self.start("regions.delete", &args);
    }

    /// Starts typing a new name for a region of the active image.
    pub fn begin_rename(&mut self, index: usize) {
        let Some(dataset) = self.active() else {
            return;
        };
        let Some(entry) = dataset.overlay.get(index) else {
            return;
        };
        self.regions.renaming = Some(Renaming {
            image: dataset.id,
            index,
            text: entry.props.name.clone().unwrap_or_default(),
            fresh: true,
        });
    }

    /// The name being typed, if one is.
    pub fn renaming(&mut self) -> Option<&mut Renaming> {
        let active = self.session.workspace.active();
        self.regions
            .renaming
            .as_mut()
            .filter(|renaming| Some(renaming.image) == active)
    }

    /// Ends typing a name: with `keep`, by running Rename Region if the
    /// name changed.
    pub fn end_rename(&mut self, keep: bool) {
        let Some(renaming) = self.regions.renaming.take() else {
            return;
        };
        if !keep {
            return;
        }
        let Ok(dataset) = self.session.workspace.get(renaming.image) else {
            return;
        };
        let Some(entry) = dataset.overlay.get(renaming.index) else {
            return;
        };
        let name = renaming.text.trim();
        if entry.props.name.as_deref().unwrap_or_default() == name {
            return;
        }
        let args = Args::new()
            .with("input", renaming.image)
            .with("region", renaming.index + 1)
            .with("name", name);
        self.start("regions.rename", &args);
    }

    /// True while the keyboard belongs to the list of regions: Delete
    /// removes the picked regions and Ctrl+A picks them all.
    pub fn regions_have_attention(&self) -> bool {
        self.regions.attention
    }

    /// Gives the keyboard to the list of regions, or takes it away. The
    /// Results table and the list never both have it.
    pub fn set_regions_attention(&mut self, attention: bool) {
        self.regions.attention = attention;
        if attention {
            self.tables.attention = false;
        }
    }

    /// Says that the list was drawn this frame; see
    /// [`regions_were_drawn`](Self::regions_were_drawn).
    pub fn note_regions_drawn(&mut self) {
        self.regions.drawn = true;
    }

    /// Whether the list has been drawn since the last call. If not, it no
    /// longer has the keyboard's attention.
    pub fn regions_were_drawn(&mut self) -> bool {
        let drawn = std::mem::take(&mut self.regions.drawn);
        if !drawn {
            self.regions.attention = false;
        }
        drawn
    }

    /// Brings what is kept about regions in line with the images now
    /// open, given the workspace as it was before a change.
    pub(super) fn note_region_changes(&mut self, before: &calipix_command::Workspace) {
        let workspace = &self.session.workspace;
        for dataset in workspace.datasets() {
            let unchanged = before
                .get(dataset.id)
                .is_ok_and(|old| old.overlay == dataset.overlay);
            if !unchanged || !self.regions.versions.contains_key(&dataset.id) {
                self.regions.next_version += 1;
                self.regions
                    .versions
                    .insert(dataset.id, self.regions.next_version);
            }
            if !unchanged && let Some(selected) = self.regions.selected.get_mut(&dataset.id) {
                // Numbers in a list of another length mean other regions.
                let same_length = before
                    .get(dataset.id)
                    .is_ok_and(|old| old.overlay.len() == dataset.overlay.len());
                if !same_length {
                    selected.clear();
                }
            }
        }
        let open = |id: &DatasetId| workspace.get(*id).is_ok();
        self.regions.versions.retain(|id, _| open(id));
        self.regions.selected.retain(|id, _| open(id));
        if self
            .regions
            .renaming
            .as_ref()
            .is_some_and(|renaming| !open(&renaming.image))
        {
            self.regions.renaming = None;
        }
    }

    /// What a finished region command leaves picked and on show.
    pub(super) fn note_region_command(&mut self, command: &str, input: Option<DatasetId>) {
        let Some(dataset) = input.and_then(|id| self.session.workspace.get(id).ok()) else {
            return;
        };
        let (id, count) = (dataset.id, dataset.overlay.len());
        match command {
            // The new region is the one to name or measure next.
            "regions.add" if count > 0 => {
                self.regions
                    .selected
                    .insert(id, BTreeSet::from([count - 1]));
                self.regions.anchor = Some(count - 1);
            }
            "regions.add" | "regions.open" => {}
            _ => return,
        }
        self.queue.push(UiAction::ShowPanel(Panel::Regions));
        self.apply_actions();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use calipix_core::DType;

    #[test]
    fn regions_belong_to_planes_as_in_imagej() {
        // Two channels, three slices, shown one channel at a time.
        let mut meta = ImageMeta::xyczt(DType::U8, 4, 4, 2, 3, 1).unwrap();
        meta.properties
            .insert("imagej.mode".to_string(), "color".to_string());
        let shown_apart = !view::shown_together(&meta);
        let props = |position, channel, slice| {
            let mut props = RoiProps::default();
            (props.position, props.channel, props.slice) = (position, channel, slice);
            props
        };
        // Axes beyond X and Y are channel then slice here.
        let at = |channel: usize, slice: usize| {
            let mut position = vec![0; meta.plane_shape().len()];
            let c = meta.axis_index(&AxisKind::Channel).unwrap() - 2;
            let z = meta.axis_index(&AxisKind::Z).unwrap() - 2;
            position[c] = channel;
            position[z] = slice;
            position
        };
        // No position: every plane.
        assert!(on_plane(&props(0, 0, 0), &meta, &at(1, 2)));
        // A slice, any channel.
        assert!(on_plane(&props(0, 0, 3), &meta, &at(1, 2)));
        assert!(!on_plane(&props(0, 0, 3), &meta, &at(1, 1)));
        if shown_apart {
            // A channel and a slice.
            assert!(on_plane(&props(0, 2, 3), &meta, &at(1, 2)));
            assert!(!on_plane(&props(0, 1, 3), &meta, &at(1, 2)));
            // A stack position: channel 2 of slice 3 is the sixth plane.
            assert!(on_plane(&props(6, 0, 0), &meta, &at(1, 2)));
            assert!(!on_plane(&props(5, 0, 0), &meta, &at(1, 2)));
        } else {
            // All channels together: either plane of the slice will do.
            assert!(on_plane(&props(5, 0, 0), &meta, &at(1, 2)));
            assert!(on_plane(&props(6, 0, 0), &meta, &at(0, 2)));
            assert!(!on_plane(&props(4, 0, 0), &meta, &at(1, 2)));
        }

        // A plain stack counts slices.
        let stack = ImageMeta::xyczt(DType::U8, 4, 4, 1, 5, 1).unwrap();
        let slice = |index: usize| {
            let mut position = vec![0; stack.plane_shape().len()];
            let z = stack.axis_index(&AxisKind::Z).unwrap() - 2;
            position[z] = index;
            position
        };
        assert!(on_plane(&props(3, 0, 0), &stack, &slice(2)));
        assert!(!on_plane(&props(3, 0, 0), &stack, &slice(0)));
    }

    #[test]
    fn regions_are_called_by_name_or_number() {
        let roi = calipix_roi::Roi::rectangle(0.0, 0.0, 1.0, 1.0);
        assert_eq!(region_title(&RoiEntry::new(roi.clone()), 2), "Region 3");
        assert_eq!(region_title(&RoiEntry::named(roi, "Nucleus"), 2), "Nucleus");
    }
}
