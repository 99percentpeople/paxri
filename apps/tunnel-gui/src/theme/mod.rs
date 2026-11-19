//! Appearance is a desktop preference shared by every workspace window in this process.
mod palette;
pub use palette::{Palette, TerminalPalette};

use anyhow::{Context as _, Result};
use gpui::{App, Global, Pixels, Window, WindowAppearance, px};
use std::{
    fs,
    io::{ErrorKind, Write},
    path::{Path, PathBuf},
};
use tempfile::NamedTempFile;

pub const SPACE_1: Pixels = px(4.);
pub const SPACE_2: Pixels = px(8.);
pub const SPACE_3: Pixels = px(12.);
pub const SPACE_4: Pixels = px(16.);
pub const BUTTON_HEIGHT: Pixels = px(32.);
pub const CONTROL_RADIUS: Pixels = px(6.);
pub const PANEL_RADIUS: Pixels = px(8.);

#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub enum Appearance {
    #[default]
    System,
    Light,
    Dark,
}

impl Global for Appearance {}

impl Appearance {
    pub const ALL: [Self; 3] = [Self::System, Self::Light, Self::Dark];

    pub fn label(self) -> &'static str {
        match self {
            Self::System => "System",
            Self::Light => "Light",
            Self::Dark => "Dark",
        }
    }

    fn path() -> Result<PathBuf> {
        Ok(dirs::config_dir()
            .context("cannot find the user configuration directory")?
            .join("p2p-tunnel")
            .join("appearance"))
    }

    fn load(path: &Path) -> Result<Self> {
        let text = match fs::read_to_string(path) {
            Ok(text) => text,
            Err(e) if e.kind() == ErrorKind::NotFound => return Ok(Self::System),
            Err(e) => return Err(e.into()),
        };
        Self::ALL
            .into_iter()
            .find(|mode| mode.label().eq_ignore_ascii_case(text.trim()))
            .context("unrecognized appearance preference")
    }

    fn save(self, path: &Path) -> Result<()> {
        let dir = path.parent().context("missing preference directory")?;
        fs::create_dir_all(dir)?;
        let mut file = NamedTempFile::new_in(dir)?;
        writeln!(file, "{}", self.label())?;
        file.persist(path)?;
        Ok(())
    }

    pub fn init(cx: &mut App) {
        let mode = Self::path()
            .and_then(|path| Self::load(&path))
            .unwrap_or_else(|err| {
                eprintln!("Load appearance: {err}");
                Self::System
            });
        cx.set_global(mode);
    }

    pub fn apply(self, cx: &mut App) -> Result<()> {
        if *cx.global::<Self>() != self {
            self.save(&Self::path()?)?;
            cx.set_global(self);
            cx.refresh_windows();
        }
        Ok(())
    }

    pub(crate) fn resolve(self, system: WindowAppearance) -> &'static Palette {
        match (self, system) {
            (Self::Dark, _)
            | (Self::System, WindowAppearance::Dark | WindowAppearance::VibrantDark) => {
                &palette::DARK
            }
            _ => &palette::LIGHT,
        }
    }
}

pub fn palette(window: &Window, cx: &App) -> &'static Palette {
    cx.global::<Appearance>().resolve(window.appearance())
}

pub fn observe(window: &Window) {
    window
        .observe_window_appearance(|window, cx| {
            if *cx.global::<Appearance>() == Appearance::System {
                window.refresh();
            }
        })
        .detach();
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ptr;

    #[test]
    fn appearance_defaults_to_system_and_round_trips_each_choice() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("p2p-tunnel/appearance");
        assert_eq!(Appearance::load(&file).unwrap(), Appearance::System);
        for mode in [Appearance::Dark, Appearance::Light, Appearance::System] {
            mode.save(&file).unwrap();
            assert_eq!(Appearance::load(&file).unwrap(), mode);
        }
        fs::write(&file, "unrecognized").unwrap();
        assert!(Appearance::load(&file).is_err());
    }

    #[test]
    fn explicit_and_system_modes_resolve_both_native_appearances() {
        use WindowAppearance::*;
        for (system, palette) in [
            (Light, &palette::LIGHT),
            (VibrantLight, &palette::LIGHT),
            (Dark, &palette::DARK),
            (VibrantDark, &palette::DARK),
        ] {
            assert!(ptr::eq(Appearance::System.resolve(system), palette));
            assert!(ptr::eq(Appearance::Dark.resolve(system), &palette::DARK));
            assert!(ptr::eq(Appearance::Light.resolve(system), &palette::LIGHT));
        }
    }
}
