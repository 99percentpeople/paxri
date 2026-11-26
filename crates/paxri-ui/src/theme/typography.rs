//! Shared typography scale for desktop and Android views.
use gpui::{FontWeight, Rems, Styled};

/// Semantic text tiers unifying font sizes and weights across all views.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextRole {
    /// Top-level page titles (1.5rem / 24px, semibold).
    PageTitle,
    /// Compact app-bar and header titles (1.25rem / 20px, semibold).
    Title,
    /// Section and modal headings (1.125rem / 18px, semibold).
    Section,
    /// Group titles, setting row titles, and window chrome labels (0.875rem / 14px, semibold).
    Subtitle,
    /// Buttons, tabs, and emphasized primary labels (0.875rem / 14px, medium).
    Label,
    /// Standard body copy, controls, inputs, and form labels (0.875rem / 14px, normal).
    Body,
    /// Tooltips, helper text, secondary metadata, and hints (0.75rem / 12px, normal).
    Caption,
    /// Status badges and emphasized compact tags (0.75rem / 12px, medium).
    Badge,
}

impl TextRole {
    pub const fn size(self) -> Rems {
        match self {
            Self::PageTitle => Rems(1.5),
            Self::Title => Rems(1.25),
            Self::Section => Rems(1.125),
            Self::Subtitle | Self::Label | Self::Body => Rems(0.875),
            Self::Caption | Self::Badge => Rems(0.75),
        }
    }

    pub const fn weight(self) -> FontWeight {
        match self {
            Self::PageTitle | Self::Title | Self::Section | Self::Subtitle => FontWeight::SEMIBOLD,
            Self::Label | Self::Badge => FontWeight::MEDIUM,
            Self::Body | Self::Caption => FontWeight::NORMAL,
        }
    }
}

pub trait TypographyExt: Styled + Sized {
    fn text_role(self, role: TextRole) -> Self {
        self.text_size(role.size()).font_weight(role.weight())
    }
}

impl<E: Styled> TypographyExt for E {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_roles_follow_descending_size_hierarchy() {
        assert!(TextRole::PageTitle.size().0 > TextRole::Title.size().0);
        assert!(TextRole::Title.size().0 > TextRole::Section.size().0);
        assert!(TextRole::Section.size().0 > TextRole::Subtitle.size().0);
        assert_eq!(TextRole::Subtitle.size(), TextRole::Label.size());
        assert_eq!(TextRole::Label.size(), TextRole::Body.size());
        assert!(TextRole::Body.size().0 > TextRole::Caption.size().0);
        assert_eq!(TextRole::Caption.size(), TextRole::Badge.size());
        assert_eq!(TextRole::Caption.size(), Rems(0.75));
        assert_eq!(TextRole::Caption.weight(), FontWeight::NORMAL);
        assert_eq!(TextRole::Badge.weight(), FontWeight::MEDIUM);
    }
}
