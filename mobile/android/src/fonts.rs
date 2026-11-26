use crate::bridge;
use gpui::{App, FallbackFontClass, Font as GpuiFont, FontFallbacks, font};
use ndk::font::{Font, FontMatcher};
use paxri_ui::theme::fonts::{Face, MonospaceFonts, TERMINAL_SYMBOL_FAMILY, TERMINAL_SYMBOL_FONT};
use std::{
    borrow::Cow,
    collections::{HashMap, HashSet},
    ffi::CString,
    fs,
    path::PathBuf,
};
use ttf_parser::{Face as FontFile, name_id};

fn family_name(bytes: &[u8], collection_index: u32) -> Option<String> {
    let face = FontFile::parse(bytes, collection_index).ok()?;
    for id in [name_id::TYPOGRAPHIC_FAMILY, name_id::FAMILY] {
        let names = || face.names().into_iter().filter(|n| n.name_id == id);
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

fn font_tables(bytes: &[u8]) -> Option<Vec<[u8; 4]>> {
    if bytes.len() < 12 {
        return None;
    }
    let num_tables = u16::from_be_bytes([bytes[4], bytes[5]]) as usize;
    let end = 12usize.checked_add(num_tables.checked_mul(16)?)?;
    if bytes.len() < end {
        return None;
    }
    let mut tags = Vec::with_capacity(num_tables);
    for i in 0..num_tables {
        let off = 12 + i * 16;
        tags.push([bytes[off], bytes[off + 1], bytes[off + 2], bytes[off + 3]]);
    }
    Some(tags)
}

fn colr_version(bytes: &[u8]) -> Option<u16> {
    if bytes.len() < 12 {
        return None;
    }
    let num_tables = u16::from_be_bytes([bytes[4], bytes[5]]) as usize;
    let end = 12usize.checked_add(num_tables.checked_mul(16)?)?;
    if bytes.len() < end {
        return None;
    }
    for i in 0..num_tables {
        let off = 12 + i * 16;
        if &bytes[off..off + 4] == b"COLR" {
            let table_off = u32::from_be_bytes([
                bytes[off + 8],
                bytes[off + 9],
                bytes[off + 10],
                bytes[off + 11],
            ]) as usize;
            let version = bytes.get(table_off..table_off + 2)?;
            return Some(u16::from_be_bytes([version[0], version[1]]));
        }
    }
    None
}

/// swash renders CBDT/CBLC, sbix, and COLR v0 color emoji, but not COLR v1.
fn supports_swash_color_emoji(bytes: &[u8]) -> bool {
    let Some(tags) = font_tables(bytes) else {
        return false;
    };
    if tags.iter().any(|t| t == b"CBDT" || t == b"sbix") {
        return true;
    }
    colr_version(bytes) == Some(0)
}

fn is_unsupported_colr_v1_emoji(bytes: &[u8]) -> bool {
    let Some(tags) = font_tables(bytes) else {
        return false;
    };
    let has_bitmap = tags.iter().any(|t| t == b"CBDT" || t == b"sbix");
    !has_bitmap && colr_version(bytes).is_some_and(|v| v >= 1)
}

fn load_font_file(
    path: &std::path::Path,
    index: u32,
    existing: &[String],
    files: &mut HashMap<PathBuf, Vec<u8>>,
    loaded: &mut HashSet<PathBuf>,
) -> Option<String> {
    if let Some(bytes) = files.get(path) {
        return family_name(bytes, index);
    }
    let bytes = fs::read(path).ok()?;
    if is_unsupported_colr_v1_emoji(&bytes) {
        loaded.insert(path.to_owned());
        return None;
    }
    let name = family_name(&bytes, index)?;
    loaded.insert(path.to_owned());
    if !existing.iter().any(|known| known == &name) {
        files.insert(path.to_owned(), bytes);
    }
    Some(name)
}

fn matched_family(
    matched: &Font,
    existing: &[String],
    files: &mut HashMap<PathBuf, Vec<u8>>,
    loaded: &mut HashSet<PathBuf>,
) -> Option<String> {
    load_font_file(
        matched.path(),
        matched.collection_index() as u32,
        existing,
        files,
        loaded,
    )
}

fn resolve_color_emoji_family(
    matcher: &mut FontMatcher,
    existing: &[String],
    files: &mut HashMap<PathBuf, Vec<u8>>,
    loaded: &mut HashSet<PathBuf>,
) -> Option<String> {
    let mut emoji_units = [0u16; 2];
    let units = '😀'.encode_utf16(&mut emoji_units);
    let matched = matcher.match_font(c"sans-serif", units, None);
    let mut candidates = vec![(matched.path().to_owned(), matched.collection_index() as u32)];
    for fallback_path in [
        "/system/fonts/SamsungColorEmoji.ttf",
        "/system/fonts/NotoColorEmojiLegacy.ttf",
        "/system/fonts/NotoColorEmoji.ttf",
    ] {
        let path = PathBuf::from(fallback_path);
        if !candidates.iter().any(|(existing_path, _)| existing_path == &path) {
            candidates.push((path, 0));
        }
    }
    for (path, index) in candidates {
        let Ok(bytes) = fs::read(&path) else { continue };
        if !supports_swash_color_emoji(&bytes) {
            loaded.insert(path);
            continue;
        }
        let Some(name) = family_name(&bytes, index) else {
            continue;
        };
        loaded.insert(path.clone());
        if !existing.iter().any(|known| known == &name) {
            files.insert(path, bytes);
        }
        return Some(name);
    }
    None
}

fn noto_cjk_for_locales(locales: &str) -> Option<&'static str> {
    for raw in locales.split(',') {
        let tag = raw.trim();
        if tag.is_empty() {
            continue;
        }
        let mut parts = tag.split(['-', '_']);
        let Some(lang) = parts.next() else { continue };
        if lang.eq_ignore_ascii_case("ja") {
            return Some("Noto Sans CJK JP");
        }
        if lang.eq_ignore_ascii_case("ko") {
            return Some("Noto Sans CJK KR");
        }
        if lang.eq_ignore_ascii_case("zh") {
            let rest: Vec<_> = parts.collect();
            if rest.iter().any(|part| part.eq_ignore_ascii_case("hk")) {
                return Some("Noto Sans CJK HK");
            }
            if rest.iter().any(|part| {
                part.eq_ignore_ascii_case("hant")
                    || part.eq_ignore_ascii_case("tw")
                    || part.eq_ignore_ascii_case("mo")
            }) {
                return Some("Noto Sans CJK TC");
            }
            return Some("Noto Sans CJK SC");
        }
    }
    None
}

fn push_unique(target: &mut Vec<String>, candidate: Option<String>, available: &[String]) {
    if let Some(name) = candidate
        && available.contains(&name)
        && !target.contains(&name)
    {
        target.push(name);
    }
}

pub(super) fn init(cx: &mut App) -> GpuiFont {
    let text = cx.text_system();
    let existing = text.all_font_names();
    let mut matcher = FontMatcher::new();
    let locales = bridge::system_locales();
    if let Some(tags) = locales.as_deref()
        && let Ok(c_tags) = CString::new(tags)
    {
        matcher.set_locales(&c_tags);
    }

    let mut system_files = HashMap::<PathBuf, Vec<u8>>::new();
    let mut loaded = HashSet::<PathBuf>::new();
    let ui = matcher.match_font(c"sans-serif", &[u16::from(b'M')], None);
    let ui_family = matched_family(&ui, &existing, &mut system_files, &mut loaded);

    // Match Android's locale-specific Hanzi fallback so unified CJK characters
    // follow the system script (e.g. Simplified Chinese) instead of TTC face 0 (JP).
    let han = ['中' as u16];
    let cjk_ui = matched_family(
        &matcher.match_font(c"sans-serif", &han, None),
        &existing,
        &mut system_files,
        &mut loaded,
    );
    let cjk_mono = matched_family(
        &matcher.match_font(c"monospace", &han, None),
        &existing,
        &mut system_files,
        &mut loaded,
    );
    let noto_cjk = locales
        .as_deref()
        .and_then(noto_cjk_for_locales)
        .map(str::to_owned);
    let color_emoji = resolve_color_emoji_family(
        &mut matcher,
        &existing,
        &mut system_files,
        &mut loaded,
    );

    let mut system_families = Vec::new();
    let mut preferred = None;
    // Use a locale-neutral matcher for the console repertoire so CJK locales do
    // not pull fullwidth 1em CJK glyphs (like ●, ■, →) ahead of monospace symbols.
    let mut console_matcher = FontMatcher::new();
    let cjk_families: Vec<&str> = [cjk_mono.as_deref(), cjk_ui.as_deref(), noto_cjk.as_deref()]
        .into_iter()
        .flatten()
        .collect();
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
        let matched = console_matcher.match_font(c"monospace", remaining, Some(&mut count));
        if let Some(name) = matched_family(&matched, &existing, &mut system_files, &mut loaded) {
            if remaining.len() == console.len() {
                preferred = Some(name.clone());
            }
            if Some(name.as_str()) != color_emoji.as_deref()
                && !cjk_families.contains(&name.as_str())
                && !system_families.contains(&name)
            {
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
            && !loaded.contains(&path)
            && let Ok(bytes) = fs::read(&path)
        {
            loaded.insert(path.clone());
            system_files.insert(path, bytes);
        }
    }
    let mut fonts: Vec<_> = system_files.into_values().map(Cow::Owned).collect();
    fonts.push(Cow::Borrowed(TERMINAL_SYMBOL_FONT));
    if let Err(error) = text.add_fonts(fonts) {
        log::warn!("Registering terminal fonts: {error}");
    }
    let available = text.all_font_names();
    system_families.retain(|name| available.contains(name));
    let preferred = preferred.filter(|name| available.contains(name));

    let mut ui_fallbacks = Vec::new();
    push_unique(&mut ui_fallbacks, cjk_ui.clone(), &available);
    push_unique(&mut ui_fallbacks, noto_cjk.clone(), &available);
    push_unique(&mut ui_fallbacks, color_emoji.clone(), &available);

    let mut mono_cjk_fallbacks = Vec::new();
    push_unique(&mut mono_cjk_fallbacks, cjk_mono, &available);
    push_unique(&mut mono_cjk_fallbacks, cjk_ui, &available);
    push_unique(&mut mono_cjk_fallbacks, noto_cjk, &available);
    push_unique(&mut mono_cjk_fallbacks, color_emoji.clone(), &available);

    log::info!(
        "Android fonts: locales={:?} ui={:?} ui_fallbacks={:?} mono_default={:?} mono_cjk={:?} emoji={:?}",
        locales,
        ui_family,
        ui_fallbacks,
        preferred,
        mono_cjk_fallbacks,
        color_emoji
    );

    let make_face = |name: &str| {
        let mut face = Face::measure(name.to_owned().into(), text);
        let mut fallbacks = Vec::new();
        // Prefer the bundled monospace terminal symbols ahead of proportional system
        // symbol fonts (e.g. Noto Sans Symbols) and fullwidth CJK fonts so single-cell
        // terminal glyphs stay within one grid column.
        for family in [TERMINAL_SYMBOL_FAMILY]
            .into_iter()
            .chain(system_families.iter().map(String::as_str))
            .chain(mono_cjk_fallbacks.iter().map(String::as_str))
        {
            if family != name && !fallbacks.iter().any(|existing| existing == family) {
                fallbacks.push(family.to_owned());
            }
        }
        face.font.fallbacks = Some(FontFallbacks::from_fonts(fallbacks));
        face
    };
    let installed: Vec<_> = available
        .iter()
        .filter(|name| {
            let lower = name.to_lowercase();
            (lower.contains("mono") && !lower.contains("cjk")) || Some(*name) == preferred.as_ref()
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
                && !is_unsupported_colr_v1_emoji(&bytes)
            {
                fonts.push(Cow::Owned(bytes));
            }
        }
        if !fonts.is_empty() && cx.text_system().add_fonts(fonts).is_ok() {
            cx.refresh_windows();
        }
    })
    .detach();

    let ui_name = ui_family
        .filter(|name| available.contains(name))
        .unwrap_or_else(|| ".SystemUIFont".to_owned());
    let mut ui_font = font(ui_name);
    let ui_fallbacks: Vec<_> = ui_fallbacks
        .into_iter()
        .filter(|name| name != ui_font.family.as_ref())
        .collect();
    if !ui_fallbacks.is_empty() {
        ui_font.fallbacks = Some(FontFallbacks::from_fonts(ui_fallbacks));
    }
    ui_font
}
