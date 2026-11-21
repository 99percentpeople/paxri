//! Local font faces and the character-cell dimensions shared by the terminal and its preview.
use super::TerminalAppearance;
use gpui::{App, Font, FontFeatures, Global, SharedString, TextSystem, font, px};
use std::collections::{BTreeSet, HashSet};

const PLATFORM_FONT: &str = if cfg!(windows) {
    "Consolas"
} else if cfg!(target_os = "macos") {
    "Menlo"
} else {
    "DejaVu Sans Mono"
};
pub(crate) const FONT_SIZE: f32 = 15.;

#[derive(Clone, Copy)]
pub(crate) struct CellSize {
    pub width: f32,
    pub height: f32,
}

#[derive(Clone)]
pub(crate) struct TerminalFont {
    pub face: Font,
    pub cell: CellSize,
}

#[derive(Clone)]
pub(crate) struct Face {
    pub font: Font,
    advance: f32,
}

impl Face {
    fn measure(name: SharedString, text: &TextSystem) -> Self {
        let mut font = font(name);
        // Keep operators such as => in separate terminal cells with programming fonts.
        font.features = FontFeatures(vec![("calt".into(), 0), ("liga".into(), 0)].into());
        let id = text.resolve_font(&font);
        let advance = text
            .em_advance(id, px(FONT_SIZE))
            .map(f32::from)
            .unwrap_or(9.);
        Self { font, advance }
    }
}

pub(crate) struct MonospaceFonts {
    pub installed: Vec<Face>,
    default: Face,
}
impl Global for MonospaceFonts {}

impl MonospaceFonts {
    /// An empty family selects the same platform default that the terminal resolves.
    pub fn choices(&self) -> impl Iterator<Item = (SharedString, &Font)> {
        std::iter::once((SharedString::default(), &self.default.font)).chain(
            self.installed
                .iter()
                .map(|face| (face.font.family.clone(), &face.font)),
        )
    }

    pub fn resolve(&self, style: &TerminalAppearance) -> TerminalFont {
        let face = self
            .installed
            .iter()
            .find(|f| f.font.family.as_ref() == style.font_family)
            .unwrap_or(&self.default);
        TerminalFont {
            face: face.font.clone(),
            cell: CellSize {
                width: (face.advance.round() + f32::from(style.letter_spacing)).clamp(1., 64.),
                height: f32::from(style.line_height),
            },
        }
    }
}

pub(super) fn init(cx: &mut App) {
    let text = cx.text_system().clone();
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

#[cfg(test)]
impl TerminalFont {
    pub(crate) fn test_font() -> Self {
        Self {
            face: font(PLATFORM_FONT),
            cell: CellSize {
                width: 9.,
                height: 20.,
            },
        }
    }
}
