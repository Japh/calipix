//! Application commands: operations on the interface itself (undo, zoom,
//! panels, theme, palette). They are ordinary commands in the registry, so
//! menus, the palette, shortcuts and `calipix commands` list them like any
//! other. Running one queues a [`UiAction`], which the application applies
//! on its next frame.

use std::sync::{Arc, Mutex};

use calipix_command::{Args, Command, CommandCtx, CommandSpec, Flags, Outputs, Registry, Result};
use serde::{Deserialize, Serialize};

use super::Panel;

/// Which colours the interface uses.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Theme {
    /// Follow the operating system's light or dark setting.
    #[default]
    System,
    Light,
    Dark,
}

/// A change of magnification asked of the image viewer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Zoom {
    In,
    Out,
    /// Scale the image to fill its tab.
    Fit,
    /// One image pixel per screen pixel.
    Actual,
}

/// Something an application command asks the interface to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UiAction {
    Undo,
    Redo,
    Zoom(Zoom),
    /// Step the plane on screen forward (1) or back (-1) along the
    /// slider used last.
    Step(i8),
    TogglePanel(Panel),
    /// Bring a panel into view; queued by the application itself when a
    /// command writes its results there.
    ShowPanel(Panel),
    ResetLayout,
    /// Put each open image in a group of its own, next to the others.
    TileImages,
    SetTheme(Theme),
    OpenPalette,
    About,
    ClearRecent,
}

/// Actions waiting to be applied, shared between the registered commands
/// and the application.
#[derive(Clone, Debug, Default)]
pub struct ActionQueue(Arc<Mutex<Vec<UiAction>>>);

impl ActionQueue {
    pub fn push(&self, action: UiAction) {
        self.0.lock().expect("action queue poisoned").push(action);
    }

    pub fn take(&self) -> Vec<UiAction> {
        std::mem::take(&mut *self.0.lock().expect("action queue poisoned"))
    }
}

struct AppCommand {
    spec: CommandSpec,
    action: UiAction,
    queue: ActionQueue,
}

impl Command for AppCommand {
    fn spec(&self) -> &CommandSpec {
        &self.spec
    }

    fn run(&self, _ctx: &mut CommandCtx<'_>, _args: &Args) -> Result<Outputs> {
        self.queue.push(self.action);
        Ok(Outputs::new())
    }
}

/// Shortcuts beyond the one a spec can declare, as (command id, shortcut).
pub const EXTRA_SHORTCUTS: [(&str, &str); 1] = [("help.command_palette", "Ctrl+Shift+P")];

/// The application commands and what each one does.
fn definitions() -> Vec<(CommandSpec, UiAction)> {
    let spec = |id: &str, title: &str| CommandSpec::new(id, title);
    let mut all = vec![
        (
            spec("edit.undo", "Undo")
                .summary("Reverts the most recent change to an image.")
                .menu("Edit", 10.0)
                .shortcut("Ctrl+Z")
                .ijm_name("Undo"),
            UiAction::Undo,
        ),
        (
            spec("edit.redo", "Redo")
                .summary("Re-applies the most recently undone change.")
                .menu("Edit", 20.0)
                .shortcut("Ctrl+Shift+Z"),
            UiAction::Redo,
        ),
        (
            spec("view.zoom_in", "Zoom In")
                .summary("Magnifies the view of the active image.")
                .menu("View", 10.0)
                .shortcut("+")
                .ijm_name("In [+]"),
            UiAction::Zoom(Zoom::In),
        ),
        (
            spec("view.zoom_out", "Zoom Out")
                .summary("Shrinks the view of the active image.")
                .menu("View", 20.0)
                .shortcut("-")
                .ijm_name("Out [-]"),
            UiAction::Zoom(Zoom::Out),
        ),
        (
            spec("view.zoom_fit", "Fit to Window")
                .summary("Scales the view so the whole active image fills its tab.")
                .menu("View", 30.0)
                .shortcut("0")
                .ijm_name("Scale to Fit"),
            UiAction::Zoom(Zoom::Fit),
        ),
        (
            spec("view.zoom_actual", "Zoom to 100%")
                .summary("Shows the active image with one image pixel per screen pixel.")
                .menu("View", 40.0)
                .shortcut("1")
                .ijm_name("View 100%"),
            UiAction::Zoom(Zoom::Actual),
        ),
        (
            spec("view.previous_plane", "Previous Plane")
                .summary("Steps back one slice or frame along the slider used last.")
                .menu("View", 110.0)
                .shortcut("Left")
                .ijm_name("Previous Slice [<]"),
            UiAction::Step(-1),
        ),
        (
            spec("view.next_plane", "Next Plane")
                .summary("Steps forward one slice or frame along the slider used last.")
                .menu("View", 120.0)
                .shortcut("Right")
                .ijm_name("Next Slice [>]"),
            UiAction::Step(1),
        ),
        // The display commands of the standard library sit between these
        // and the theme, at weights 200 to 499.
        (
            spec("view.theme_system", "System Theme")
                .summary("Makes the interface follow the system's light or dark setting.")
                .menu("View > Theme", 910.0),
            UiAction::SetTheme(Theme::System),
        ),
        (
            spec("view.theme_light", "Light Theme")
                .summary("Switches the interface to light colours.")
                .menu("View > Theme", 920.0),
            UiAction::SetTheme(Theme::Light),
        ),
        (
            spec("view.theme_dark", "Dark Theme")
                .summary("Switches the interface to dark colours.")
                .menu("View > Theme", 930.0),
            UiAction::SetTheme(Theme::Dark),
        ),
        (
            spec("window.tile_images", "Show Images Side by Side")
                .summary("Arranges the open images next to each other, to compare them.")
                .menu("Window", 105.0)
                .ijm_name("Tile"),
            UiAction::TileImages,
        ),
        (
            spec("window.reset_layout", "Reset Layout")
                .summary("Puts every panel back in its default place and the images back in tabs.")
                .menu("Window", 110.0),
            UiAction::ResetLayout,
        ),
        (
            spec("file.clear_recent", "Clear Recent Files")
                .summary("Empties the list of recently opened files.")
                .menu("File > Open Recent", 15.0),
            UiAction::ClearRecent,
        ),
        (
            spec("help.command_palette", "Command Palette")
                .summary("Searches every command by name and runs it.")
                .menu("Help", 10.0)
                .shortcut("Ctrl+K"),
            UiAction::OpenPalette,
        ),
        (
            spec("help.about", "About Calipix")
                .summary("Shows the version of Calipix and its licence.")
                .menu("Help", 910.0),
            UiAction::About,
        ),
    ];
    for (index, panel) in Panel::ALL.into_iter().enumerate() {
        let mut spec = CommandSpec::new(panel.command_id(), format!("{} Panel", panel.title()))
            .summary(format!("Shows or hides the {} panel.", panel.title()))
            .menu("Window", 10.0 * (index + 1) as f64);
        if let Some(name) = panel.ijm_name() {
            spec = spec.ijm_name(name);
        }
        all.push((spec, UiAction::TogglePanel(panel)));
    }
    all
}

/// Adds the application commands to `registry`. Running one pushes its
/// action onto `queue`. Returns each command's id with its action.
pub fn register(registry: &mut Registry, queue: &ActionQueue) -> Result<Vec<(String, UiAction)>> {
    let mut table = Vec::new();
    for (spec, action) in definitions() {
        // The interface is the only thing these act on: they mean nothing
        // without a window and must not appear in recorded macros.
        let spec = spec.flags(Flags {
            headless_ok: false,
            recordable: false,
            ..Flags::default()
        });
        table.push((spec.id.clone(), action));
        registry.register(Arc::new(AppCommand {
            spec,
            action,
            queue: queue.clone(),
        }))?;
    }
    Ok(table)
}

/// Every command the application knows: the standard library plus the
/// application commands. Used where commands are listed but no interface
/// exists to act on them.
pub fn full_registry() -> Registry {
    let mut registry = calipix_commands::standard_registry();
    register(&mut registry, &ActionQueue::default())
        .expect("application commands are valid and unique");
    super::table_commands::register(&mut registry).expect("table commands are valid and unique");
    registry
}
