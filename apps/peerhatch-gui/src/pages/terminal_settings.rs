use super::settings_catalog as catalog;
use crate::{
    Dashboard,
    components::{Select, SettingsGroup, Ui, setting_field},
    terminal::TerminalPreview,
    theme::{self, CursorShape, TerminalAppearance, TerminalScheme, fonts::MonospaceFonts},
};
use gpui::{prelude::*, *};
use peerhatch_control::{Command, GuiChange};

impl Dashboard {
    pub(super) fn terminal_settings(&self, ui: Ui, cx: &Context<Self>) -> SettingsGroup {
        let style = theme::current(cx).terminal;
        let mut group = SettingsGroup::new(catalog::TERMINAL);
        group.notice = Some(TerminalPreview(style.clone()).into_any_element());
        let fonts = cx.global::<MonospaceFonts>();
        let mut schemes = Select::new(
            "terminal-scheme",
            style.scheme,
            cx.listener(|view, scheme: &TerminalScheme, _, cx| {
                view.send(
                    Command::SetGui {
                        change: GuiChange::Scheme(*scheme),
                    },
                    cx,
                );
            }),
        );
        for scheme in TerminalScheme::ALL {
            let palette = theme::terminal_palette(scheme, ui.colors);
            schemes = schemes.option_with_content(scheme, move || {
                let swatch = div().flex_none().flex().flex_col().gap(px(1.)).children(
                    palette.ansi.chunks_exact(8).map(|row| {
                        div().flex().gap(px(1.)).children(
                            row.iter()
                                .map(|&color| div().w(px(4.)).h(px(5.)).bg(rgb(color))),
                        )
                    }),
                );
                div()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .gap(theme::SPACE_2)
                    .child(swatch)
                    .child(div().truncate().child(scheme.label()))
            });
        }
        group.grouped_setting("Text", catalog::SCHEME, setting_field(schemes));
        let selected = fonts
            .installed
            .iter()
            .find(|face| face.font.family.as_ref() == style.font_family)
            .map(|face| face.font.family.clone())
            .unwrap_or_default();
        let mut families = Select::new(
            "terminal-font-family",
            selected,
            cx.listener(|v, family: &SharedString, _, cx| {
                v.send(
                    Command::SetGui {
                        change: GuiChange::FontFamily(family.to_string()),
                    },
                    cx,
                );
            }),
        );
        for (family, font) in fonts.choices() {
            let label = if family.is_empty() {
                "Platform default".into()
            } else {
                family.clone()
            };
            let font = font.clone();
            families = families.option_with_content(family, move || {
                div()
                    .min_w_0()
                    .truncate()
                    .font(font.clone())
                    .child(label.clone())
            });
        }
        group.grouped_setting("Text", catalog::FONT_FAMILY, setting_field(families));
        group.grouped_setting(
            "Text",
            catalog::LINE_HEIGHT,
            ui.setting_choice(
                "terminal-line-height",
                style.line_height,
                TerminalAppearance::LINE_HEIGHTS,
                |value| format!("{value} px"),
                cx.listener(|v, value, _, cx| {
                    v.send(
                        Command::SetGui {
                            change: GuiChange::LineHeight(*value),
                        },
                        cx,
                    )
                }),
            ),
        );
        group.grouped_setting(
            "Text",
            catalog::LETTER_SPACING,
            ui.setting_choice(
                "terminal-letter-spacing",
                style.letter_spacing,
                TerminalAppearance::LETTER_SPACINGS,
                |value| format!("{value} px"),
                cx.listener(|v, value, _, cx| {
                    v.send(
                        Command::SetGui {
                            change: GuiChange::LetterSpacing(*value),
                        },
                        cx,
                    )
                }),
            ),
        );
        group.grouped_setting(
            "Cursor",
            catalog::CURSOR_SHAPE,
            ui.setting_choice(
                "terminal-cursor-shape",
                style.cursor_shape,
                CursorShape::ALL,
                CursorShape::label,
                cx.listener(|v, value, _, cx| {
                    v.send(
                        Command::SetGui {
                            change: GuiChange::CursorShape(*value),
                        },
                        cx,
                    )
                }),
            ),
        );
        group.grouped_setting(
            "Cursor",
            catalog::CURSOR_BLINK,
            ui.setting_toggle(
                "terminal-cursor-blink",
                style.blink_cursor,
                true,
                cx.listener(move |view, _, _, cx| {
                    view.send(
                        Command::SetGui {
                            change: GuiChange::BlinkCursor(!style.blink_cursor),
                        },
                        cx,
                    )
                }),
            ),
        );
        group
    }
}
