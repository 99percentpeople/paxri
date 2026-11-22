use super::emulator::Emulator;
use crate::components::{byte_at_utf16, caret::GlyphSpan};
use crate::theme::{self, TerminalAppearance, fonts::CellSize};
use alacritty_terminal::{
    grid::Dimensions,
    index::Point as GridPoint,
    term::{TermMode, cell::Flags},
    vte::ansi::{CursorShape, CursorStyle},
};
use gpui::{App, Bounds, Pixels, Point, Window, point, px, size};
use std::ops::Range;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

pub fn default_style(appearance: &TerminalAppearance) -> CursorStyle {
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
pub struct CursorCell {
    pub point: GridPoint,
    pub width: u16,
}

impl CursorCell {
    pub fn visible(screen: &Emulator) -> Option<Self> {
        if !screen.term.mode().contains(TermMode::SHOW_CURSOR) {
            return None;
        }
        Self::at_input(screen)
    }

    pub(super) fn composition(screen: &Emulator, text: &str) -> Option<Self> {
        let mut cell = Self::at_input(screen)?;
        let columns = screen.term.columns();
        let width = text.width().clamp(1, columns);
        cell.point.column.0 = cell.point.column.0.min(columns - width);
        cell.width = width as u16;
        Some(cell)
    }

    fn at_input(screen: &Emulator) -> Option<Self> {
        let grid = screen.term.grid();
        if grid.display_offset() != 0 {
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

    pub(super) fn stroke(
        self,
        shape: CursorShape,
        at: Point<Pixels>,
        cell: CellSize,
        span: GlyphSpan,
    ) -> Option<Bounds<Pixels>> {
        let mut bounds = span.in_row(Bounds::new(
            at,
            size(px(cell.width * f32::from(self.width)), px(cell.height)),
        ));
        match shape {
            CursorShape::Underline => {
                let thickness = px(2.).min(bounds.size.height);
                bounds.origin.y = bounds.bottom() - thickness;
                bounds.size.height = thickness;
            }
            CursorShape::Beam => bounds.size.width = px(2.).min(bounds.size.width),
            CursorShape::Block => {}
            _ => return None,
        }
        Some(bounds)
    }
}

#[derive(Default)]
pub struct Preedit {
    pub text: String,
    pub selection: Range<usize>,
}

impl Preedit {
    pub fn clear(&mut self) {
        self.text.clear();
        self.selection = 0..0;
    }

    pub fn replace(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        selection: Option<Range<usize>>,
    ) {
        let bytes = range.map_or(0..self.text.len(), |r| {
            byte_at_utf16(&self.text, r.start)..byte_at_utf16(&self.text, r.end.max(r.start))
        });
        let base = self.text[..bytes.start].encode_utf16().count();
        self.text.replace_range(bytes, text);
        let len = text.encode_utf16().count();
        let selected = selection.unwrap_or(len..len);
        self.selection = base + selected.start.min(len)..base + selected.end.min(len);
    }

    pub fn layout(&self, screen: &Emulator, origin: Point<Pixels>) -> Option<PreeditLayout<'_>> {
        let span = CursorCell::composition(screen, &self.text)?;
        let cell = screen.font.cell;
        let mut start = 0;
        let mut end = self.text.len();
        if self.text.width() > usize::from(span.width) {
            // Show the portion around the IME caret without splitting wide glyphs or emoji.
            let caret = byte_at_utf16(&self.text, self.selection.end);
            let mut width = self.text[..caret].width();
            for (byte, glyph) in self.text.grapheme_indices(true) {
                if width <= usize::from(span.width) {
                    break;
                }
                width = width.saturating_sub(glyph.width());
                start = byte + glyph.len();
            }
            let mut width = 0;
            for (byte, glyph) in self.text[start..].grapheme_indices(true) {
                width += glyph.width();
                if width > usize::from(span.width) {
                    end = start + byte;
                    break;
                }
            }
        }
        Some(PreeditLayout {
            text: &self.text[start..end],
            first_utf16: self.text[..start].encode_utf16().count(),
            at: origin
                + point(
                    px(span.point.column.0 as f32 * cell.width),
                    px(span.point.line.0 as f32 * cell.height),
                ),
            columns: span.width,
            cell,
        })
    }
}

pub struct PreeditLayout<'a> {
    pub text: &'a str,
    pub at: Point<Pixels>,
    pub columns: u16,
    first_utf16: usize,
    cell: CellSize,
}

impl PreeditLayout<'_> {
    pub fn bounds_for_range(&self, range: Range<usize>) -> Bounds<Pixels> {
        let x = |offset: usize| {
            let byte = byte_at_utf16(self.text, offset.saturating_sub(self.first_utf16));
            px(self.text[..byte].width() as f32 * self.cell.width)
        };
        let right = px(f32::from(self.columns) * self.cell.width);
        let stroke = px(1.5).min(right);
        let left = x(range.start).min(right - stroke);
        Bounds::new(
            self.at + point(left, px(0.)),
            size(
                (x(range.end).min(right) - left).max(stroke),
                px(self.cell.height),
            ),
        )
    }
}

pub fn blink_visible(
    enabled: bool,
    cell: Option<CursorCell>,
    window: &mut Window,
    cx: &mut App,
) -> bool {
    crate::components::caret::blink_visible(
        cell.filter(|_| enabled).map(|c| c.point),
        false,
        window,
        cx,
    )
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
    fn preedit_caret_follows_the_ime_selection_with_the_remote_cursor_hidden() {
        let mut screen = Emulator::new(GridSize { rows: 3, cols: 8 });
        screen.process(b"\x1b[?25l\x1b[3;8H");
        let origin = point(px(6.), px(4.));
        let mut marked = Preedit::default();
        marked.replace(None, "a😀中", Some(3..3));
        let layout = marked.layout(&screen, origin).unwrap();
        assert_eq!(layout.text, "a😀中");
        assert_eq!(
            layout.bounds_for_range(3..3).origin,
            point(px(60.), px(44.))
        );

        marked.replace(Some(1..3), "ni", None);
        assert_eq!(marked.text, "ani中");
        assert_eq!(marked.selection, 3..3);

        marked.replace(None, "12345678901", None);
        let layout = marked.layout(&screen, origin).unwrap();
        assert_eq!(layout.text, "45678901");
        assert_eq!(
            layout.bounds_for_range(marked.selection.clone()),
            Bounds::new(point(px(76.5), px(44.)), size(px(1.5), px(20.)))
        );
        marked.clear();
        assert!(marked.text.is_empty());
        assert_eq!(marked.selection, 0..0);
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
        let span = GlyphSpan {
            top: px(4.),
            height: px(12.),
        };
        for (shape, bounds) in [
            (
                CursorShape::Underline,
                Bounds::new(point(px(30.), px(54.)), size(px(18.), px(2.))),
            ),
            (
                CursorShape::Beam,
                Bounds::new(point(px(30.), px(44.)), size(px(2.), px(12.))),
            ),
            (
                CursorShape::Block,
                Bounds::new(point(px(30.), px(44.)), size(px(18.), px(12.))),
            ),
        ] {
            assert_eq!(cell.stroke(shape, at, screen.font.cell, span), Some(bounds));
        }
        screen.process(b"\x1b[?25l");
        assert!(CursorCell::visible(&screen).is_none());
        screen.process(b"\x1b[3;8H");
        let preedit = CursorCell::composition(&screen, "ni好").unwrap();
        assert_eq!(preedit.point, GridPoint::new(Line(2), Column(4)));
        assert_eq!(preedit.width, 4);
        screen.process(b"\x1b[?25h\r\n1\r\n2\r\n3");
        screen.scroll(Scroll::Top);
        assert!(CursorCell::visible(&screen).is_none());
    }
}
