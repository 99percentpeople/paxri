use std::ops::Range;
use vt100::Screen;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct Position {
    row: usize,
    col: u16,
}

#[derive(Clone, Copy)]
enum Unit {
    Character,
    Word,
    Line,
}

pub(super) struct Selection {
    anchor: Range<Position>,
    head: Range<Position>,
    unit: Unit,
    pub(super) dragging: bool,
}

/// Row numbers span both the visible screen and retained scrollback.
pub(super) fn top_row(screen: &Screen, discarded: usize) -> usize {
    discarded + screen.scrollback_len() - screen.scrollback()
}

fn cell_start(screen: &Screen, row: u16, col: u16) -> u16 {
    if screen
        .cell(row, col)
        .is_some_and(|c| c.is_wide_continuation())
    {
        col.saturating_sub(1)
    } else {
        col
    }
}

#[derive(PartialEq, Eq)]
enum WordClass {
    Space,
    Word,
    Punctuation(char),
}

fn word_class(screen: &Screen, row: u16, col: u16) -> WordClass {
    let col = cell_start(screen, row, col);
    match screen
        .cell(row, col)
        .and_then(|c| c.contents().chars().next())
    {
        None => WordClass::Space,
        Some(c) if c.is_whitespace() => WordClass::Space,
        Some(c) if c.is_alphanumeric() || c == '_' => WordClass::Word,
        Some(c) => WordClass::Punctuation(c),
    }
}

impl Unit {
    fn span(self, screen: &Screen, top: usize, row: u16, x: f32) -> Range<Position> {
        let cols = screen.size().1.get();
        let x = x.clamp(0., f32::from(cols));
        let col = cell_start(screen, row, (x as u16).min(cols - 1));
        let (start, end) = match self {
            Self::Character => {
                let width = if screen.cell(row, col).is_some_and(|c| c.is_wide()) {
                    2
                } else {
                    1
                };
                let caret = if x < f32::from(col) + f32::from(width) / 2. {
                    col
                } else {
                    (col + width).min(cols)
                };
                (caret, caret)
            }
            Self::Word => {
                let class = word_class(screen, row, col);
                let mut start = col;
                let mut end = col + 1;
                while start > 0 && word_class(screen, row, start - 1) == class {
                    start -= 1;
                }
                while end < cols && word_class(screen, row, end) == class {
                    end += 1;
                }
                (start, end)
            }
            Self::Line => (0, cols),
        };
        let row = top + usize::from(row);
        Position { row, col: start }..Position { row, col: end }
    }
}

impl Selection {
    pub(super) fn start(screen: &Screen, top: usize, row: u16, x: f32, clicks: usize) -> Self {
        let unit = match clicks {
            0 | 1 => Unit::Character,
            2 => Unit::Word,
            _ => Unit::Line,
        };
        let anchor = unit.span(screen, top, row, x);
        Self {
            head: anchor.clone(),
            anchor,
            unit,
            dragging: true,
        }
    }

    pub(super) fn extend(&mut self, screen: &Screen, top: usize, row: u16, x: f32) {
        self.head = self.unit.span(screen, top, row, x);
    }

    fn range(&self) -> Range<Position> {
        self.anchor.start.min(self.head.start)..self.anchor.end.max(self.head.end)
    }

    pub(super) fn is_empty(&self) -> bool {
        self.range().is_empty()
    }

    pub(super) fn evicted(&self, discarded: usize) -> bool {
        self.range().end.row < discarded
    }

    pub(super) fn columns(&self, row: usize, cols: u16) -> Range<u16> {
        let range = self.range();
        if row < range.start.row || row > range.end.row {
            return 0..0;
        }
        let start = if row == range.start.row {
            range.start.col.min(cols)
        } else {
            0
        };
        let end = if row == range.end.row {
            range.end.col.min(cols)
        } else {
            cols
        };
        start..end
    }

    pub(super) fn text(&self, screen: &mut Screen, discarded: usize) -> Option<String> {
        let range = self.range();
        let (rows, cols) = screen.size();
        let live = discarded + screen.scrollback_len();
        let start = range.start.max(Position {
            row: discarded,
            col: 0,
        });
        let end = range.end.min(Position {
            row: live + usize::from(rows.get()) - 1,
            col: cols.get(),
        });
        if start >= end {
            return None;
        }
        // vt100 exposes one viewport at a time. Copy in pages, restoring the user's
        // viewport before returning. Soft-wrapped rows belong to the same line.
        let original = screen.scrollback();
        let mut text = String::new();
        let mut next = start;
        loop {
            screen.set_scrollback(live.saturating_sub(next.row));
            let top = top_row(screen, discarded);
            let last = end.row.min(top + usize::from(rows.get()) - 1);
            let last_row = (last - top) as u16;
            text.push_str(&screen.contents_between(
                (next.row - top) as u16,
                next.col,
                last_row,
                if last == end.row { end.col } else { cols.get() },
            ));
            if last == end.row {
                break;
            }
            if !screen.row_wrapped(last_row) {
                text.push('\n');
            }
            next = Position {
                row: last + 1,
                col: 0,
            };
        }
        screen.set_scrollback(original);
        Some(text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parser(rows: u16, cols: u16) -> vt100::Parser {
        vt100::Parser::new(rows.try_into().unwrap(), cols.try_into().unwrap(), 8)
    }

    #[test]
    fn reverse_drag_copies_wide_and_combining_cells_without_padding() {
        let mut term = parser(3, 12);
        term.process("ab界e\u{301}!\r\nbeta".as_bytes());
        let screen = term.screen_mut();
        let mut sel = Selection::start(screen, 0, 1, 2., 1);
        sel.extend(screen, 0, 0, 2.8);
        assert_eq!(sel.columns(0, 12), 2..12);
        assert_eq!(sel.columns(1, 12), 0..2);
        assert_eq!(sel.columns(2, 12), 0..0);
        assert_eq!(sel.text(screen, 0).as_deref(), Some("界e\u{301}!\nbe"));
        sel.extend(screen, 0, 0, 3.2);
        assert_eq!(sel.columns(0, 12), 4..12);
    }

    #[test]
    fn click_word_line_and_shift_extension_use_cell_boundaries() {
        let mut term = parser(2, 16);
        term.process("a界b::two_2".as_bytes());
        let screen = term.screen_mut();
        assert!(Selection::start(screen, 0, 0, 1., 1).is_empty());
        let mut word = Selection::start(screen, 0, 0, 2.4, 2);
        assert_eq!(word.text(screen, 0).as_deref(), Some("a界b"));
        word.extend(screen, 0, 0, 7.);
        assert_eq!(word.text(screen, 0).as_deref(), Some("a界b::two_2"));
        let line = Selection::start(screen, 0, 0, 8., 3);
        assert_eq!(line.text(screen, 0).as_deref(), Some("a界b::two_2"));
    }

    #[test]
    fn copying_across_scrollback_joins_soft_wraps_and_restores_the_viewport() {
        let mut term = parser(2, 5);
        term.process(b"abcdef\r\nB\r\nC\r\nD");
        let screen = term.screen_mut();
        screen.set_scrollback(usize::MAX);
        let mut sel = Selection::start(screen, top_row(screen, 0), 0, 2., 1);
        screen.set_scrollback(0);
        sel.extend(screen, top_row(screen, 0), 1, 1.);
        screen.set_scrollback(1);
        assert_eq!(sel.text(screen, 0).as_deref(), Some("cdef\nB\nC\nD"));
        assert_eq!(screen.scrollback(), 1);
        assert!(!sel.evicted(0));
        assert!(sel.evicted(5));
    }
}
