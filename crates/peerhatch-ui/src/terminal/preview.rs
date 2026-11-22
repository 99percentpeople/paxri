use super::{
    cursor::{self, CursorCell},
    display,
    emulator::{Emulator, GridSize},
};
use crate::theme::{self, TerminalAppearance};
use gpui::{prelude::*, *};

#[derive(IntoElement)]
pub struct TerminalPreview(pub TerminalAppearance, pub theme::fonts::TerminalFont);

impl RenderOnce for TerminalPreview {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let appearance = self.0;
        let ui = theme::palette(window, cx);
        let palette = crate::theme::terminal_palette(appearance.scheme, ui);
        let font = self.1;
        let cell_height = font.cell.height;
        let grid = GridSize { rows: 6, cols: 38 };
        let diff = concat!(
            "\x1b[36m~\x1b[0m $ git diff\r\n",
            "\x1b[2mdiff --git a/demo b/demo\x1b[0m\r\n",
            "\x1b[36m@@ -1 +1 @@\x1b[0m\r\n",
            "\x1b[31m-hello\x1b[0m\r\n",
            "\x1b[32m+hello, world\x1b[0m\r\n",
            "\x1b[36m~\x1b[0m $ ",
        );
        let sample = window.use_keyed_state("terminal-appearance-preview", cx, |_, _| {
            let mut screen = Emulator::with_palette(grid, palette, font.clone());
            screen.process(diff.as_bytes());
            screen
        });
        div()
            .flex()
            .flex_col()
            .w_full()
            .max_w(px(520.))
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
                        if !bounds.intersects(&window.content_mask().bounds) {
                            return;
                        }
                        let cell = CursorCell::visible(sample.read(cx));
                        let cursor_on =
                            cursor::blink_visible(appearance.blink_cursor, cell, window, cx);
                        sample.update(cx, |screen, cx| {
                            screen.font = font;
                            display::paint(
                                screen,
                                bounds,
                                None,
                                cursor_on.then_some(cursor::default_style(&appearance).shape),
                                palette,
                                window,
                                cx,
                            )
                        });
                    },
                )
                .w_full()
                .h(px(f32::from(grid.rows) * cell_height)),
            )
    }
}
