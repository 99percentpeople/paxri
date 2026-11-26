use super::TerminalAppearance;
use gpui::{Font, FontFeatures, Global, SharedString, TextSystem, font, px};

pub const FONT_SIZE: f32 = 15.;
pub const TERMINAL_SYMBOL_FAMILY: &str = "paxri Terminal Symbols";
pub const TERMINAL_SYMBOL_FONT: &[u8] = include_bytes!("../../fonts/terminal-symbols.ttf");

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

impl TerminalFont {
    pub fn scaled(mut self, scale: f32) -> Self {
        self.size *= scale;
        self.cell.width *= scale;
        self.cell.height *= scale;
        self
    }
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
    fn alternatives(&self) -> impl Iterator<Item = &Face> {
        self.installed.iter().filter(|face| {
            face.font.family != self.default.font.family
                && face.font.family.as_ref() != TERMINAL_SYMBOL_FAMILY
        })
    }

    pub fn custom_face(&self, family: &str) -> Option<&Face> {
        self.alternatives()
            .find(|face| face.font.family.as_ref() == family)
    }

    /// An empty family selects the same platform default that the terminal resolves.
    pub fn choices(&self) -> impl Iterator<Item = (SharedString, &Font)> {
        std::iter::once((SharedString::default(), &self.default.font)).chain(
            self.alternatives()
                .map(|face| (face.font.family.clone(), &face.font)),
        )
    }

    pub fn resolve(&self, style: &TerminalAppearance) -> TerminalFont {
        let face = self
            .custom_face(&style.font_family)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picker_and_terminal_agree_on_the_default_family() {
        let face = |name| Face {
            font: font(name),
            advance: 9.,
        };
        let fonts = MonospaceFonts {
            default: face("System Mono"),
            installed: vec![
                face("System Mono"),
                face(TERMINAL_SYMBOL_FAMILY),
                face("Other Mono"),
            ],
        };
        let choices: Vec<_> = fonts.choices().map(|(key, _)| key.to_string()).collect();
        assert_eq!(choices, ["", "Other Mono"]);
        for (saved, expected) in [
            ("", "System Mono"),
            ("System Mono", "System Mono"),
            (TERMINAL_SYMBOL_FAMILY, "System Mono"),
            ("Unavailable", "System Mono"),
            ("Other Mono", "Other Mono"),
        ] {
            let style = TerminalAppearance {
                font_family: saved.to_owned(),
                ..Default::default()
            };
            assert_eq!(fonts.resolve(&style).face.family.as_ref(), expected);
        }
    }
}
