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
#[serde(rename_all = "lowercase")]
pub enum TabPlacement {
    #[default]
    Top,
    Left,
}

impl TabPlacement {
    fn is_top(&self) -> bool {
        *self == Self::Top
    }
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

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct TerminalAppearance {
    pub scheme: TerminalScheme,
    pub cursor_shape: CursorShape,
    pub blink_cursor: bool,
    pub font_family: String,
    pub line_height: u8,
    pub letter_spacing: i8,
}

impl TerminalAppearance {
    pub const LINE_HEIGHTS: std::ops::RangeInclusive<u8> = 16..=32;
    pub const LETTER_SPACINGS: std::ops::RangeInclusive<i8> = -1..=4;

    pub(crate) fn validate(&self) -> Result<(), String> {
        if !Self::LINE_HEIGHTS.contains(&self.line_height) {
            return Err("gui.terminal.line_height must be between 16 and 32 px".into());
        }
        if !Self::LETTER_SPACINGS.contains(&self.letter_spacing) {
            return Err("gui.terminal.letter_spacing must be between -1 and 4 px".into());
        }
        if self.font_family.chars().any(char::is_control) {
            return Err("gui.terminal.font_family must be a single-line font name".into());
        }
        Ok(())
    }
}

impl Default for TerminalAppearance {
    fn default() -> Self {
        Self {
            scheme: TerminalScheme::default(),
            cursor_shape: CursorShape::default(),
            blink_cursor: false,
            font_family: String::new(),
            line_height: 18,
            letter_spacing: 0,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct GuiSettings {
    pub appearance: Appearance,
    #[serde(skip_serializing_if = "TabPlacement::is_top")]
    pub tab_placement: TabPlacement,
    pub terminal: TerminalAppearance,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GuiChange {
    Appearance(Appearance),
    TabPlacement(TabPlacement),
    Scheme(TerminalScheme),
    CursorShape(CursorShape),
    BlinkCursor(bool),
    FontFamily(String),
    LineHeight(u8),
    LetterSpacing(i8),
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
