use crate::{
    components::{Select, Ui, form_row},
    terminal::TerminalPreview,
    theme::{CursorShape, TerminalAppearance, TerminalScheme},
};
use gpui::{prelude::*, *};

#[derive(Default)]
pub(crate) struct TerminalSettings {
    error: Option<String>,
}

impl TerminalSettings {
    fn change(&mut self, cx: &mut Context<Self>, change: impl FnOnce(&mut TerminalAppearance)) {
        let mut style = *cx.global::<TerminalAppearance>();
        change(&mut style);
        self.error = style
            .apply(cx)
            .err()
            .map(|e| format!("Save terminal appearance: {e}"));
        cx.notify();
    }
}

impl Render for TerminalSettings {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let ui = Ui::new(window, cx);
        let style = *cx.global::<TerminalAppearance>();
        let mut scheme = Select::new(
            "terminal-scheme",
            style.scheme,
            cx.listener(|view, value: &TerminalScheme, _, cx| {
                view.change(cx, |style| style.scheme = *value);
            }),
        );
        for value in TerminalScheme::ALL {
            scheme = scheme.option(value, value.label());
        }
        let mut cursor = Select::new(
            "terminal-cursor-shape",
            style.cursor_shape,
            cx.listener(|view, value: &CursorShape, _, cx| {
                view.change(cx, |style| style.cursor_shape = *value);
            }),
        );
        for shape in CursorShape::ALL {
            cursor = cursor.option(shape, shape.label());
        }
        let blink = Select::new(
            "terminal-cursor-blink",
            style.blink_cursor,
            cx.listener(|view, value: &bool, _, cx| {
                view.change(cx, |style| style.blink_cursor = *value);
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
            .when_some(self.error.as_ref(), |section, message| {
                section.child(ui.alert(message.clone()))
            })
    }
}
