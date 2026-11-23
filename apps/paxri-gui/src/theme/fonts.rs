//! Local font faces and the character-cell dimensions shared by the terminal and its preview.
use gpui::App;
use paxri_ui::theme::fonts::TERMINAL_SYMBOL_FONT;
use std::{
    borrow::Cow,
    collections::{BTreeSet, HashSet},
};

const PLATFORM_FONT: &str = if cfg!(windows) {
    "Consolas"
} else if cfg!(target_os = "macos") {
    "Menlo"
} else {
    "DejaVu Sans Mono"
};
pub(crate) use paxri_ui::theme::fonts::{CellSize, Face, MonospaceFonts};

pub(super) fn init(cx: &mut App) {
    let text = cx.text_system().clone();
    if let Err(error) = text.add_fonts(vec![Cow::Borrowed(TERMINAL_SYMBOL_FONT)]) {
        eprintln!("Registering terminal symbol font: {error}");
    }
    let platform = Face::measure(PLATFORM_FONT.into(), &text);
    cx.set_global(MonospaceFonts {
        installed: Vec::new(),
        default: platform.clone(),
    });
    let scan = cx.background_executor().spawn(async move {
        let available: HashSet<_> = text.all_font_names().into_iter().collect();
        let mut db = fontdb::Database::new();
        db.load_system_fonts();
        let names: BTreeSet<_> = db
            .faces()
            .filter(|face| face.monospaced)
            .filter_map(|face| {
                face.families
                    .iter()
                    .map(|(name, _)| name)
                    .find(|name| available.contains(*name))
            })
            .collect();
        let mut installed: Vec<_> = names
            .into_iter()
            .map(|name| Face::measure(name.clone().into(), &text))
            .collect();
        installed.sort_by_cached_key(|face| face.font.family.to_lowercase());
        let default = installed
            .iter()
            .find(|face| face.font.family == PLATFORM_FONT)
            .or_else(|| installed.first())
            .cloned()
            .unwrap_or(platform);
        MonospaceFonts { installed, default }
    });
    cx.spawn(async move |cx| {
        let fonts = scan.await;
        let _ = cx.update(|cx| {
            cx.set_global(fonts);
            cx.refresh_windows();
        });
    })
    .detach();
}
