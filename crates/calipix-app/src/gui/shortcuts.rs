//! Keyboard shortcuts. Which keys run which commands comes entirely from
//! the command specs, through [`AppState::shortcut_command`]; this module
//! only translates the toolkit's key events.

use eframe::egui;

use crate::state::shortcut::NAMED_KEYS;
use crate::state::{AppState, KeyPress};

/// The toolkit's key for a key written as in a shortcut declaration.
fn toolkit_key(name: &str) -> Option<egui::Key> {
    egui::Key::from_name(name)
}

/// The name a shortcut declaration uses for a key, if it has one.
fn key_name(key: egui::Key) -> Option<String> {
    let symbols = ('!'..='~')
        .filter(|c| !c.is_ascii_lowercase())
        .map(String::from);
    let named = NAMED_KEYS.iter().map(|name| name.to_string());
    let function = (1..=12).map(|n| format!("F{n}"));
    symbols
        .chain(named)
        .chain(function)
        .find(|name| toolkit_key(name) == Some(key))
}

/// Takes the key presses that are shortcuts out of this frame's input and
/// returns the commands they run. Does nothing while a text field has
/// focus, where keys are text.
pub fn pressed(ctx: &egui::Context, state: &AppState) -> Vec<String> {
    if ctx.text_edit_focused() {
        return Vec::new();
    }
    let mut commands = Vec::new();
    ctx.input_mut(|input| {
        input.events.retain(|event| {
            let egui::Event::Key {
                key,
                pressed: true,
                repeat: false,
                modifiers,
                ..
            } = event
            else {
                return true;
            };
            let Some(name) = key_name(*key) else {
                return true;
            };
            let press = KeyPress {
                // `command` is Ctrl on Windows and Linux, Cmd on macOS.
                ctrl: modifiers.command,
                shift: modifiers.shift,
                alt: modifiers.alt,
                key: name,
            };
            match state.shortcut_command(&press) {
                Some(id) => {
                    commands.push(id.to_string());
                    false
                }
                None => true,
            }
        });
    });
    commands
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_translate_both_ways() {
        for name in [
            "S", "0", "+", "-", "=", "F5", "Left", "Enter", "Escape", "PageUp", "Space",
        ] {
            let key = toolkit_key(name).unwrap_or_else(|| panic!("{name} has no key"));
            assert_eq!(key_name(key).as_deref(), Some(name));
        }
        assert_eq!(toolkit_key("Left"), Some(egui::Key::ArrowLeft));
    }

    /// Every key a spec may declare must exist in the toolkit, or its
    /// shortcut could never fire.
    #[test]
    fn every_declarable_key_exists() {
        let letters = ('A'..='Z').chain('0'..='9').map(String::from);
        let named = NAMED_KEYS.iter().map(|name| name.to_string());
        for name in letters
            .chain(named)
            .chain((1..=12).map(|n| format!("F{n}")))
        {
            assert!(toolkit_key(&name).is_some(), "{name}");
        }
    }
}
