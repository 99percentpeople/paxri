//! Appearance shared by every workspace connected to this node.
mod palette;
pub use palette::{Palette, TerminalPalette};
pub use peerhatch_config::gui::{
    Appearance, CursorShape, GuiSettings, TerminalAppearance, TerminalScheme,
};

use gpui::{App, Global, Pixels, Window, WindowAppearance, px};
use std::path::{Path, PathBuf};

pub const SPACE_1: Pixels = px(4.);
pub const SPACE_2: Pixels = px(8.);
pub const SPACE_3: Pixels = px(12.);
pub const SPACE_4: Pixels = px(16.);
pub const BUTTON_HEIGHT: Pixels = px(32.);
pub const CONTROL_RADIUS: Pixels = px(6.);
pub const PANEL_RADIUS: Pixels = px(8.);

#[derive(Default)]
struct Preferences(GuiSettings);
impl Global for Preferences {}

pub fn current(cx: &App) -> GuiSettings {
    cx.global::<Preferences>().0
}

pub fn init(path: Option<&Path>, cx: &mut App) {
    let settings = path
        .and_then(|path| {
            peerhatch_config::file::read_gui(path)
                .inspect_err(|err| eprintln!("Load settings from {}: {err:#}", path.display()))
                .ok()
        })
        .unwrap_or_default();
    cx.set_global(Preferences(settings));
}

pub fn apply(settings: GuiSettings, cx: &mut App) {
    if current(cx) != settings {
        cx.set_global(Preferences(settings));
        cx.refresh_windows();
    }
}

// Offline startup uses the ordinary per-user node unless a custom IPC address
// selects a gateway whose configuration file has not been discovered yet.
pub fn offline_path(config: Option<PathBuf>, custom_ipc: bool) -> Option<PathBuf> {
    config.or_else(|| {
        if custom_ipc {
            return None;
        }
        peerhatch_config::file::user_config()
    })
}

pub(crate) fn resolve(mode: Appearance, system: WindowAppearance) -> &'static Palette {
    match (mode, system) {
        (Appearance::Dark, _)
        | (Appearance::System, WindowAppearance::Dark | WindowAppearance::VibrantDark) => {
            &palette::DARK
        }
        _ => &palette::LIGHT,
    }
}

pub(crate) fn terminal_palette(scheme: TerminalScheme, app: &Palette) -> &TerminalPalette {
    match scheme {
        TerminalScheme::FollowApp => &app.terminal,
        TerminalScheme::Light => &palette::LIGHT.terminal,
        TerminalScheme::Dark => &palette::DARK.terminal,
        TerminalScheme::Midnight => &palette::MIDNIGHT,
    }
}

pub fn palette(window: &Window, cx: &App) -> &'static Palette {
    resolve(current(cx).appearance, window.appearance())
}

pub fn observe(window: &Window) {
    window
        .observe_window_appearance(|window, cx| {
            if current(cx).appearance == Appearance::System {
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
    fn explicit_and_system_modes_resolve_both_native_appearances() {
        use WindowAppearance::*;
        for (system, palette) in [
            (Light, &palette::LIGHT),
            (VibrantLight, &palette::LIGHT),
            (Dark, &palette::DARK),
            (VibrantDark, &palette::DARK),
        ] {
            assert!(ptr::eq(resolve(Appearance::System, system), palette));
            assert!(ptr::eq(resolve(Appearance::Dark, system), &palette::DARK));
            assert!(ptr::eq(resolve(Appearance::Light, system), &palette::LIGHT));
        }
    }
}
