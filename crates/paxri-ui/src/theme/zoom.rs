use gpui::{Keystroke, Modifiers};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Zoom {
    In,
    Out,
    Reset,
    Pinch(f32),
}

impl Zoom {
    pub fn from_key(key: &Keystroke) -> Option<Self> {
        let mut modifiers = key.modifiers;
        modifiers.shift = false;
        if modifiers != Modifiers::control() && modifiers != Modifiers::secondary_key() {
            return None;
        }
        match key.key.as_str() {
            "+" | "=" | "add" => Some(Self::In),
            "-" | "subtract" if !key.modifiers.shift => Some(Self::Out),
            "0" if !key.modifiers.shift => Some(Self::Reset),
            _ => None,
        }
    }

    pub fn scale(self, current: f32) -> f32 {
        match self {
            Self::In => current + 0.125,
            Self::Out => current - 0.125,
            Self::Reset => 1.,
            Self::Pinch(factor) if factor.is_finite() && factor > 0. => current * factor,
            Self::Pinch(_) => current,
        }
        .clamp(0.5, 2.)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::fonts::TerminalFont;

    #[test]
    fn zoom_only_claims_its_shortcuts() {
        for (key, expected) in [
            ("ctrl-=", Some(Zoom::In)),
            ("ctrl-shift-+", Some(Zoom::In)),
            ("ctrl--", Some(Zoom::Out)),
            ("ctrl-0", Some(Zoom::Reset)),
            ("=", None),
            ("ctrl-c", None),
            ("ctrl-alt-=", None),
            ("ctrl-shift-0", None),
        ] {
            assert_eq!(
                Zoom::from_key(&Keystroke::parse(key).unwrap()),
                expected,
                "{key}"
            );
        }
    }

    #[test]
    fn pinch_and_keyboard_scale_custom_cell_metrics_together() {
        let mut base = TerminalFont::test_font();
        base.cell.height = 27.;
        let scale = Zoom::Pinch(1.5).scale(1.);
        let font = base.clone().scaled(scale);
        assert_eq!(
            (font.size, font.cell.width, font.cell.height),
            (22.5, 13.5, 40.5)
        );
        assert_eq!(Zoom::Out.scale(Zoom::In.scale(scale)), scale);
        assert_eq!(base.scaled(Zoom::Reset.scale(scale)).cell.height, 27.);
        assert_eq!(Zoom::Pinch(f32::NAN).scale(scale), scale);
        assert_eq!(Zoom::Pinch(0.01).scale(scale), 0.5);
        assert_eq!(Zoom::Pinch(5.).scale(scale), 2.);
    }
}
