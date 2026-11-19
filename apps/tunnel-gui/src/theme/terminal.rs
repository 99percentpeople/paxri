//! Terminal preferences belong to the desktop user and are shared by its windows.
use super::{Palette, TerminalPalette, palette, preference_path, save_preference};
use anyhow::Result;
use gpui::{App, Global};
use serde::{Deserialize, Serialize};
use std::{fs, io::ErrorKind, path::Path};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TerminalScheme {
    #[default]
    FollowApp,
    Light,
    Dark,
    Midnight,
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

    pub fn palette(self, app: &Palette) -> &TerminalPalette {
        match self {
            Self::FollowApp => &app.terminal,
            Self::Light => &palette::LIGHT.terminal,
            Self::Dark => &palette::DARK.terminal,
            Self::Midnight => &palette::MIDNIGHT,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CursorShape {
    #[default]
    Underline,
    Bar,
    Block,
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

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct TerminalAppearance {
    pub scheme: TerminalScheme,
    pub cursor_shape: CursorShape,
    pub blink_cursor: bool,
}

impl Global for TerminalAppearance {}

impl TerminalAppearance {
    fn load(path: &Path) -> Result<Self> {
        match fs::read_to_string(path) {
            Ok(text) => Ok(toml::from_str(&text)?),
            Err(e) if e.kind() == ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e.into()),
        }
    }

    fn save(self, path: &Path) -> Result<()> {
        save_preference(path, &toml::to_string_pretty(&self)?)
    }

    pub fn init(cx: &mut App) {
        let style = preference_path("terminal.toml")
            .and_then(|p| Self::load(&p))
            .unwrap_or_else(|err| {
                eprintln!("Load terminal appearance: {err}");
                Self::default()
            });
        cx.set_global(style);
    }

    pub fn apply(self, cx: &mut App) -> Result<()> {
        if self != *cx.global::<Self>() {
            self.save(&preference_path("terminal.toml")?)?;
            cx.set_global(self);
            cx.refresh_windows();
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_and_partial_preferences_keep_the_existing_look() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("terminal.toml");
        let default = TerminalAppearance::load(&path).unwrap();
        assert_eq!(default.cursor_shape, CursorShape::Underline);
        assert!(!default.blink_cursor);
        for app in [&palette::LIGHT, &palette::DARK] {
            let terminal = default.scheme.palette(app);
            assert_eq!(terminal.surface.background, app.terminal.surface.background);
            assert_eq!(terminal.ansi, app.terminal.ansi);
        }
        fs::write(&path, "cursor_shape = 'bar'\n").unwrap();
        assert_eq!(
            TerminalAppearance::load(&path).unwrap(),
            TerminalAppearance {
                cursor_shape: CursorShape::Bar,
                ..default
            }
        );
    }

    #[test]
    fn each_preset_round_trips_and_keeps_its_palette_in_both_app_themes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("gui/terminal.toml");
        for (scheme, expected) in [
            (TerminalScheme::Light, &palette::LIGHT.terminal),
            (TerminalScheme::Dark, &palette::DARK.terminal),
            (TerminalScheme::Midnight, &palette::MIDNIGHT),
        ] {
            let style = TerminalAppearance {
                scheme,
                cursor_shape: CursorShape::Block,
                blink_cursor: true,
            };
            style.save(&path).unwrap();
            let saved = TerminalAppearance::load(&path).unwrap();
            assert_eq!(saved, style);
            for app in [&palette::LIGHT, &palette::DARK] {
                let resolved = saved.scheme.palette(app);
                assert_eq!(resolved.surface.background, expected.surface.background);
                assert_eq!(resolved.surface.foreground, expected.surface.foreground);
                assert_eq!(resolved.cursor, expected.cursor);
                assert_eq!(resolved.ansi, expected.ansi);
            }
        }
    }

    #[test]
    fn older_preferences_use_the_saved_scheme_and_cursor() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("terminal.toml");
        fs::write(
            &path,
            r##"scheme = "midnight"
cursor_shape = "bar"
blink_cursor = true
[colors]
background = "#FF00FF"
cursor = "#000000"
"##,
        )
        .unwrap();
        let style = TerminalAppearance::load(&path).unwrap();
        assert_eq!(style.scheme, TerminalScheme::Midnight);
        assert_eq!(style.cursor_shape, CursorShape::Bar);
        assert!(style.blink_cursor);
        let terminal = style.scheme.palette(&palette::LIGHT);
        assert_eq!(
            terminal.surface.background,
            palette::MIDNIGHT.surface.background
        );
        assert_eq!(terminal.cursor, palette::MIDNIGHT.cursor);
    }
}
