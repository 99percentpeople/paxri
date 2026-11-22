use super::TerminalAppearance;
use gpui::{Font, FontFeatures, Global, SharedString, TextSystem, font, px};

pub const FONT_SIZE: f32 = 15.;

#[derive(Clone, Copy)]
pub struct CellSize {
    pub width: f32,
    pub height: f32,
}

#[derive(Clone)]
pub struct TerminalFont {
    pub face: Font,
    pub size: f32,
    pub cell: CellSize,
}

#[cfg(test)]
impl TerminalFont {
    pub(crate) fn test_font() -> Self {
        Self {
            face: gpui::font("monospace"),
            size: FONT_SIZE,
            cell: CellSize {
                width: 9.,
                height: 20.,
            },
        }
    }
}

#[derive(Clone)]
pub struct Face {
    pub font: Font,
    advance: f32,
}

impl Face {
    pub fn measure(name: SharedString, text: &TextSystem) -> Self {
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

pub struct MonospaceFonts {
    pub installed: Vec<Face>,
    pub default: Face,
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
            size: style.size(),
            cell: CellSize {
                width: {
                    let width = face.advance * style.size() / FONT_SIZE;
                    let width = if cfg!(target_os = "android") {
                        width
                    } else {
                        width.round()
                    };
                    (width + f32::from(style.letter_spacing)).clamp(1., 64.)
                },
                height: style.row_height(style.size()),
            },
        }
    }
}
