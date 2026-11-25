//! Local terminal actions, independent of the GUI toolkit and the remote shell.
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, fmt, str::FromStr};

pub const CTRL: u8 = 1;
pub const ALT: u8 = 2;
pub const SHIFT: u8 = 4;
pub const META: u8 = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminalAction {
    Copy,
    Paste,
    ZoomIn,
    ZoomOut,
    ZoomReset,
}

impl TerminalAction {
    pub const ALL: [Self; 5] = [
        Self::Copy,
        Self::Paste,
        Self::ZoomIn,
        Self::ZoomOut,
        Self::ZoomReset,
    ];

    pub fn key(self) -> &'static str {
        match self {
            Self::Copy => "copy",
            Self::Paste => "paste",
            Self::ZoomIn => "zoom_in",
            Self::ZoomOut => "zoom_out",
            Self::ZoomReset => "zoom_reset",
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Self::Copy => "Copy",
            Self::Paste => "Paste",
            Self::ZoomIn => "Zoom in",
            Self::ZoomOut => "Zoom out",
            Self::ZoomReset => "Reset zoom",
        }
    }

    fn default_chord(self) -> (u8, &'static str) {
        let clipboard = if cfg!(target_os = "macos") {
            META
        } else {
            CTRL | SHIFT
        };
        let zoom = if cfg!(target_os = "macos") {
            META
        } else {
            CTRL
        };
        match self {
            Self::Copy => (clipboard, "C"),
            Self::Paste => (clipboard, "V"),
            Self::ZoomIn => (zoom, "+"),
            Self::ZoomOut => (zoom, "-"),
            Self::ZoomReset => (zoom, "0"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Shortcut {
    modifiers: u8,
    key: String,
}

fn normalize(mut modifiers: u8, key: &str) -> (u8, &str) {
    let key = match key {
        "+" | "=" | "add" => {
            modifiers &= !SHIFT;
            "+"
        }
        "subtract" => "-",
        "prior" => "pageup",
        "next" => "pagedown",
        "return" => "enter",
        other => other,
    };
    (modifiers, key)
}

impl FromStr for Shortcut {
    type Err = String;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let text = text.trim();
        if text.eq_ignore_ascii_case("off") {
            return Ok(Self::disabled());
        }
        let mut modifiers = 0;
        let mut key = text;
        while let Some((name, rest)) = key.split_once('+') {
            if name.is_empty() {
                break;
            }
            let bit = match name.trim().to_ascii_lowercase().as_str() {
                "ctrl" => CTRL,
                "alt" => ALT,
                "shift" => SHIFT,
                "cmd" | "meta" | "win" => META,
                _ => return Err(format!("Unknown modifier: {name}")),
            };
            if modifiers & bit != 0 {
                return Err(format!("Repeated modifier: {name}"));
            }
            modifiers |= bit;
            key = rest.trim();
        }
        Self::from_keys(modifiers, key)
    }
}

impl Shortcut {
    pub fn disabled() -> Self {
        Self {
            modifiers: 0,
            key: String::new(),
        }
    }

    pub fn from_keys(modifiers: u8, key: &str) -> Result<Self, String> {
        let raw = key.to_ascii_lowercase();
        let (modifiers, key) = normalize(modifiers, &raw);
        let function = (1..=12).any(|n| key == format!("f{n}"));
        let named = matches!(
            key,
            "up" | "down"
                | "left"
                | "right"
                | "home"
                | "end"
                | "pageup"
                | "pagedown"
                | "insert"
                | "delete"
                | "enter"
                | "space"
                | "tab"
                | "backspace"
                | "escape"
        );
        let printable = key.len() == 1 && key.as_bytes()[0].is_ascii_graphic();
        if !(function || named || printable) {
            return Err("Use a supported key, such as a letter, arrow or function key.".into());
        }
        if printable && modifiers & SHIFT != 0 && !key.as_bytes()[0].is_ascii_alphabetic() {
            return Err(
                "For a shifted symbol, enter the resulting character, such as Ctrl+!.".into(),
            );
        }
        if !function && modifiers & (CTRL | ALT | META) == 0 {
            return Err("Include Ctrl, Alt or Meta, or use a function key.".into());
        }
        Ok(Self {
            modifiers,
            key: key.to_ascii_uppercase(),
        })
    }
}

pub fn modifier_labels<'a>(modifiers: u8) -> impl Iterator<Item = &'a str> {
    [
        (CTRL, "Ctrl"),
        (ALT, "Alt"),
        (SHIFT, "Shift"),
        (
            META,
            if cfg!(target_os = "macos") {
                "Cmd"
            } else {
                "Meta"
            },
        ),
    ]
    .into_iter()
    .filter_map(move |(bit, label)| (modifiers & bit != 0).then_some(label))
}

impl Shortcut {
    pub fn keys(&self) -> impl Iterator<Item = &str> {
        modifier_labels(self.modifiers).chain((!self.key.is_empty()).then_some(self.key.as_str()))
    }
}

impl TryFrom<String> for Shortcut {
    type Error = String;
    fn try_from(text: String) -> Result<Self, Self::Error> {
        text.parse()
    }
}

impl From<Shortcut> for String {
    fn from(value: Shortcut) -> Self {
        value.to_string()
    }
}

impl fmt::Display for Shortcut {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.key.is_empty() {
            return f.write_str("Off");
        }
        for name in modifier_labels(self.modifiers) {
            write!(f, "{name}+")?;
        }
        f.write_str(&self.key)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct TerminalControls {
    pub remote_clipboard: bool,
    pub clickable_links: bool,
    pub terminal_bell: bool,
    pub shortcuts: BTreeMap<TerminalAction, Shortcut>,
    // Older profiles and gateway responses can still contain the retired switch.
    #[serde(rename = "copy_selection_on_ctrl_c", skip_serializing)]
    legacy_copy_switch: serde::de::IgnoredAny,
}

impl Default for TerminalControls {
    fn default() -> Self {
        Self {
            remote_clipboard: true,
            clickable_links: true,
            terminal_bell: true,
            shortcuts: BTreeMap::new(),
            legacy_copy_switch: serde::de::IgnoredAny,
        }
    }
}

impl TerminalControls {
    pub(crate) fn is_default(&self) -> bool {
        self == &Self::default()
    }

    pub fn rebind(
        &self,
        action: TerminalAction,
        binding: Option<Shortcut>,
    ) -> Result<Option<Shortcut>, String> {
        let mut edited = self.clone();
        edited.shortcuts.remove(&action);
        if let Some(chord) = binding
            && chord != edited.shortcut(action)
        {
            edited.shortcuts.insert(action, chord);
        }
        edited.validate()?;
        Ok(edited.shortcuts.remove(&action))
    }

    pub fn shortcut(&self, action: TerminalAction) -> Shortcut {
        self.shortcuts.get(&action).cloned().unwrap_or_else(|| {
            let (modifiers, key) = action.default_chord();
            Shortcut {
                modifiers,
                key: key.into(),
            }
        })
    }

    fn matches(&self, action: TerminalAction, modifiers: u8, key: &str) -> bool {
        if let Some(custom) = self.shortcuts.get(&action) {
            return !custom.key.is_empty()
                && modifiers == custom.modifiers
                && custom.key.eq_ignore_ascii_case(key);
        }
        let (default_mods, default_key) = action.default_chord();
        // macOS also accepts Ctrl for the traditional terminal zoom keys.
        let ctrl_zoom = cfg!(target_os = "macos")
            && default_mods == META
            && modifiers == CTRL
            && matches!(
                action,
                TerminalAction::ZoomIn | TerminalAction::ZoomOut | TerminalAction::ZoomReset
            );
        (modifiers == default_mods || ctrl_zoom) && default_key.eq_ignore_ascii_case(key)
    }

    pub fn resolve(&self, modifiers: u8, key: &str) -> Option<TerminalAction> {
        let (modifiers, key) = normalize(modifiers, key);
        TerminalAction::ALL
            .into_iter()
            .find(|action| self.matches(*action, modifiers, key))
    }

    pub fn validate(&self) -> Result<(), String> {
        for (&action, binding) in &self.shortcuts {
            if let Some(other) = TerminalAction::ALL.into_iter().find(|other| {
                *other != action
                    && !binding.key.is_empty()
                    && self.matches(*other, binding.modifiers, &binding.key)
            }) {
                return Err(format!(
                    "{binding} is already assigned to {}.",
                    other.title()
                ));
            }
        }
        Ok(())
    }
}
