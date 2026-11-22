//! The Android terminal keybar. Rows and the keys within them are ordered.
use serde::{Deserialize, Serialize};

pub const KEYS: &[&str] = &[
    "Esc",
    "Tab",
    "Ctrl",
    "Alt",
    "Shift",
    "Left",
    "Down",
    "Up",
    "Right",
    "Enter",
    "Keys",
    "Home",
    "End",
    "PageUp",
    "PageDown",
    "Insert",
    "Delete",
    "Backspace",
    "Space",
    "F1",
    "F2",
    "F3",
    "F4",
    "F5",
    "F6",
    "F7",
    "F8",
    "F9",
    "F10",
    "F11",
    "F12",
];

pub fn label(key: &str) -> &str {
    match key {
        "Left" => "←",
        "Down" => "↓",
        "Up" => "↑",
        "Right" => "→",
        "PageUp" => "PgUp",
        "PageDown" => "PgDn",
        "Backspace" => "Bksp",
        other => other,
    }
}

pub fn valid_key(key: &str) -> bool {
    KEYS.contains(&key)
        || (key.chars().count() == 1 && !key.chars().any(|c| c.is_control() || c.is_whitespace()))
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum KeyHaptics {
    Off,
    Light,
    #[default]
    Standard,
    Strong,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct KeybarSettings {
    pub rows: Vec<Vec<String>>,
    pub row_height: u8,
    pub haptics: KeyHaptics,
}

impl Default for KeybarSettings {
    fn default() -> Self {
        Self {
            rows: vec![
                ["Esc", "Tab", "Ctrl", "Alt", "Home", "Up", "End", "PageUp"]
                    .map(String::from)
                    .to_vec(),
                [
                    "Keys",
                    "Shift",
                    "Backspace",
                    "Enter",
                    "Left",
                    "Down",
                    "Right",
                    "PageDown",
                ]
                .map(String::from)
                .to_vec(),
            ],
            row_height: Self::DEFAULT_ROW_HEIGHT,
            haptics: KeyHaptics::default(),
        }
    }
}

impl KeybarSettings {
    pub const DEFAULT_ROW_HEIGHT: u8 = 40;
    pub const HEIGHTS: std::ops::RangeInclusive<u8> = 32..=56;

    pub fn validate(&self) -> Result<(), String> {
        if !Self::HEIGHTS.contains(&self.row_height) {
            return Err("Key height must be between 32 and 56 dp.".into());
        }
        for (i, row) in self.rows.iter().enumerate() {
            if row.is_empty() {
                return Err(format!("Add a key to row {} or remove the row.", i + 1));
            }
            if let Some(key) = row.iter().find(|key| !valid_key(key)) {
                return Err(format!("Unknown key {key:?} in row {}.", i + 1));
            }
        }
        Ok(())
    }
}
