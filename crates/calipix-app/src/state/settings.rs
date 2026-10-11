//! What the application remembers between sessions.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value as Json;

use super::Theme;

/// A saved workspace layout. `data` is opaque to everything but the layout
/// module that wrote it; `version` lets that module reject old formats.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedLayout {
    pub version: u32,
    pub data: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub theme: Theme,
    /// Recently opened files, most recent first.
    pub recent: Vec<PathBuf>,
    /// Last used dialog values: command id, then parameter name.
    pub remembered: BTreeMap<String, BTreeMap<String, Json>>,
    pub layout: Option<SavedLayout>,
    /// Folder the file dialog starts in.
    pub last_dir: Option<PathBuf>,
}

impl Settings {
    pub const MAX_RECENT: usize = 10;

    /// Reads settings from `path`. A missing file gives the defaults; an
    /// unreadable one gives the defaults and a message saying why.
    pub fn load(path: &Path) -> (Settings, Option<String>) {
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return (Settings::default(), None);
            }
            Err(e) => {
                let problem = format!("Could not read settings from {}: {e}", path.display());
                return (Settings::default(), Some(problem));
            }
        };
        match serde_json::from_str(&text) {
            Ok(settings) => (settings, None),
            Err(e) => (
                Settings::default(),
                Some(format!(
                    "Settings in {} were ignored because they could not be understood: {e}",
                    path.display()
                )),
            ),
        }
    }

    /// Writes settings to `path`, replacing the file in one step so that a
    /// crash cannot leave half a file behind.
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let text = serde_json::to_string_pretty(self).map_err(std::io::Error::other)?;
        let partial = path.with_extension("json.part");
        std::fs::write(&partial, text)?;
        std::fs::rename(&partial, path)
    }

    /// Puts `path` at the front of the recent files.
    pub fn add_recent(&mut self, path: &Path) {
        // Compare absolute paths so the same file is not listed twice.
        let path = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
        self.recent.retain(|p| p != &path);
        self.recent.insert(0, path);
        self.recent.truncate(Settings::MAX_RECENT);
    }
}

/// Where settings are kept: `CALIPIX_CONFIG_DIR` if set, otherwise the
/// platform's per-user configuration folder.
pub fn default_path() -> Option<PathBuf> {
    let var = |name: &str| {
        std::env::var_os(name)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
    };
    let dir = if let Some(dir) = var("CALIPIX_CONFIG_DIR") {
        dir
    } else if cfg!(windows) {
        var("APPDATA")?.join("Calipix")
    } else if cfg!(target_os = "macos") {
        var("HOME")?.join("Library/Application Support/Calipix")
    } else {
        var("XDG_CONFIG_HOME")
            .or_else(|| Some(var("HOME")?.join(".config")))?
            .join("calipix")
    };
    Some(dir.join("settings.json"))
}
