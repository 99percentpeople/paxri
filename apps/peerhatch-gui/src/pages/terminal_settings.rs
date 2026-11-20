use crate::{
    Dashboard,
    components::{Select, Ui, form_row},
    terminal::TerminalPreview,
    theme::{self, CursorShape, TerminalScheme},
};
use gpui::{prelude::*, *};

use peerhatch_control::{Command, GuiChange};

impl Dashboard {
    pub(super) fn terminal_settings(&self, ui: Ui, cx: &Context<Self>) -> Div {
        let style = theme::current(cx).terminal;
        let mut scheme = Select::new(
            "terminal-scheme",
            style.scheme,
            cx.listener(|view, value: &TerminalScheme, _, cx| {
                view.send(
                    Command::SetGui {
                        change: GuiChange::Scheme(*value),
                    },
                    cx,
                );
            }),
        );
        for value in TerminalScheme::ALL {
            scheme = scheme.option(value, value.label());
        }
        let mut cursor = Select::new(
            "terminal-cursor-shape",
            style.cursor_shape,
            cx.listener(|view, value: &CursorShape, _, cx| {
                view.send(
                    Command::SetGui {
                        change: GuiChange::CursorShape(*value),
                    },
                    cx,
                );
            }),
        );
        for shape in CursorShape::ALL {
            cursor = cursor.option(shape, shape.label());
        }
        let blink = Select::new(
            "terminal-cursor-blink",
            style.blink_cursor,
            cx.listener(|view, value: &bool, _, cx| {
                view.send(
                    Command::SetGui {
                        change: GuiChange::BlinkCursor(*value),
                    },
                    cx,
                );
            }),
        )
        .option(false, "Steady")
        .option(true, "Blinking");

        ui.section("Terminal")
            .child(
                form_row()
                    .child(ui.labeled("Color scheme", scheme))
                    .child(ui.labeled("Default cursor shape", cursor))
                    .child(ui.labeled("Default cursor blink", blink)),
            )
            .child(TerminalPreview(style))
    }
}
