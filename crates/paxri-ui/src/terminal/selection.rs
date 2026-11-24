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
    TouchDrag { leading: bool, side: Side },
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

    pub fn start_touch_selection(&mut self, row: usize, x: f32) {
        let (point, side) = self.hit(row, x);
        self.selection_unit = if self.word_class(point) == WordClass::Space {
            Unit::Character
        } else {
            Unit::Word
        };
        let mut selection = Selection::new(SelectionType::Simple, point, side);
        selection.include_all();
        self.term.selection = Some(selection);
    }

    pub fn extend_touch_selection(&mut self, row: usize, x: f32) {
        let (point, side) = self.hit(row, x);
        let Some(selection) = self.term.selection.as_mut() else {
            return;
        };
        selection.update(point, side);
        // Keep both touched cells in the raw range. Alacritty tracks the original
        // cell through scrolling, including when the finger crosses back over it.
        selection.include_all();
        if let Some(range) = self.raw_selection_range() {
            let leading = if range.start == range.end {
                side == Side::Left
            } else {
                point == range.start
            };
            self.selection_unit = Unit::TouchDrag { leading, side };
        }
    }

    pub fn move_selection_handle(&mut self, leading: bool, row: usize, x: f32) {
        let Some(mut range) = self.selected_range() else {
            return;
        };
        let (point, side) = self.hit(row, x);
        self.place_selection_edge(&mut range, leading, point, side);
        self.selection_unit = Unit::Character;
        let mut selection = Selection::new(SelectionType::Simple, range.start, Side::Left);
        selection.update(range.end, Side::Right);
        self.term.selection = Some(selection);
    }

    fn place_selection_edge(
        &self,
        range: &mut SelectionRange,
        leading: bool,
        point: Point,
        side: Side,
    ) {
        use alacritty_terminal::index::Boundary;
        if leading {
            let first = if side == Side::Right {
                point.add(&self.term, Boundary::Grid, 1)
            } else {
                point
            };
            range.start = first.min(range.end);
        } else {
            let last = if side == Side::Left {
                point.sub(&self.term, Boundary::Grid, 1)
            } else {
                point
            };
            range.end = last.max(range.start);
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

    fn word_start(&self, mut point: Point) -> Point {
        let class = self.word_class(point);
        while point.column.0 > 0 {
            let prev = Point::new(point.line, point.column - 1);
            if self.word_class(prev) != class {
                break;
            }
            point = prev;
        }
        point
    }

    fn word_end(&self, mut point: Point) -> Point {
        let class = self.word_class(point);
        while point.column < self.term.last_column() {
            let next = Point::new(point.line, point.column + 1);
            if self.word_class(next) != class {
                break;
            }
            point = next;
        }
        point
    }

    fn raw_selection_range(&self) -> Option<SelectionRange> {
        self.term.selection.as_ref()?.to_range(&self.term)
    }

    pub fn selected_range(&self) -> Option<SelectionRange> {
        let mut range = self.raw_selection_range()?;
        let last = self.term.last_column();
        match self.selection_unit {
            Unit::Character => {}
            Unit::Line => {
                range.start.column = Column(0);
                range.end.column = last;
            }
            Unit::Word => {
                range.start = self.word_start(range.start);
                range.end = self.word_end(range.end);
            }
            Unit::TouchDrag { leading, side } => {
                let finger = if leading { range.start } else { range.end };
                if leading && self.word_class(range.end) != WordClass::Space {
                    range.end = self.word_end(range.end);
                } else if !leading && self.word_class(range.start) != WordClass::Space {
                    range.start = self.word_start(range.start);
                }
                self.place_selection_edge(&mut range, leading, finger, side);
            }
        }
        let grid = self.term.grid();
        if grid[range.start].flags.contains(Flags::WIDE_CHAR_SPACER) {
            range.start.column.0 = range.start.column.0.saturating_sub(1);
        }
        if grid[range.end].flags.contains(Flags::WIDE_CHAR) {
            range.end.column.0 = (range.end.column.0 + 1).min(grid.last_column().0);
        }
        Some(range)
    }

    /// Bottom edges of the selected cells relative to the terminal viewport.
    pub fn selection_handles(&self) -> Option<[gpui::Point<f32>; 2]> {
        let range = self.selected_range()?;
        let cell = self.font.cell;
        let offset = self.term.grid().display_offset() as i32;
        let at = |point: Point, after: usize| {
            gpui::point(
                (point.column.0 + after) as f32 * cell.width,
                (point.line.0 + offset + 1) as f32 * cell.height,
            )
        };
        Some([at(range.start, 0), at(range.end, 1)])
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
    fn first_touch_drag_follows_the_finger_through_words_and_line_padding() {
        let mut em = term(3, 24, "aa bravo tail\r\n中文字符");
        em.start_touch_selection(0, 5.);
        assert_eq!(em.selected_text().as_deref(), Some("bravo"));
        em.extend_touch_selection(0, 11.);
        assert_eq!(em.selected_text().as_deref(), Some("bravo ta"));
        em.extend_touch_selection(0, 16.);
        assert_eq!(
            em.selected_range().unwrap().end,
            Point::new(Line(0), Column(15))
        );
        em.extend_touch_selection(0, 1.);
        assert_eq!(em.selected_text().as_deref(), Some("a bravo"));
        em.extend_touch_selection(1, 4.);
        assert_eq!(em.selected_text().as_deref(), Some("bravo tail\n中文"));

        em.start_touch_selection(0, 18.);
        let blank = em.selected_range().unwrap();
        assert_eq!(blank.start, Point::new(Line(0), Column(18)));
        assert_eq!(blank.end, blank.start);
        em.extend_touch_selection(0, 21.);
        assert_eq!(
            em.selected_range().unwrap().end,
            Point::new(Line(0), Column(20))
        );
    }

    #[test]
    fn handles_adjust_word_selection_at_character_boundaries() {
        let mut em = term(3, 12, "a界bc\r\nsecond");
        em.start_selection(0, 1., 2);
        em.move_selection_handle(true, 0, 1.);
        assert_eq!(em.selected_text().as_deref(), Some("界bc"));
        em.move_selection_handle(false, 1, 3.);
        assert_eq!(em.selected_text().as_deref(), Some("界bc\nsec"));
        em.move_selection_handle(false, 0, 0.);
        assert_eq!(em.selected_text().as_deref(), Some("界"));
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
