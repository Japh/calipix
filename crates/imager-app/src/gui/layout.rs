//! The dockable workspace: image tabs in the centre, panels around them.
//!
//! This is the only module that knows the docking crate (`egui_tiles`).
//! The rest of the application sees [`Pane`], [`Layout`] and [`PaneHost`];
//! the layout tree stores pane ids only, never their contents.

use eframe::egui;
use egui_tiles::{
    Behavior, Container, Linear, LinearDir, SimplificationOptions, Tile, TileId, Tiles, Tree,
    UiResponse,
};
use imager_command::DatasetId;
use serde::{Deserialize, Serialize};

use crate::state::{Dock, Panel, SavedLayout};

/// Something that occupies a tab.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Pane {
    /// Shown in the centre while no image is open. New image tabs open
    /// beside it, so it also marks where images live.
    Welcome,
    /// An open image, by dataset id.
    Image(u64),
    Panel(Panel),
}

/// Supplies what the layout shows in its tabs.
pub trait PaneHost {
    fn title(&mut self, pane: Pane) -> egui::WidgetText;
    fn show(&mut self, ui: &mut egui::Ui, pane: Pane);
}

/// What the person did to the tabs this frame.
#[derive(Debug, Default, PartialEq)]
pub struct LayoutEvents {
    /// An image tab was clicked.
    pub chosen_image: Option<DatasetId>,
    /// The close button of these image tabs was clicked.
    pub close_images: Vec<DatasetId>,
}

pub struct Layout {
    tree: Tree<Pane>,
}

struct TilesBehavior<'a> {
    host: &'a mut dyn PaneHost,
    events: LayoutEvents,
    /// Panel tabs closed this frame, hidden once the tree has been drawn.
    hide: Vec<TileId>,
}

impl Behavior<Pane> for TilesBehavior<'_> {
    fn pane_ui(&mut self, ui: &mut egui::Ui, _tile: TileId, pane: &mut Pane) -> UiResponse {
        ui.painter()
            .rect_filled(ui.max_rect(), 0.0, ui.visuals().panel_fill);
        self.host.show(ui, *pane);
        // Panes are moved by dragging their tab, not their contents.
        UiResponse::None
    }

    fn tab_title_for_pane(&mut self, pane: &Pane) -> egui::WidgetText {
        self.host.title(*pane)
    }

    fn is_tab_closable(&self, tiles: &Tiles<Pane>, tile: TileId) -> bool {
        matches!(tiles.get(tile), Some(Tile::Pane(pane)) if *pane != Pane::Welcome)
    }

    /// Nothing is removed here. A closed panel is hidden, keeping its place
    /// in the tree so that showing it again puts it back where it was. A
    /// closed image is reported, and its tab goes when the image does.
    fn on_tab_close(&mut self, tiles: &mut Tiles<Pane>, tile: TileId) -> bool {
        match tiles.get(tile) {
            Some(Tile::Pane(Pane::Image(id))) => self.events.close_images.push(DatasetId(*id)),
            // Hiding the tile while the tree is being drawn trips an
            // assertion in egui_tiles, so it waits until afterwards.
            Some(Tile::Pane(Pane::Panel(_))) => self.hide.push(tile),
            _ => {}
        }
        false
    }

    fn on_tab_button(
        &mut self,
        tiles: &mut Tiles<Pane>,
        tile: TileId,
        response: egui::Response,
    ) -> egui::Response {
        if response.clicked()
            && let Some(Tile::Pane(Pane::Image(id))) = tiles.get(tile)
        {
            self.events.chosen_image = Some(DatasetId(*id));
        }
        response
    }

    fn simplification_options(&self) -> SimplificationOptions {
        SimplificationOptions {
            // Every pane keeps its tab (title, close button, drag handle)
            // even when it is alone in its group.
            all_panes_must_have_tabs: true,
            ..SimplificationOptions::default()
        }
    }

    fn tab_bar_height(&self, _style: &egui::Style) -> f32 {
        26.0
    }

    fn gap_width(&self, _style: &egui::Style) -> f32 {
        2.0
    }
}

fn split(tiles: &mut Tiles<Pane>, dir: LinearDir, children: &[(TileId, f32)]) -> TileId {
    let mut linear = Linear::new(dir, children.iter().map(|(id, _)| *id).collect());
    for &(id, share) in children {
        linear.shares.set_share(id, share);
    }
    tiles.insert_container(linear)
}

impl Default for Layout {
    fn default() -> Layout {
        Layout::new()
    }
}

impl Layout {
    /// Bumped whenever a saved layout from an older version could not be
    /// used as it is.
    pub const VERSION: u32 = 1;

    /// The default arrangement: images in the centre, [`Dock::Right`]
    /// panels stacked in a column on the right, [`Dock::Bottom`] panels as
    /// tabs in a strip along the bottom.
    pub fn new() -> Layout {
        let mut tiles = Tiles::default();
        let welcome = tiles.insert_pane(Pane::Welcome);
        let images = tiles.insert_tab_tile(vec![welcome]);

        let docked = |tiles: &mut Tiles<Pane>, dock: Dock| -> Vec<TileId> {
            Panel::ALL
                .into_iter()
                .filter(|panel| panel.dock() == dock)
                .map(|panel| tiles.insert_pane(Pane::Panel(panel)))
                .collect()
        };
        let right: Vec<(TileId, f32)> = docked(&mut tiles, Dock::Right)
            .into_iter()
            .map(|pane| (tiles.insert_tab_tile(vec![pane]), 1.0))
            .collect();
        let bottom = docked(&mut tiles, Dock::Bottom);

        let mut top = images;
        if !right.is_empty() {
            let column = split(&mut tiles, LinearDir::Vertical, &right);
            top = split(
                &mut tiles,
                LinearDir::Horizontal,
                &[(images, 0.78), (column, 0.22)],
            );
        }
        let mut root = top;
        if !bottom.is_empty() {
            let strip = tiles.insert_tab_tile(bottom);
            root = split(
                &mut tiles,
                LinearDir::Vertical,
                &[(top, 0.76), (strip, 0.24)],
            );
        }
        Layout {
            tree: Tree::new("imager_workspace", root, tiles),
        }
    }

    /// Draws the workspace into `ui`.
    pub fn ui(&mut self, ui: &mut egui::Ui, host: &mut dyn PaneHost) -> LayoutEvents {
        let mut behavior = TilesBehavior {
            host,
            events: LayoutEvents::default(),
            hide: Vec::new(),
        };
        self.tree.ui(&mut behavior, ui);
        if !behavior.hide.is_empty() {
            for tile in behavior.hide {
                self.tree.tiles.set_visible(tile, false);
            }
            ui.ctx().request_repaint();
        }
        behavior.events
    }

    fn find(&self, pane: Pane) -> Option<TileId> {
        self.tree.tiles.find_pane(&pane)
    }

    fn image_tiles(&self) -> Vec<(TileId, u64)> {
        self.tree
            .tiles
            .iter()
            .filter_map(|(tile, content)| match content {
                Tile::Pane(Pane::Image(id)) => Some((*tile, *id)),
                _ => None,
            })
            .collect()
    }

    /// Adds `pane` as a tab next to `sibling`, or at the root of the
    /// layout if there is no such place.
    fn add_beside(&mut self, pane: Pane, sibling: Option<TileId>) -> TileId {
        let tiles = &mut self.tree.tiles;
        let tile = tiles.insert_pane(pane);
        let group = sibling.and_then(|sibling| tiles.parent_of(sibling));
        if let Some(Tile::Container(Container::Tabs(tabs))) = group.and_then(|g| tiles.get_mut(g)) {
            tabs.add_child(tile);
            return tile;
        }
        let tab = tiles.insert_tab_tile(vec![tile]);
        match self.tree.root.and_then(|root| tiles.get_mut(root)) {
            Some(Tile::Container(Container::Linear(linear))) => linear.add_child(tab),
            Some(Tile::Container(Container::Tabs(tabs))) => tabs.add_child(tile),
            _ => {
                let children = self.tree.root.into_iter().chain([tab]).collect();
                self.tree.root = Some(tiles.insert_horizontal_tile(children));
            }
        }
        tile
    }

    /// Makes the image tabs match the open images: a tab for each, opened
    /// beside the existing image tabs, and none for images that are gone.
    pub fn sync_images(&mut self, open: &[DatasetId]) {
        for (tile, id) in self.image_tiles() {
            if !open.contains(&DatasetId(id)) {
                self.tree.remove_recursively(tile);
            }
        }
        for id in open {
            if self.find(Pane::Image(id.0)).is_none() {
                let sibling = self
                    .image_tiles()
                    .first()
                    .map(|(tile, _)| *tile)
                    .or_else(|| self.find(Pane::Welcome));
                self.add_beside(Pane::Image(id.0), sibling);
                self.focus(Pane::Image(id.0));
            }
        }
        let welcome = self
            .find(Pane::Welcome)
            .unwrap_or_else(|| self.add_beside(Pane::Welcome, None));
        self.tree.tiles.set_visible(welcome, open.is_empty());
    }

    /// Brings a pane's tab to the front of its group.
    pub fn focus(&mut self, pane: Pane) {
        self.tree
            .make_active(|_, tile| matches!(tile, Tile::Pane(p) if *p == pane));
    }

    pub fn is_open(&self, panel: Panel) -> bool {
        self.find(Pane::Panel(panel))
            .is_some_and(|tile| self.tree.tiles.is_visible(tile))
    }

    /// Shows or hides a panel. A hidden panel keeps its place, so showing
    /// it again returns it to where it was.
    pub fn set_open(&mut self, panel: Panel, open: bool) {
        if self.is_open(panel) == open {
            return;
        }
        match self.find(Pane::Panel(panel)) {
            Some(tile) => self.tree.tiles.set_visible(tile, open),
            // Not in the tree at all, as in a layout saved before the
            // panel existed: put it beside a panel docked on the same side.
            None => {
                let sibling = Panel::ALL
                    .into_iter()
                    .filter(|other| other.dock() == panel.dock())
                    .find_map(|other| self.find(Pane::Panel(other)));
                self.add_beside(Pane::Panel(panel), sibling);
            }
        }
        if open {
            self.focus(Pane::Panel(panel));
        }
    }

    /// The layout as an opaque blob for the settings file. Image tabs are
    /// left out: images are not reopened with the next session.
    pub fn save(&self) -> Option<SavedLayout> {
        let mut tree = self.tree.clone();
        let images: Vec<TileId> = tree
            .tiles
            .iter()
            .filter(|(_, tile)| matches!(tile, Tile::Pane(Pane::Image(_))))
            .map(|(id, _)| *id)
            .collect();
        for tile in images {
            tree.remove_recursively(tile);
        }
        Some(SavedLayout {
            version: Layout::VERSION,
            data: serde_json::to_string(&tree).ok()?,
        })
    }

    /// Rebuilds a saved layout, or `None` if it is from another version or
    /// does not make sense, in which case the default is used.
    pub fn restore(saved: &SavedLayout) -> Option<Layout> {
        if saved.version != Layout::VERSION {
            return None;
        }
        let tree: Tree<Pane> = serde_json::from_str(&saved.data).ok()?;
        tree.root?;
        let mut layout = Layout { tree };
        layout.sync_images(&[]);
        // Panels added to the application since the layout was saved.
        for panel in Panel::ALL {
            if layout.find(Pane::Panel(panel)).is_none() {
                layout.set_open(panel, true);
            }
        }
        Some(layout)
    }
}
