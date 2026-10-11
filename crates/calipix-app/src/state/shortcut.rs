//! Keyboard shortcuts as declared on commands, e.g. `Ctrl+Shift+S`.

/// A parsed shortcut. `Ctrl` stands for the platform's command modifier:
/// Ctrl on Windows and Linux, Cmd on macOS.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Shortcut {
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
    /// The key as written in the declaration: `S`, `0`, `+`, `F5`, `Left`.
    pub key: String,
}

/// A key the user pressed, in the same vocabulary as [`Shortcut`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeyPress {
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
    pub key: String,
}

/// Keys that are written out by name.
pub const NAMED_KEYS: [&str; 14] = [
    "Enter",
    "Escape",
    "Space",
    "Tab",
    "Delete",
    "Backspace",
    "Left",
    "Right",
    "Up",
    "Down",
    "Home",
    "End",
    "PageUp",
    "PageDown",
];

impl Shortcut {
    /// Parses zero or more of `Ctrl`, `Shift` and `Alt`, then one key,
    /// joined by `+`. The `+` key itself is written last, as in `Ctrl++`.
    pub fn parse(text: &str) -> Result<Shortcut, String> {
        let bad = || format!("`{text}` is not a shortcut like `Ctrl+Shift+S`");
        let (modifiers, key) = match text.strip_suffix('+') {
            Some(rest) if rest.is_empty() || rest.ends_with('+') => {
                (rest.strip_suffix('+').unwrap_or(rest), "+")
            }
            Some(_) => return Err(bad()),
            None => text.rsplit_once('+').unwrap_or(("", text)),
        };
        let mut shortcut = Shortcut {
            ctrl: false,
            shift: false,
            alt: false,
            key: key.to_string(),
        };
        for modifier in modifiers.split('+').filter(|_| !modifiers.is_empty()) {
            let flag = match modifier {
                "Ctrl" => &mut shortcut.ctrl,
                "Shift" => &mut shortcut.shift,
                "Alt" => &mut shortcut.alt,
                _ => return Err(bad()),
            };
            if std::mem::replace(flag, true) {
                return Err(bad());
            }
        }
        let function_key = key
            .strip_prefix('F')
            .and_then(|n| n.parse::<u8>().ok())
            .is_some_and(|n| (1..=12).contains(&n));
        let mut chars = key.chars();
        let single = matches!(
            (chars.next(), chars.next()),
            (Some(c), None)
                if c.is_ascii_uppercase() || c.is_ascii_digit() || c.is_ascii_punctuation()
        );
        if single || function_key || NAMED_KEYS.contains(&key) {
            Ok(shortcut)
        } else {
            Err(bad())
        }
    }

    /// The shortcut as shown in menus and the palette.
    pub fn display(&self, mac: bool) -> String {
        let mut text = String::new();
        if self.ctrl {
            text.push_str(if mac { "Cmd+" } else { "Ctrl+" });
        }
        if self.shift {
            text.push_str("Shift+");
        }
        if self.alt {
            text.push_str(if mac { "Option+" } else { "Alt+" });
        }
        text + &self.key
    }

    fn is_symbol(&self) -> bool {
        self.key.len() == 1 && self.key.chars().all(|c| c.is_ascii_punctuation())
    }

    /// Number of modifiers, used to try the most specific shortcut first.
    pub fn specificity(&self) -> usize {
        usize::from(self.ctrl) + usize::from(self.shift) + usize::from(self.alt)
    }

    /// True if `press` triggers this shortcut. Ctrl and Alt must match
    /// exactly. Shift must match too, except for symbol keys, where
    /// keyboard layouts differ on whether the symbol needs Shift (`+` is
    /// Shift+`=` on a US keyboard); there, `=` also counts as `+`.
    pub fn matches(&self, press: &KeyPress) -> bool {
        if self.ctrl != press.ctrl || self.alt != press.alt {
            return false;
        }
        if self.is_symbol() {
            let key = self.key == press.key || (self.key == "+" && press.key == "=");
            key && (press.shift || !self.shift)
        } else {
            self.key == press.key && self.shift == press.shift
        }
    }
}
