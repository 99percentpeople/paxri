use super::emulator::Emulator;
use alacritty_terminal::{
    grid::Dimensions,
    index::{Column, Point, Side},
    selection::{Selection, SelectionRange, SelectionType},
    term::cell::Flags,
};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Unit {
    Character,
    Word,
    Line,
}

#[derive(PartialEq, Eq)]
enum WordClass {
    Space,
    Word,
    Punctuation(char),
}

impl Emulator {
    fn hit(&self, row: usize, x: f32) -> (Point, Side) {
        let grid = self.term.grid();
        let mut p = Point::new(
            self.view_line(row),
            Column((x.max(0.) as usize).min(grid.last_column().0)),
        );
        if grid[p].flags.contains(Flags::WIDE_CHAR_SPACER) {
            p.column.0 = p.column.0.saturating_sub(1);
        }
        let wide = grid[p].flags.contains(Flags::WIDE_CHAR);
        let width = if wide { 2 } else { 1 };
        if x < p.column.0 as f32 + width as f32 / 2. {
            (p, Side::Left)
        } else {
            p.column.0 = (p.column.0 + width - 1).min(grid.last_column().0);
            (p, Side::Right)
        }
    }

    pub fn start_selection(&mut self, row: usize, x: f32, clicks: usize) {
        let (point, side) = self.hit(row, x);
        self.selection_unit = match clicks {
            0 | 1 => Unit::Character,
            2 => Unit::Word,
            _ => Unit::Line,
        };
        let mut selection = Selection::new(SelectionType::Simple, point, side);
        if self.selection_unit != Unit::Character {
            selection.include_all();
        }
        self.term.selection = Some(selection);
    }

    pub fn extend_selection(&mut self, row: usize, x: f32) {
        let (point, side) = self.hit(row, x);
        if let Some(selection) = &mut self.term.selection {
            selection.update(point, side);
            if self.selection_unit != Unit::Character {
                selection.include_all();
            }
        }
    }

    fn word_class(&self, mut point: Point) -> WordClass {
        let grid = self.term.grid();
        if grid[point].flags.contains(Flags::WIDE_CHAR_SPACER) {
            point.column.0 = point.column.0.saturating_sub(1);
        }
        match grid[point].c {
            c if c.is_whitespace() => WordClass::Space,
            c if c.is_alphanumeric() || c == '_' => WordClass::Word,
            c => WordClass::Punctuation(c),
        }
    }

    pub fn selected_range(&self) -> Option<SelectionRange> {
        let mut range = self.term.selection.as_ref()?.to_range(&self.term)?;
        let last = self.term.last_column();
        match self.selection_unit {
            Unit::Character => {}
            Unit::Line => {
                range.start.column = Column(0);
                range.end.column = last;
            }
            Unit::Word => {
                let start = self.word_class(range.start);
                let end = self.word_class(range.end);
                while range.start.column.0 > 0 {
                    let prev = Point::new(range.start.line, range.start.column - 1);
                    if self.word_class(prev) != start {
                        break;
                    }
                    range.start = prev;
                }
                while range.end.column < last {
                    let next = Point::new(range.end.line, range.end.column + 1);
                    if self.word_class(next) != end {
                        break;
                    }
                    range.end = next;
                }
            }
        }
        Some(range)
    }

    pub fn selected_text(&self) -> Option<String> {
        let range = self.selected_range()?;
        Some(self.text_between(range.start, range.end))
    }
}

#[cfg(test)]
mod tests {
    use super::super::emulator::GridSize;
    use super::*;
    use alacritty_terminal::grid::Scroll;
    use alacritty_terminal::index::Line;

    fn term(rows: u16, cols: u16, text: &str) -> Emulator {
        let mut em = Emulator::new(GridSize { rows, cols });
        em.process(text.as_bytes());
        em
    }

    #[test]
    fn reverse_drag_copies_wide_and_combining_cells_without_padding() {
        let mut em = term(3, 12, "ab界e\u{301}!\r\nbeta");
        em.start_selection(1, 2., 1);
        em.extend_selection(0, 2.8);
        let selected = em.selected_range().unwrap();
        assert_eq!(selected.start, Point::new(Line(0), Column(2)));
        assert_eq!(selected.end, Point::new(Line(1), Column(1)));
        assert_eq!(em.selected_text().as_deref(), Some("界e\u{301}!\nbe"));
        em.extend_selection(0, 3.2);
        assert_eq!(em.selected_range().unwrap().start.column, Column(4));
    }

    #[test]
    fn click_word_line_and_shift_extension_use_cell_boundaries() {
        let mut em = term(2, 16, "a界b::two_2");
        em.start_selection(0, 1., 1);
        assert!(em.selected_range().is_none());
        em.start_selection(0, 2.4, 2);
        assert_eq!(em.selected_text().as_deref(), Some("a界b"));
        em.extend_selection(0, 7.);
        assert_eq!(em.selected_text().as_deref(), Some("a界b::two_2"));
        em.start_selection(0, 4., 2);
        assert_eq!(em.selected_text().as_deref(), Some("::"));
        em.start_selection(0, 8., 3);
        assert_eq!(em.selected_text().as_deref(), Some("a界b::two_2"));
    }

    #[test]
    fn copying_across_scrollback_joins_soft_wraps_and_restores_the_viewport() {
        let mut em = term(2, 5, "abcdef\r\nB\r\nC\r\nD");
        em.scroll(Scroll::Top);
        em.start_selection(0, 2., 1);
        em.scroll(Scroll::Bottom);
        em.extend_selection(1, 1.);
        em.scroll(Scroll::Delta(1));
        assert_eq!(em.selected_text().as_deref(), Some("cdef\nB\nC\nD"));
        assert_eq!(em.term.grid().display_offset(), 1);
        for _ in 0..2010 {
            em.process(b"\r\nx");
        }
        assert!(em.selected_text().is_none());
    }
}
