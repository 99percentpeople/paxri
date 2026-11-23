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

    pub(super) fn bounds(self, at: Point<Pixels>, cell: CellSize) -> Bounds<Pixels> {
        Bounds::new(
            at,
            size(px(cell.width * f32::from(self.width)), px(cell.height)),
        )
    }
}

pub(super) fn stroke(
    shape: CursorShape,
    cell: Bounds<Pixels>,
    span: GlyphSpan,
) -> Option<Bounds<Pixels>> {
    let mut bounds = span.in_row(cell);
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
            available: screen.term.columns() - span.point.column.0,
            cell,
        })
    }
}

pub struct PreeditLayout<'a> {
    pub text: &'a str,
    pub at: Point<Pixels>,
    pub columns: u16,
    first_utf16: usize,
    available: usize,
    cell: CellSize,
}

impl PreeditLayout<'_> {
    /// Keep narrow text together for shaping and give each fullwidth cluster its own columns.
    pub(super) fn runs(&self) -> impl Iterator<Item = (&str, Range<usize>, bool)> {
        let text = self.text;
        let mut glyphs = text.grapheme_indices(true).peekable();
        let mut column = 0;
        std::iter::from_fn(move || {
            let (start, glyph) = glyphs.next()?;
            let width = glyph.width();
            let first = column;
            column += width;
            let mut end = start + glyph.len();
            if width <= 1 {
                while let Some((byte, glyph)) = glyphs.next_if(|(_, glyph)| glyph.width() <= 1) {
                    end = byte + glyph.len();
                    column += glyph.width();
                }
            }
            Some((&text[start..end], first..column, width > 1))
        })
    }

    /// The insertion cell may follow the marked text, up to the last visible cell.
    pub(super) fn caret_cell(&self, offset: usize) -> Bounds<Pixels> {
        let byte = byte_at_utf16(self.text, offset.saturating_sub(self.first_utf16));
        let mut column = 0;
        let mut width = 1;
        for (index, glyph) in self.text.grapheme_indices(true) {
            let cells = glyph.width();
            if byte < index + glyph.len() || column + cells >= self.available {
                width = cells.max(1);
                break;
            }
            column += cells;
        }
        Bounds::new(
            self.at + point(px(column as f32 * self.cell.width), px(0.)),
            size(
                px(width.min(self.available - column) as f32 * self.cell.width),
                px(self.cell.height),
            ),
        )
    }

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
    fn display_keeps_the_configured_shape_while_vt_controls_blink_visibility_and_position() {
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
                assert_eq!(screen.display_cursor_style().shape, fallback.shape);
                assert_eq!(screen.display_cursor_style().blinking, code == n);
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
        assert_eq!(screen.display_cursor_style().shape, new_default.shape);
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
            layout.runs().collect::<Vec<_>>(),
            [("a", 0..1, false), ("😀", 1..3, true), ("中", 3..5, true)]
        );
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
    fn ime_caret_uses_the_same_shape_and_respects_graphemes_and_the_right_edge() {
        let mut screen = Emulator::new(GridSize { rows: 2, cols: 8 });
        screen.process(b"\x1b[?25l");
        let span = GlyphSpan::from_metrics(px(15.), px(20.), px(14.), px(-4.));
        let mut marked = Preedit::default();
        marked.replace(None, "a中e\u{301}😀", None);
        let check = |screen: &Emulator, offset, col: f32, width: f32| {
            let layout = marked.layout(screen, point(px(0.), px(0.))).unwrap();
            let area = layout.caret_cell(offset);
            assert_eq!(
                area,
                Bounds::new(point(px(col * 9.), px(0.)), size(px(width * 9.), px(20.)))
            );
            let block = stroke(CursorShape::Block, area, span).unwrap();
            let underline = stroke(CursorShape::Underline, area, span).unwrap();
            let bar = stroke(CursorShape::Beam, area, span).unwrap();
            assert_eq!(block.size, size(area.size.width, px(18.)));
            assert_eq!(underline.size, size(area.size.width, px(2.)));
            assert_eq!(bar.size, size(px(2.), block.size.height));
        };
        check(&screen, 1, 1., 2.);
        check(&screen, 3, 3., 1.);
        check(&screen, 4, 4., 2.);
        check(&screen, 6, 6., 1.);
        screen.process(b"\x1b[1;8H");
        check(&screen, 6, 6., 2.);
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
        let span = GlyphSpan::from_metrics(px(15.), px(screen.font.cell.height), px(14.), px(-4.));
        for (shape, bounds) in [
            (
                CursorShape::Underline,
                Bounds::new(point(px(30.), px(57.)), size(px(18.), px(2.))),
            ),
            (
                CursorShape::Beam,
                Bounds::new(point(px(30.), px(41.)), size(px(2.), px(18.))),
            ),
            (
                CursorShape::Block,
                Bounds::new(point(px(30.), px(41.)), size(px(18.), px(18.))),
            ),
        ] {
            assert_eq!(
                stroke(shape, cell.bounds(at, screen.font.cell), span),
                Some(bounds)
            );
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
