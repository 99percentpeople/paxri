use super::{
    display::{COL, LINE},
    emulator::Emulator,
};
use crate::theme::{self, TerminalAppearance};
use alacritty_terminal::{
    grid::Dimensions,
    index::Point as GridPoint,
    term::{TermMode, cell::Flags},
    vte::ansi::{CursorShape, CursorStyle},
};
use gpui::{App, Bounds, Pixels, Point, Task, Window, point, px, size};
use std::time::Duration;

pub(super) fn default_style(appearance: TerminalAppearance) -> CursorStyle {
    CursorStyle {
        shape: match appearance.cursor_shape {
            theme::CursorShape::Block => CursorShape::Block,
            theme::CursorShape::Underline => CursorShape::Underline,
            theme::CursorShape::Bar => CursorShape::Beam,
        },
        blinking: appearance.blink_cursor,
    }
}

#[derive(Clone, Copy)]
pub(super) struct CursorCell {
    pub point: GridPoint,
    pub width: u16,
}

impl CursorCell {
    pub(super) fn visible(screen: &Emulator) -> Option<Self> {
        let grid = screen.term.grid();
        if grid.display_offset() != 0 || !screen.term.mode().contains(TermMode::SHOW_CURSOR) {
            return None;
        }
        let mut point = grid.cursor.point;
        if grid[point].flags.contains(Flags::WIDE_CHAR_SPACER) {
            point.column.0 = point.column.0.saturating_sub(1);
        }
        let wide =
            grid[point].flags.contains(Flags::WIDE_CHAR) && point.column < grid.last_column();
        Some(Self {
            point,
            width: if wide { 2 } else { 1 },
        })
    }

    pub(super) fn covers(self, point: GridPoint) -> bool {
        point.line == self.point.line
            && (self.point.column.0..self.point.column.0 + usize::from(self.width))
                .contains(&point.column.0)
    }

    pub(super) fn stroke(self, shape: CursorShape, at: Point<Pixels>) -> Option<Bounds<Pixels>> {
        match shape {
            CursorShape::Underline => Some(Bounds::new(
                at + point(px(0.), px(LINE - 2.)),
                size(px(COL * f32::from(self.width)), px(2.)),
            )),
            CursorShape::Beam => Some(Bounds::new(at, size(px(2.), px(LINE)))),
            _ => None,
        }
    }
}

struct Blink {
    point: GridPoint,
    hidden: bool,
    next: Option<Task<()>>,
}

// Element state is dropped when the terminal is hidden or leaves this window.
// A steady cursor needs no scheduled work.
pub(super) fn blink_visible(
    enabled: bool,
    cell: Option<CursorCell>,
    window: &mut Window,
    cx: &mut App,
) -> bool {
    let Some(cell) = cell.filter(|_| enabled && window.is_window_active()) else {
        return true;
    };
    let blink = window.use_keyed_state("terminal-cursor-blink", cx, |_, _| Blink {
        point: cell.point,
        hidden: false,
        next: None,
    });
    blink.update(cx, |blink, cx| {
        if blink.point != cell.point {
            blink.point = cell.point;
            blink.hidden = false;
            blink.next = None;
        }
        if blink.next.is_none() {
            blink.next = Some(cx.spawn(async move |this, cx| {
                cx.background_executor()
                    .timer(Duration::from_millis(550))
                    .await;
                let _ = this.update(cx, |blink, cx| {
                    blink.next = None;
                    blink.hidden = !blink.hidden;
                    cx.notify();
                });
            }));
        }
        !blink.hidden
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terminal::emulator::GridSize;
    use alacritty_terminal::{
        grid::Scroll,
        index::{Column, Line},
    };

    #[test]
    fn remote_cursor_overrides_preferences_until_reset_and_reports_its_position() {
        let mut screen = Emulator::new(GridSize { rows: 8, cols: 20 });
        let fallback = CursorStyle {
            shape: CursorShape::Beam,
            blinking: true,
        };
        screen.set_default_cursor(fallback);
        screen.process(b"\x1b[4;");
        screen.process(b"8H\x1b[6n");
        assert_eq!(screen.finish_batch(), b"\x1b[4;8R");
        for (n, shape) in [
            (1, CursorShape::Block),
            (3, CursorShape::Underline),
            (5, CursorShape::Beam),
        ] {
            for code in n..=n + 1 {
                for byte in format!("\x1b[{code} q").bytes() {
                    screen.process(&[byte]);
                }
                assert_eq!(
                    screen.term.cursor_style(),
                    CursorStyle {
                        shape,
                        blinking: code == n
                    }
                );
            }
        }
        screen.process(b"\x1b[2 q\x1b[?12h");
        let new_default = CursorStyle {
            shape: CursorShape::Underline,
            blinking: false,
        };
        screen.set_default_cursor(new_default);
        assert_eq!(
            screen.term.cursor_style(),
            CursorStyle {
                shape: CursorShape::Block,
                blinking: true
            }
        );
        screen.process(b"\x1b[?12l\x1b[?25l");
        assert!(!screen.term.cursor_style().blinking);
        assert!(CursorCell::visible(&screen).is_none());
        screen.process(b"\x1b[0 q\x1b[?25h");
        assert_eq!(screen.term.cursor_style(), new_default);
        assert_eq!(
            CursorCell::visible(&screen).unwrap().point,
            GridPoint::new(Line(3), Column(7))
        );
    }

    #[test]
    fn all_cursor_shapes_align_to_the_whole_wide_character() {
        let mut screen = Emulator::new(GridSize { rows: 3, cols: 8 });
        screen.process("a界b\x1b[1;3H".as_bytes());
        let cell = CursorCell::visible(&screen).unwrap();
        assert_eq!(cell.point, GridPoint::new(Line(0), Column(1)));
        assert!(cell.covers(GridPoint::new(Line(0), Column(2))));
        assert!(!cell.covers(GridPoint::new(Line(0), Column(3))));
        let at = point(px(30.), px(40.));
        assert_eq!(
            cell.stroke(CursorShape::Underline, at),
            Some(Bounds::new(point(px(30.), px(58.)), size(px(18.), px(2.))))
        );
        assert_eq!(
            cell.stroke(CursorShape::Beam, at),
            Some(Bounds::new(at, size(px(2.), px(20.))))
        );
        assert!(cell.stroke(CursorShape::Block, at).is_none());
        screen.process(b"\x1b[?25l");
        assert!(CursorCell::visible(&screen).is_none());
        screen.process(b"\x1b[?25h\r\n1\r\n2\r\n3");
        screen.scroll(Scroll::Top);
        assert!(CursorCell::visible(&screen).is_none());
    }
}
