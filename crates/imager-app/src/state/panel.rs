//! The panels of the workspace other than image tabs.
//!
//! Adding a panel takes two steps: a variant here (with its title, key and
//! default position), and its contents in `gui::panels::show`. The Window
//! menu command, the default layout and layout persistence follow from the
//! variant.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Panel {
    Info,
    History,
    Log,
}

/// Where a panel sits in the default layout.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dock {
    /// Stacked top to bottom in the column right of the images.
    Right,
    /// Tabs in the strip under the images.
    Bottom,
}

impl Panel {
    /// Every panel, in Window menu order.
    pub const ALL: [Panel; 3] = [Panel::Info, Panel::History, Panel::Log];

    pub fn title(self) -> &'static str {
        match self {
            Panel::Info => "Info",
            Panel::History => "History",
            Panel::Log => "Log",
        }
    }

    /// Stable `snake_case` name, used in command ids.
    pub fn key(self) -> &'static str {
        match self {
            Panel::Info => "info",
            Panel::History => "history",
            Panel::Log => "log",
        }
    }

    pub fn dock(self) -> Dock {
        match self {
            Panel::Info => Dock::Right,
            Panel::History | Panel::Log => Dock::Bottom,
        }
    }

    /// The command that shows or hides this panel.
    pub fn command_id(self) -> String {
        format!("window.toggle_{}", self.key())
    }
}
