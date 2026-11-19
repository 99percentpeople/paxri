use super::{
    cursor::{self, CursorCell},
    display,
    emulator::{Emulator, GridSize},
};
use crate::theme::{self, TerminalAppearance};
use gpui::{prelude::*, *};

#[derive(IntoElement)]
pub(crate) struct TerminalPreview(pub TerminalAppearance);

impl RenderOnce for TerminalPreview {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let appearance = self.0;
        let ui = theme::palette(window, cx);
        let palette = crate::theme::terminal_palette(appearance.scheme, ui);
        let sample = window.use_keyed_state("terminal-preview-screen", cx, |_, _| {
            let mut screen = Emulator::with_palette(GridSize { rows: 3, cols: 26 }, palette);
            screen.process(b"\x1b[36m~\x1b[0m $ ls\r\n\x1b[1;34msrc/\x1b[0m  \x1b[32mtunnel\x1b[0m  Cargo.toml\r\n\x1b[36m~\x1b[0m $ ");
            screen
        });
        let cell = CursorCell::visible(sample.read(cx));
        let cursor_on = cursor::blink_visible(appearance.blink_cursor, cell, window, cx);
        div()
            .p(theme::SPACE_3)
            .rounded(theme::CONTROL_RADIUS)
            .border_1()
            .border_color(rgb(ui.border))
            .bg(rgb(palette.surface.background))
            .overflow_hidden()
            .child(
                canvas(
                    |_, _, _| (),
                    move |bounds, (), window, cx| {
                        sample.update(cx, |screen, cx| {
                            display::paint(
                                screen,
                                bounds,
                                "",
                                cursor_on.then_some(cursor::default_style(appearance).shape),
                                palette,
                                window,
                                cx,
                            )
                        });
                    },
                )
                .w_full()
                .h(px(3. * display::LINE)),
            )
    }
}
