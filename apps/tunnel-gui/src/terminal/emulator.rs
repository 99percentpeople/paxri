use super::display::{COL, LINE};
use super::selection::Unit;
use crate::theme::TerminalPalette;
use alacritty_terminal::{
    Grid, Term,
    event::{Event, EventListener, WindowSize},
    grid::{Dimensions, Scroll},
    index::{Column, Line, Point},
    term::{
        Config, Osc52, TermMode,
        cell::{Cell, Flags, LineLength},
    },
    vte::ansi::{Color, CursorStyle, NamedColor, Processor, Rgb},
};
use portable_pty::PtySize;
use std::{cell::RefCell, rc::Rc};
mod redraw;
use redraw::{Boundary, Redraw};

#[derive(Clone, Copy)]
pub(super) struct GridSize {
    pub rows: u16,
    pub cols: u16,
}

impl GridSize {
    pub fn pty(self) -> PtySize {
        PtySize {
            rows: self.rows,
            cols: self.cols,
            pixel_width: self.cols * COL as u16,
            pixel_height: self.rows * LINE as u16,
        }
    }
}

impl Dimensions for GridSize {
    fn columns(&self) -> usize {
        self.cols.into()
    }
    fn screen_lines(&self) -> usize {
        self.rows.into()
    }
    fn total_lines(&self) -> usize {
        self.screen_lines()
    }
}

// A viewport can start halfway through a soft-wrapped line. Measure that
// position relative to the live cursor so eviction at the head of history
// does not move it when a narrower window creates more physical rows.
#[derive(Clone, Copy)]
struct RowAnchor {
    hard_breaks_to_cursor: i32,
    cells_into_line: usize,
}

impl RowAnchor {
    fn wrapped_width(grid: &Grid<Cell>, line: Line) -> Option<usize> {
        let flags = grid[line][grid.last_column()].flags;
        flags
            .contains(Flags::WRAPLINE)
            .then(|| grid.columns() - usize::from(flags.contains(Flags::LEADING_WIDE_CHAR_SPACER)))
    }

    fn at(grid: &Grid<Cell>, line: Line) -> Self {
        Self::at_from(grid, line, grid.cursor.point.line)
    }

    fn at_from(grid: &Grid<Cell>, line: Line, from: Line) -> Self {
        let breaks = (line.min(from).0..line.max(from).0)
            .filter(|&row| Self::wrapped_width(grid, Line(row)).is_none())
            .count() as i32;
        let cells_into_line = (grid.topmost_line().0..line.0)
            .rev()
            .map_while(|row| Self::wrapped_width(grid, Line(row)))
            .sum();
        Self {
            hard_breaks_to_cursor: if line <= from { breaks } else { -breaks },
            cells_into_line,
        }
    }

    fn resolve(&self, grid: &Grid<Cell>) -> Line {
        self.resolve_from(grid, grid.cursor.point.line)
    }

    fn resolve_from(&self, grid: &Grid<Cell>, from: Line) -> Line {
        let mut line = from.clamp(grid.topmost_line(), grid.bottommost_line());
        let mut breaks = self.hard_breaks_to_cursor;
        if breaks < 0 {
            while breaks < 0 && line < grid.bottommost_line() {
                if Self::wrapped_width(grid, line).is_none() {
                    breaks += 1;
                }
                line += 1;
            }
        } else {
            while line > grid.topmost_line() {
                if Self::wrapped_width(grid, line - 1i32).is_none() {
                    if breaks == 0 {
                        break;
                    }
                    breaks -= 1;
                }
                line -= 1;
            }
        }
        if breaks == 0 {
            let mut offset = self.cells_into_line;
            while let Some(width) = Self::wrapped_width(grid, line) {
                if offset < width || line >= grid.bottommost_line() {
                    break;
                }
                offset -= width;
                line += 1;
            }
        }
        line
    }
}

// Resizing from a cursor in the middle of a command must also retain the
// wrapped suffix below it. Let the grid keep the whole logical line in view,
// then restore the insertion position by counting terminal cells.
struct CursorAnchor(usize);

impl CursorAnchor {
    fn capture(term: &mut Term<Replies>) -> Self {
        let cursor = &term.grid().cursor;
        let point = cursor.point;
        let after = point.column.0 + usize::from(cursor.input_needs_wrap);
        let offset = RowAnchor::at(term.grid(), point.line).cells_into_line + after;
        let end = term.line_search_right(point).line;
        let grid = term.grid_mut();
        let row = &grid[end];
        let mut length = row.line_length().0;
        if length > 0 && row[Column(length - 1)].flags.contains(Flags::WIDE_CHAR) {
            length += 1;
        }
        if end == point.line {
            length = length.max(after);
        }
        grid.cursor.point = Point::new(end, Column(length.min(grid.last_column().0)));
        grid.cursor.input_needs_wrap = length >= grid.columns();
        Self(offset)
    }

    fn restore(self, term: &mut Term<Replies>) {
        let mut line = term.line_search_left(term.grid().cursor.point).line;
        let grid = term.grid_mut();
        let mut offset = self.0;
        while let Some(width) = RowAnchor::wrapped_width(grid, line) {
            if offset < width || line >= grid.bottommost_line() {
                break;
            }
            offset -= width;
            line += 1;
        }
        grid.cursor.point = Point::new(line.max(Line(0)), Column(offset.min(grid.last_column().0)));
        grid.cursor.input_needs_wrap = offset >= grid.columns();
    }
}

#[derive(Clone, Default)]
pub(super) struct Replies(Rc<RefCell<Vec<Event>>>);

impl EventListener for Replies {
    fn send_event(&self, event: Event) {
        if matches!(
            event,
            Event::PtyWrite(_) | Event::TextAreaSizeRequest(_) | Event::ColorRequest(_, _)
        ) {
            self.0.borrow_mut().push(event);
        }
    }
}

pub(super) struct Emulator {
    pub term: Term<Replies>,
    pub palette: &'static TerminalPalette,
    default_cursor: CursorStyle,
    pub selection_unit: Unit,
    parser: Processor,
    replies: Replies,
    last_primary_top: Option<RowAnchor>,
    resized_in_alt: bool,
    redraw: Redraw,
}

impl Emulator {
    fn config(default_cursor_style: CursorStyle) -> Config {
        Config {
            scrolling_history: 2000,
            osc52: Osc52::Disabled,
            default_cursor_style,
            ..Config::default()
        }
    }

    #[cfg(test)]
    pub fn new(size: GridSize) -> Self {
        Self::with_palette(
            size,
            &crate::theme::Appearance::Dark
                .resolve(gpui::WindowAppearance::Dark)
                .terminal,
        )
    }

    pub fn with_palette(size: GridSize, palette: &'static TerminalPalette) -> Self {
        let replies = Replies::default();
        let default_cursor = super::cursor::default_style(Default::default());
        Self {
            term: Term::new(Self::config(default_cursor), &size, replies.clone()),
            palette,
            default_cursor,
            selection_unit: Unit::Character,
            parser: Processor::new(),
            replies,
            last_primary_top: None,
            resized_in_alt: false,
            redraw: Redraw::default(),
        }
    }

    pub fn set_default_cursor(&mut self, style: CursorStyle) {
        if self.default_cursor != style {
            self.default_cursor = style;
            self.term.set_options(Self::config(style));
        }
    }

    pub fn process(&mut self, mut bytes: &[u8]) {
        while !bytes.is_empty() {
            let (used, event) = self.redraw.next(bytes);
            let (chunk, rest) = bytes.split_at(used);
            bytes = rest;
            let Some(event) = event else {
                self.parser.advance(&mut self.term, chunk);
                continue;
            };
            if matches!(event, Boundary::ColorQuery) {
                // Resolve at the query, even if a later OSC in this same read resets the slot.
                let first = self.replies.0.borrow().len();
                self.parser.advance(&mut self.term, chunk);
                self.flush_sync();
                for event in &mut self.replies.0.borrow_mut()[first..] {
                    if let Event::ColorRequest(index, format) = event {
                        let name = match *index {
                            i if i == NamedColor::Foreground as usize => NamedColor::Foreground,
                            i if i == NamedColor::Background as usize => NamedColor::Background,
                            i if i == NamedColor::Cursor as usize => NamedColor::Cursor,
                            _ => continue,
                        };
                        let value = super::display::color(
                            Color::Named(name),
                            self.palette,
                            self.term.colors(),
                        );
                        let [_, r, g, b] = value.to_be_bytes();
                        *event = Event::PtyWrite(format(Rgb { r, g, b }));
                    }
                }
                continue;
            }
            let (final_byte, prefix) = chunk.split_last().unwrap();
            self.parser.advance(&mut self.term, prefix);
            self.flush_sync();
            let editing = self.term.mode().contains(TermMode::BRACKETED_PASTE)
                && !self.term.mode().contains(TermMode::ALT_SCREEN);
            match event {
                Boundary::Up if editing => self.redraw.start_up(self.term.grid()),
                Boundary::Erase(up) if editing => self.redraw.erase(up, self.term.grid_mut()),
                _ => self.redraw.clear(),
            }
            self.parser
                .advance(&mut self.term, std::slice::from_ref(final_byte));
        }
    }

    pub fn input_started(&mut self) {
        self.redraw.clear();
    }

    // GPUI paints only after the batch. Materialize pending VT operations before
    // observing cursor coordinates or changing the geometry beneath those bytes.
    fn flush_sync(&mut self) {
        if self.parser.sync_timeout().sync_timeout().is_some() {
            self.parser.stop_sync(&mut self.term);
        }
    }

    pub fn finish_batch(&mut self) -> Vec<u8> {
        self.flush_sync();
        if !self.term.mode().contains(TermMode::ALT_SCREEN) {
            if self.resized_in_alt {
                self.resized_in_alt = false;
                let grid = self.term.grid();
                let prompt_start = self.term.line_search_left(grid.cursor.point).line;
                let first = self
                    .last_primary_top
                    .as_ref()
                    .map_or(prompt_start, |anchor| {
                        anchor.resolve(grid).min(prompt_start)
                    });
                self.reveal_using_blank_rows(first);
            }
            self.last_primary_top = Some(RowAnchor::at(self.term.grid(), Line(0)));
        }
        let mut bytes = Vec::new();
        for event in self.replies.0.borrow_mut().drain(..) {
            let reply = match event {
                Event::PtyWrite(text) => text,
                Event::TextAreaSizeRequest(format) => format(WindowSize {
                    num_cols: self.term.columns() as u16,
                    num_lines: self.term.screen_lines() as u16,
                    cell_width: COL as u16,
                    cell_height: LINE as u16,
                }),
                _ => continue,
            };
            bytes.extend_from_slice(reply.as_bytes());
        }
        bytes
    }

    pub fn resize(&mut self, size: GridSize) -> bool {
        let grid = self.term.grid();
        if grid.columns() == size.columns() && grid.screen_lines() == size.screen_lines() {
            return false;
        }
        self.flush_sync();
        let grid = self.term.grid();
        let width_changed = grid.columns() != size.columns();
        let alternate = self.term.mode().contains(TermMode::ALT_SCREEN);
        let pending_motion = self.redraw.motion_anchor(grid);
        if width_changed && !alternate && self.term.mode().contains(TermMode::BRACKETED_PASTE) {
            self.redraw.resizing(grid);
        }
        let live = (width_changed && !alternate).then(|| RowAnchor::at(grid, Line(0)));
        let offset = grid.display_offset();
        let at_top = offset > 0 && offset == grid.history_size();
        let reading = (offset > 0 && !at_top).then(|| RowAnchor::at(grid, self.view_line(0)));
        self.resized_in_alt |= width_changed && alternate;
        let cursor = (!alternate).then(|| CursorAnchor::capture(&mut self.term));
        self.term.resize(size);
        if let Some(anchor) = live {
            self.reveal_using_blank_rows(anchor.resolve(self.term.grid()));
        }
        if let Some(cursor) = cursor {
            cursor.restore(&mut self.term);
        }
        self.redraw.restore_motion(self.term.grid(), pending_motion);
        if at_top {
            self.scroll(Scroll::Top);
        } else if let Some(anchor) = reading {
            let line = anchor.resolve(self.term.grid());
            let delta = -line.0 - self.term.grid().display_offset() as i32;
            self.scroll(Scroll::Delta(delta));
        }
        if !alternate {
            self.last_primary_top = Some(RowAnchor::at(self.term.grid(), Line(0)));
        }
        true
    }

    // Reflow can move the old first row into history even when the window has
    // room below the prompt. Exchange only trailing empty rows for that text.
    fn reveal_using_blank_rows(&mut self, first: Line) {
        if first.0 >= 0 {
            return;
        }
        let grid = self.term.grid_mut();
        let rows = grid.screen_lines();
        let cols = grid.columns();
        let protected = grid.cursor.point.line.max(grid.saved_cursor.point.line);
        let available = (protected.0 + 1..rows as i32)
            .rev()
            .take((-first.0) as usize)
            .take_while(|&row| grid[Line(row)].is_clear())
            .count();
        if available > 0 {
            grid.resize(true, rows - available, cols);
            grid.resize(true, rows, cols);
        }
    }

    pub fn view_line(&self, row: usize) -> Line {
        Line(row as i32 - self.term.grid().display_offset() as i32)
    }

    pub fn scroll(&mut self, scroll: Scroll) {
        if !self.term.mode().contains(TermMode::ALT_SCREEN) {
            self.term.scroll_display(scroll);
        }
    }

    pub fn text_between(&self, start: Point, mut end: Point) -> String {
        // A wide glyph that wrapped before the last column starts on the next
        // row. Its leading spacer alone contributes no character to a copy.
        if self.term.grid()[end]
            .flags
            .contains(Flags::LEADING_WIDE_CHAR_SPACER)
        {
            if start == end {
                return String::new();
            }
            end.column -= 1;
        }
        self.term.bounds_to_string(start, end)
    }

    pub fn visible_text(&self) -> String {
        let start = Point::new(self.view_line(0), Column(0));
        let end = Point::new(
            self.view_line(self.term.screen_lines() - 1),
            self.term.last_column(),
        );
        self.text_between(start, end).trim_end_matches('\n').into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alacritty_terminal::vte::ansi::{Color, NamedColor};

    fn grid(rows: u16, cols: u16) -> GridSize {
        GridSize { rows, cols }
    }

    fn buffer_text(em: &Emulator) -> String {
        em.text_between(
            Point::new(em.term.topmost_line(), Column(0)),
            Point::new(em.term.bottommost_line(), em.term.last_column()),
        )
        .trim_end_matches('\n')
        .into()
    }

    #[test]
    fn resizing_a_repainted_multiline_prompt_keeps_history_and_one_right_prompt() {
        // The line editor draws a full-width header, then uses its previous physical
        // line count to erase it. Repaints for older widths can arrive after resizing.
        let header = |width: usize| format!("~{:>w$}\r\n❯ echo draft", "08:03:56", w = width - 2);
        let mut em = Emulator::new(grid(10, 36));
        let history = "saved output with a long line and 宽字符 at the end";
        em.process(format!("{history}\r\n{}\x1b[?2004h", header(36)).as_bytes());
        let expected = buffer_text(&em);
        let repaint = |w| format!("\r\r\x1b[A\x1b[0m\x1b[J{}", header(w));
        for width in [19, 32, 24, 36] {
            em.resize(grid(10, width));
        }
        for width in [19, 32, 24, 36] {
            for byte in repaint(width).as_bytes() {
                em.process(std::slice::from_ref(byte));
            }
        }
        assert_eq!(buffer_text(&em), expected);
        // The window can change while the new repaint is split across network reads.
        em.process(b"\r\x1b[A");
        em.resize(grid(10, 17));
        em.process(format!("\x1b[0m\x1b[J{}", header(17)).as_bytes());
        em.resize(grid(10, 36));
        em.process(format!("\x1b[?2026h{}\x1b[?2026l", repaint(36)).as_bytes());
        assert_eq!(buffer_text(&em), expected);
    }

    #[test]
    fn cursor_up_before_carriage_return_counts_all_rows_of_an_edited_command() {
        let mut em = Emulator::new(grid(10, 18));
        let cmd = "echo abcdefghijklmnopqrst";
        let prompt = |w: usize| format!("~{:>pad$}\r\n> {cmd}", "clock", pad = w - 2);
        em.process(format!("keep\r\n\r\n{}\x1b[?2004h", prompt(18)).as_bytes());
        let expected = buffer_text(&em);
        em.resize(grid(10, 12));
        em.process(format!("\x1b[A\r\r\x1b[A\x1b[A\x1b[J\r\n{}", prompt(12)).as_bytes());
        em.resize(grid(10, 18));
        em.process(format!("\x1b[2A\r\r\x1b[2A\x1b[J\r\n{}", prompt(18)).as_bytes());
        assert_eq!(buffer_text(&em), expected);
        em.process(b"\x1b[?2004l\r\noutput\r\n");
        assert!(buffer_text(&em).ends_with("output"));
    }

    #[test]
    fn long_output_and_partially_edited_prompt_survive_round_trip() {
        let mut em = Emulator::new(grid(9, 26));
        let output = "abcde界e\u{301}0123456789----";
        let prompt = "dev:/working/directory $ edit file";
        em.process(format!("\x1b[31m{output}\x1b[0m\r\n\r\n{prompt}\x1b[4D").as_bytes());
        let expected = format!("{output}\n\n{prompt}");
        for (rows, cols) in [(9, 6), (6, 17), (12, 43), (9, 26)] {
            em.resize(grid(rows, cols));
            assert_eq!(buffer_text(&em), expected);
            let screen = em.term.grid();
            let cell = (screen.topmost_line().0..=screen.bottommost_line().0)
                .flat_map(|row| screen[Line(row)][..].iter())
                .find(|cell| cell.c == '界')
                .unwrap();
            assert_eq!(cell.fg, Color::Named(NamedColor::Red));
            assert!(cell.flags.contains(Flags::WIDE_CHAR));
        }
        em.process(b"FILE");
        assert_eq!(
            em.visible_text(),
            expected.replace("edit file", "edit FILE")
        );
    }

    #[test]
    fn resizing_with_the_caret_inside_a_wide_prompt_preserves_the_suffix() {
        for (prompt, initial, intermediate) in [("ab界> ABCD", 6, 9), ("ab界尾> ABCD", 13, 5)] {
            let mut em = Emulator::new(grid(5, initial));
            let text = format!("one\r\ntwo\r\nthree\r\n{prompt}");
            em.process(text.as_bytes());
            em.process(b"\x1b[2D");
            em.resize(grid(5, intermediate));
            em.resize(grid(3, 6));
            let expected = text.replace("\r\n", "\n");
            assert_eq!(buffer_text(&em), expected);
            em.process(b"XY");
            assert_eq!(buffer_text(&em), expected.replace("ABCD", "ABXY"));
        }
    }

    #[test]
    fn pending_wrap_and_incomplete_input_survive_resize() {
        let mut em = Emulator::new(grid(4, 8));
        em.process(b"12345678");
        em.resize(grid(4, 4));
        em.resize(grid(4, 8));
        let glyph = "界".as_bytes();
        em.process(&glyph[..1]);
        em.resize(grid(4, 9));
        em.process(&glyph[1..]);
        em.process(b"\x1b[3");
        em.resize(grid(4, 20));
        em.process(b"1mR\x1b[6n\x1b[5n\x1b[18t\x1b[14t");
        assert_eq!(em.visible_text(), "12345678界R");
        assert_eq!(
            em.term.grid()[Line(0)][Column(10)].fg,
            Color::Named(NamedColor::Red)
        );
        assert_eq!(
            em.finish_batch(),
            b"\x1b[1;12R\x1b[0n\x1b[8;4;20t\x1b[4;80;180t"
        );
    }

    #[test]
    fn history_reflows_and_stays_scrolled_when_reading_earlier_output() {
        let mut em = Emulator::new(grid(5, 16));
        let lines: Vec<_> = (0..24)
            .map(|n| format!("row-{n:02}-abcdef界-012345"))
            .collect();
        em.process(lines.join("\r\n").as_bytes());
        em.scroll(Scroll::Top);
        for (rows, cols) in [(4, 7), (7, 32), (5, 16)] {
            em.resize(grid(rows, cols));
            assert_eq!(buffer_text(&em), lines.join("\n"));
            assert!(em.term.grid().display_offset() > 0);
            assert!(em.visible_text().starts_with("row-00-"));
        }
        em.scroll(Scroll::Bottom);
        assert!(em.visible_text().ends_with(&lines[23]));
        assert_eq!(em.term.grid().display_offset(), 0);
    }

    #[test]
    fn scrolled_view_tracks_the_same_logical_line_and_returns_to_live_output() {
        let mut em = Emulator::new(grid(5, 18));
        let rows: Vec<_> = (0..200)
            .map(|n| format!("row-{n:03} repeated long line"))
            .collect();
        em.process(rows.join("\r\n").as_bytes());
        em.scroll(Scroll::Delta(41));
        let label = "row-177";
        assert!(em.visible_text().starts_with(label));
        for (height, width) in [(4, 9), (6, 35), (5, 18)] {
            em.resize(grid(height, width));
            assert!(em.visible_text().starts_with(label));
        }
        em.scroll(Scroll::Bottom);
        assert!(em.visible_text().ends_with(&rows[199]));
    }

    #[test]
    fn fullscreen_keeps_screen_coordinates_and_reflows_saved_prompt() {
        let mut em = Emulator::new(grid(8, 24));
        let prompt = "desk:~/workspace $ unsubmitted command";
        em.process(format!("old output\r\n{prompt}").as_bytes());
        em.finish_batch();
        em.process(b"\x1b[?1049h\x1b[2J\x1b[HABCDEFGHIJK\x1b[2;1Hrow2");
        em.resize(grid(8, 8));
        assert_eq!(em.visible_text(), "ABCDEFGH\nrow2");
        assert_eq!(em.term.history_size(), 0);
        em.process(b"\x1b[?1049l");
        em.finish_batch();
        assert_eq!(em.visible_text(), format!("old output\n{prompt}"));
        em.process(b"!");
        assert_eq!(em.visible_text(), format!("old output\n{prompt}!"));
    }
}
