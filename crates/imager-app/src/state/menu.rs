//! The menu bar as the interface draws it: the registry's menu tree put in
//! the order and grouping `docs/ux.md` prescribes, with recent files added.

use std::path::{Path, PathBuf};

use imager_command::{Args, CommandSpec, MenuItem, ParamSpec, ParamType, Registry, menu_tree};

/// Top-level menus, left to right. Menus not named here (an extension's
/// own, say) go just before Window.
pub const MENU_ORDER: [&str; 9] = [
    "File",
    "Edit",
    "View",
    "Image",
    "Process",
    "Analyze",
    "Extensions",
    "Window",
    "Help",
];

/// The submenu of File that lists recently opened files.
pub const RECENT_MENU: &str = "Open Recent";

#[derive(Clone, Debug, PartialEq)]
pub enum MenuEntry {
    Separator,
    Submenu {
        title: String,
        items: Vec<MenuEntry>,
    },
    Command(MenuCommand),
}

/// A menu item that runs a command, possibly with some arguments given.
#[derive(Clone, Debug, PartialEq)]
pub struct MenuCommand {
    pub id: String,
    /// The text of the item, with an ellipsis if choosing it opens a dialog.
    pub label: String,
    pub args: Args,
    /// Extra hover text, such as the full path of a recent file.
    pub hint: Option<String>,
}

/// Parameters of display commands that say which plane is meant. The
/// view answers them: the plane on screen is the one meant.
pub const VIEW_PARAMS: [&str; 4] = ["channel", "slice", "frame", "press"];

/// The parameters a person has to see to run `spec`: all of them, except
/// that a command's only image parameter is filled with the active image
/// without asking, and a display command is pointed at the plane on
/// screen.
pub fn dialog_params(spec: &CommandSpec) -> Vec<&ParamSpec> {
    let images = spec
        .inputs
        .iter()
        .filter(|p| p.ty == ParamType::Image)
        .count();
    let display = imager_commands::view::DISPLAY_COMMANDS.contains(&spec.id.as_str());
    spec.inputs
        .iter()
        .filter(|p| !(p.ty == ParamType::Image && images == 1))
        .filter(|p| !(display && VIEW_PARAMS.contains(&p.name.as_str())))
        .collect()
}

/// True if choosing the command from a menu asks for something first,
/// whether in a parameter dialog or the system's file dialog.
pub fn opens_dialog(spec: &CommandSpec) -> bool {
    !dialog_params(spec).is_empty()
}

/// The command's title as shown on menu items and buttons.
pub fn label(spec: &CommandSpec) -> String {
    if opens_dialog(spec) {
        format!("{}…", spec.title)
    } else {
        spec.title.clone()
    }
}

/// Builds the menu bar.
pub fn build(registry: &Registry, recent: &[PathBuf]) -> Vec<MenuEntry> {
    let (mut menus, _) = convert(registry, menu_tree(registry));
    let rank = |entry: &MenuEntry| {
        let MenuEntry::Submenu { title, .. } = entry else {
            return MENU_ORDER.len();
        };
        let window = MENU_ORDER.len() - 2;
        match MENU_ORDER.iter().position(|name| name == title) {
            Some(index) if index >= window => index + 1,
            Some(index) => index,
            None => window,
        }
    };
    // Stable: menus of equal rank keep the registry's weight order.
    menus.retain(|entry| *entry != MenuEntry::Separator);
    menus.sort_by_key(rank);

    if let Some(items) = submenu_mut(&mut menus, "File")
        && let Some(items) = submenu_mut(items, RECENT_MENU)
        && !recent.is_empty()
    {
        let mut entries: Vec<MenuEntry> = recent.iter().map(|path| recent_entry(path)).collect();
        entries.push(MenuEntry::Separator);
        entries.append(items);
        *items = entries;
    }
    menus
}

fn submenu_mut<'a>(entries: &'a mut [MenuEntry], name: &str) -> Option<&'a mut Vec<MenuEntry>> {
    entries.iter_mut().find_map(|entry| match entry {
        MenuEntry::Submenu { title, items } if title == name => Some(items),
        _ => None,
    })
}

fn recent_entry(path: &Path) -> MenuEntry {
    let name = path.file_name().map_or_else(
        || path.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    );
    MenuEntry::Command(MenuCommand {
        id: "file.open".to_string(),
        label: name,
        args: Args::new().with("path", path.to_path_buf()),
        hint: Some(path.display().to_string()),
    })
}

/// Converts one level of the registry's tree, returning it with the lowest
/// weight found in it. A separator goes wherever two neighbours fall in
/// different hundreds of weight.
fn convert(registry: &Registry, items: Vec<MenuItem>) -> (Vec<MenuEntry>, f64) {
    let mut entries = Vec::new();
    let mut lowest = f64::INFINITY;
    let mut previous_group = None;
    for item in items {
        let (entry, weight) = match item {
            MenuItem::Submenu { title, items } => {
                let (items, weight) = convert(registry, items);
                (MenuEntry::Submenu { title, items }, weight)
            }
            MenuItem::Command { id, title } => {
                let spec = registry.get(&id).ok().map(|c| c.spec());
                let weight = spec
                    .and_then(|s| s.menu.as_ref())
                    .map_or(f64::INFINITY, |m| m.weight);
                let command = MenuCommand {
                    label: spec.map_or(title, label),
                    id,
                    args: Args::new(),
                    hint: None,
                };
                (MenuEntry::Command(command), weight)
            }
        };
        let group = (weight / 100.0).floor();
        if previous_group.is_some_and(|previous| previous != group) {
            entries.push(MenuEntry::Separator);
        }
        previous_group = Some(group);
        lowest = lowest.min(weight);
        entries.push(entry);
    }
    (entries, lowest)
}
