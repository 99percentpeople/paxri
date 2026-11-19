//! Desktop settings in the node's [gui] and [gui.terminal] tables.
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Appearance {
    #[default]
    System,
    Light,
    Dark,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TerminalScheme {
    #[default]
    FollowApp,
    Light,
    Dark,
    Midnight,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CursorShape {
    #[default]
    Underline,
    Bar,
    Block,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct TerminalAppearance {
    pub scheme: TerminalScheme,
    pub cursor_shape: CursorShape,
    pub blink_cursor: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct GuiSettings {
    pub appearance: Appearance,
    pub terminal: TerminalAppearance,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GuiChange {
    Appearance(Appearance),
    Scheme(TerminalScheme),
    CursorShape(CursorShape),
    BlinkCursor(bool),
}

impl Appearance {
    pub const ALL: [Self; 3] = [Self::System, Self::Light, Self::Dark];

    pub fn label(self) -> &'static str {
        match self {
            Self::System => "System",
            Self::Light => "Light",
            Self::Dark => "Dark",
        }
    }
}

impl TerminalScheme {
    pub const ALL: [Self; 4] = [Self::FollowApp, Self::Light, Self::Dark, Self::Midnight];

    pub fn label(self) -> &'static str {
        match self {
            Self::FollowApp => "Follow app theme",
            Self::Light => "Neutral light",
            Self::Dark => "Neutral dark",
            Self::Midnight => "Midnight",
        }
    }
}

impl CursorShape {
    pub const ALL: [Self; 3] = [Self::Underline, Self::Bar, Self::Block];

    pub fn label(self) -> &'static str {
        match self {
            Self::Underline => "Underline",
            Self::Bar => "Bar",
            Self::Block => "Block",
        }
    }
}
