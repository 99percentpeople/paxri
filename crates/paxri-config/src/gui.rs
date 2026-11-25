//! Desktop settings in the node's [gui] and [gui.terminal] tables.
use crate::keybar::KeybarSettings;
use crate::shortcuts::{Shortcut, TerminalAction, TerminalControls};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

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

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct TerminalAppearance {
    pub scheme: TerminalScheme,
    pub cursor_shape: CursorShape,
    pub blink_cursor: bool,
    pub font_family: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub font_size: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line_height: Option<u8>,
    pub letter_spacing: i8,
}

impl TerminalAppearance {
    pub const FONT_SIZES: std::ops::RangeInclusive<f32> = 8.0..=30.0;
    pub const DEFAULT_FONT_SIZE: f32 = if cfg!(target_os = "android") {
        12.
    } else {
        15.
    };

    pub fn size(&self) -> f32 {
        self.font_size.unwrap_or(Self::DEFAULT_FONT_SIZE)
    }

    pub fn row_height(&self, size: f32) -> f32 {
        self.line_height.map(f32::from).unwrap_or_else(|| {
            let ratio = if cfg!(target_os = "android") {
                4. / 3.
            } else {
                1.2
            };
            size * ratio
        })
    }

    pub const LINE_HEIGHTS: std::ops::RangeInclusive<u8> = 8..=40;
    pub const LETTER_SPACINGS: std::ops::RangeInclusive<i8> = -1..=4;

    pub(crate) fn validate(&self) -> Result<(), String> {
        if self
            .font_size
            .is_some_and(|v| !Self::FONT_SIZES.contains(&v))
        {
            return Err("gui.terminal.font_size must be between 8 and 30 px".into());
        }
        if self
            .line_height
            .is_some_and(|v| !Self::LINE_HEIGHTS.contains(&v))
        {
            return Err("gui.terminal.line_height must be between 8 and 40 px".into());
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
            font_size: None,
            line_height: None,
            letter_spacing: 0,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct GuiSettings {
    pub appearance: Appearance,
    #[serde(skip_serializing_if = "TabPlacement::is_top")]
    pub tab_placement: TabPlacement,
    pub terminal: TerminalAppearance,
    #[serde(skip_serializing_if = "TerminalControls::is_default")]
    pub controls: TerminalControls,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub keybar: Option<Arc<KeybarSettings>>,
}

impl GuiSettings {
    pub(crate) fn validate(&self) -> Result<(), String> {
        self.terminal.validate()?;
        self.controls.validate()?;
        if let Some(keys) = &self.keybar { keys.validate()?; }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GuiChange {
    Appearance(Appearance),
    TabPlacement(TabPlacement),
    Scheme(TerminalScheme),
    CursorShape(CursorShape),
    BlinkCursor(bool),
    FontFamily(String),
    FontSize(Option<f32>),
    LineHeight(Option<u8>),
    LetterSpacing(i8),
    Keybar(KeybarSettings),
    Shortcut(TerminalAction, Option<Shortcut>),
    RemoteClipboard(bool),
    ClickableLinks(bool),
    TerminalBell(bool),
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
