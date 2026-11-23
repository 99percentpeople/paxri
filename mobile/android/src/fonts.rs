use gpui::{App, FallbackFontClass, FontFallbacks, SharedString};
use ndk::font::{Font, FontMatcher};
use paxri_ui::theme::fonts::{Face, MonospaceFonts, TERMINAL_SYMBOL_FAMILY, TERMINAL_SYMBOL_FONT};
use std::{
    borrow::Cow,
    collections::{HashMap, HashSet},
    fs,
    path::PathBuf,
};
use ttf_parser::{Face as FontFile, name_id};

fn matched_family(matched: &Font, files: &mut HashMap<PathBuf, Vec<u8>>) -> Option<String> {
    let path = matched.path();
    if !files.contains_key(path) {
        files.insert(path.to_owned(), fs::read(path).ok()?);
    }
    let font = FontFile::parse(files.get(path)?, matched.collection_index() as u32).ok()?;
    for id in [name_id::TYPOGRAPHIC_FAMILY, name_id::FAMILY] {
        let names = || font.names().into_iter().filter(|n| n.name_id == id);
        if let Some(name) = names()
            .filter(|n| n.language_id == 0x409)
            .chain(names())
            .find_map(|n| n.to_string())
        {
            return Some(name);
        }
    }
    None
}

pub(super) fn init(cx: &mut App) -> SharedString {
    let text = cx.text_system();
    let mut matcher = FontMatcher::new();
    let mut system_files = HashMap::<PathBuf, Vec<u8>>::new();
    let ui = matcher.match_font(c"sans-serif", &[u16::from(b'M')], None);
    let ui_family = matched_family(&ui, &mut system_files);
    let mut system_families = Vec::new();
    let mut preferred = None;
    // Ask Android for its actual monospace and its fallback runs for the exact
    // console repertoire, including OEM fonts outside /system/fonts.
    let subset = FontFile::parse(TERMINAL_SYMBOL_FONT, 0).expect("invalid terminal subset");
    let mut console = vec![u16::from(b'M')];
    let cmap = subset
        .tables()
        .cmap
        .expect("missing terminal character map");
    let table = cmap
        .subtables
        .into_iter()
        .find(|t| t.is_unicode())
        .expect("missing Unicode cmap");
    table.codepoints(|cp| {
        if let Ok(unit) = u16::try_from(cp) {
            console.push(unit);
        }
    });
    let mut remaining = console.as_slice();
    while !remaining.is_empty() {
        let mut count = 0;
        let matched = matcher.match_font(c"monospace", remaining, Some(&mut count));
        if let Some(name) = matched_family(&matched, &mut system_files) {
            if remaining.len() == console.len() {
                preferred = Some(name.clone());
            }
            if !system_families.contains(&name) {
                system_families.push(name);
            }
        }
        remaining = &remaining[(count as usize).clamp(1, remaining.len())..];
    }

    // Keep the existing font picker for other installed monospace faces.
    for entry in fs::read_dir("/system/fonts")
        .into_iter()
        .flatten()
        .flatten()
    {
        let path = entry.path();
        if entry
            .file_name()
            .to_string_lossy()
            .to_lowercase()
            .contains("mono")
            && !system_files.contains_key(&path)
            && let Ok(bytes) = fs::read(&path)
        {
            system_files.insert(path, bytes);
        }
    }
    let mut loaded: HashSet<_> = system_files.keys().cloned().collect();
    let mut fonts: Vec<_> = system_files.into_values().map(Cow::Owned).collect();
    fonts.push(Cow::Borrowed(TERMINAL_SYMBOL_FONT));
    if let Err(error) = text.add_fonts(fonts) {
        log::warn!("Registering terminal fonts: {error}");
    }
    let available = text.all_font_names();
    system_families.retain(|name| available.contains(name));
    let preferred = preferred.filter(|name| available.contains(name));
    let make_face = |name: &str| {
        let mut face = Face::measure(name.to_owned().into(), text);
        let fallbacks = system_families
            .iter()
            .map(String::as_str)
            .chain([TERMINAL_SYMBOL_FAMILY])
            .filter(|&family| family != name)
            .map(str::to_owned)
            .collect();
        face.font.fallbacks = Some(FontFallbacks::from_fonts(fallbacks));
        face
    };
    let installed: Vec<_> = available
        .iter()
        .filter(|name| {
            name.to_lowercase().contains("mono") || Some(*name) == preferred.as_ref()
        })
        .map(|name| make_face(name))
        .collect();
    let default = preferred
        .as_deref()
        .map(make_face)
        .or_else(|| installed.first().cloned())
        .unwrap_or_else(|| make_face(TERMINAL_SYMBOL_FAMILY));
    cx.set_global(MonospaceFonts { installed, default });

    // Let Android locate other scripts on demand instead of bundling them.
    cx.on_missing_glyphs(move |missing, cx| {
        let mut fonts = Vec::new();
        for glyph in missing {
            let units: Vec<_> = glyph.grapheme().encode_utf16().collect();
            if units.is_empty() {
                continue;
            }
            let family = match glyph.font_class() {
                FallbackFontClass::Monospace => c"monospace",
                FallbackFontClass::Proportional => c"sans-serif",
            };
            let matched = matcher.match_font(family, &units, None);
            let path = matched.path();
            if loaded.insert(path.to_owned())
                && let Ok(bytes) = fs::read(path)
            {
                fonts.push(Cow::Owned(bytes));
            }
        }
        if !fonts.is_empty() && cx.text_system().add_fonts(fonts).is_ok() {
            cx.refresh_windows();
        }
    })
    .detach();
    ui_family
        .filter(|name| available.contains(name))
        .unwrap_or_else(|| ".SystemUIFont".to_owned())
        .into()
}
