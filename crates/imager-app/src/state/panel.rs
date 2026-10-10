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
    Display,
    Histogram,
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
    pub const ALL: [Panel; 5] = [
        Panel::Info,
        Panel::Display,
        Panel::Histogram,
        Panel::History,
        Panel::Log,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Panel::Info => "Info",
            Panel::Display => "Display",
            Panel::Histogram => "Histogram",
            Panel::History => "History",
            Panel::Log => "Log",
        }
    }

    /// Stable `snake_case` name, used in command ids.
    pub fn key(self) -> &'static str {
        match self {
            Panel::Info => "info",
            Panel::Display => "display",
            Panel::Histogram => "histogram",
            Panel::History => "history",
            Panel::Log => "log",
        }
    }

    pub fn dock(self) -> Dock {
        match self {
            Panel::Info | Panel::Display | Panel::Histogram => Dock::Right,
            Panel::History | Panel::Log => Dock::Bottom,
        }
    }

    /// Share of its dock's length the panel gets in the default layout.
    pub fn share(self) -> f32 {
        match self {
            Panel::Display => 1.3,
            Panel::Histogram => 1.2,
            Panel::Info => 1.0,
            Panel::History | Panel::Log => 1.0,
        }
    }

    /// What ImageJ calls its nearest equivalent, as a search alias.
    pub fn ijm_name(self) -> Option<&'static str> {
        match self {
            // Its B&C window is where display ranges are adjusted.
            Panel::Display => Some("Brightness/Contrast..."),
            _ => None,
        }
    }

    /// The command that shows or hides this panel.
    pub fn command_id(self) -> String {
        format!("window.toggle_{}", self.key())
    }
}
