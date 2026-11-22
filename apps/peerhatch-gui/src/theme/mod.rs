pub use peerhatch_ui::theme::*;
pub(crate) mod fonts;
use gpui::App;
use std::path::{Path, PathBuf};

pub fn init(path: Option<&Path>, cx: &mut App) {
    let settings = path
        .and_then(|path| {
            peerhatch_config::file::read_gui(path)
                .inspect_err(|err| eprintln!("Load settings from {}: {err:#}", path.display()))
                .ok()
        })
        .unwrap_or_default();
    install(settings, cx);
    fonts::init(cx);
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
