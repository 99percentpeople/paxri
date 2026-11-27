use crate::theme::fonts::{CellSize, TerminalFont};
use super::selection::Unit;
use crate::theme::TerminalPalette;
use alacritty_terminal::{
    Grid, Term,
    event::{Event, EventListener, WindowSize},
    grid::{Dimensions, Scroll},
    index::{Column, Line, Point},
    term::{
        ClipboardType, Config, Osc52, TermMode,
        cell::{Cell, Flags, LineLength},
    },
    vte::ansi::{Color, CursorStyle, NamedColor, Processor, Rgb},
};
use std::{cell::RefCell, rc::Rc};
mod redraw;
use redraw::{Boundary, Redraw};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TerminalSize {
    pub rows: u16,
    pub cols: u16,
    pub pixel_width: u16,
    pub pixel_height: u16,
}

#[derive(Clone, Copy)]
pub struct GridSize {
    pub rows: u16,
    pub cols: u16,
}

impl GridSize {
    pub fn from_viewport(viewport: gpui::Size<gpui::Pixels>, cell: CellSize) -> Self {
        Self {
            cols: (f32::from(viewport.width) / cell.width)
                .clamp(alacritty_terminal::term::MIN_COLUMNS as f32, 500.) as u16,
            rows: (f32::from(viewport.height) / cell.height).clamp(1., 250.) as u16,
        }
    }

    pub fn pty(self, cell: CellSize) -> TerminalSize {
        TerminalSize {
            rows: self.rows,
            cols: self.cols,
            pixel_width: self.cols * cell.width.round() as u16,
            pixel_height: self.rows * cell.height.round() as u16,
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
    fn capture(term: &mut Term<TerminalEvents>) -> Self {
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

    fn restore(self, term: &mut Term<TerminalEvents>) {
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

#[derive(Default)]
struct PendingEvents {
    queue: Vec<Event>,
    // Only the last title in an output batch is visible. An empty title restores the default.
    title: Option<String>,
    bell: bool,
}

#[derive(Clone, Default)]
pub struct TerminalEvents(Rc<RefCell<PendingEvents>>);

impl EventListener for TerminalEvents {
    fn send_event(&self, event: Event) {
        match event {
            Event::Title(title) => self.0.borrow_mut().title = Some(title),
            Event::ResetTitle => self.0.borrow_mut().title = Some(String::new()),
            Event::Bell => self.0.borrow_mut().bell = true,
            Event::PtyWrite(_)
            | Event::TextAreaSizeRequest(_)
            | Event::ColorRequest(_, _)
            | Event::ClipboardStore(_, _) => {
                self.0.borrow_mut().queue.push(event);
            }
            _ => {}
        }
    }
}

pub struct Emulator {
    pub term: Term<TerminalEvents>,
    pub palette: &'static TerminalPalette,
    pub font: TerminalFont,
    default_cursor: CursorStyle,
    pub selection_unit: Unit,
    parser: Processor,
    events: TerminalEvents,
    last_primary_top: Option<RowAnchor>,
    resized_in_alt: bool,
    redraw: Redraw,
}

impl Emulator {
    fn config(default_cursor_style: CursorStyle) -> Config {
        Config {
            scrolling_history: 2000,
            osc52: Osc52::OnlyCopy,
            default_cursor_style,
            ..Config::default()
        }
    }

    #[cfg(test)]
    pub fn new(size: GridSize) -> Self {
        Self::with_palette(
            size,
            &crate::theme::resolve(crate::theme::Appearance::Dark, gpui::WindowAppearance::Dark)
                .terminal,
            TerminalFont::test_font(),
        )
    }

    pub fn with_palette(size: GridSize, palette: &'static TerminalPalette, font: TerminalFont) -> Self {
        let events = TerminalEvents::default();
        let default_cursor = super::cursor::default_style(&Default::default());
        Self {
            term: Term::new(Self::config(default_cursor), &size, events.clone()),
            palette,
            font,
            default_cursor,
            selection_unit: Unit::Character,
            parser: Processor::new(),
            events,
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

    /// Keep the user's cursor shape when a foreground program changes editing modes.
    pub fn display_cursor_style(&self) -> CursorStyle {
        CursorStyle { shape: self.default_cursor.shape, ..self.term.cursor_style() }
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
                let first = self.events.0.borrow().queue.len();
                self.parser.advance(&mut self.term, chunk);
                self.flush_sync();
                for event in &mut self.events.0.borrow_mut().queue[first..] {
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
                Boundary::Reset => {
                    // Alacritty clears its title stack on RIS without emitting a title event.
                    self.events.send_event(Event::ResetTitle);
                    self.redraw.clear();
                }
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
        self.finish_batch_with_clipboard(|_, _| {})
    }

    pub fn finish_batch_with_clipboard(
        &mut self,
        mut copy: impl FnMut(ClipboardType, String),
    ) -> Vec<u8> {
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
        for event in self.events.0.borrow_mut().queue.drain(..) {
            let reply = match event {
                Event::ClipboardStore(target, text) => {
                    copy(target, text);
                    continue;
                }
                Event::PtyWrite(text) => text,
                Event::TextAreaSizeRequest(format) => format(WindowSize {
                    num_cols: self.term.columns() as u16,
                    num_lines: self.term.screen_lines() as u16,
                    cell_width: self.font.cell.width.round() as u16,
                    cell_height: self.font.cell.height.round() as u16,
                }),
                _ => continue,
            };
            bytes.extend_from_slice(reply.as_bytes());
        }
        bytes
    }

    pub fn take_title_update(&self) -> Option<String> {
        let mut title = self.events.0.borrow_mut().title.take()?;
        title.retain(|c| !c.is_control() && !matches!(c, '\u{2028}' | '\u{2029}'));
        Some(title)
    }

    pub fn take_bell(&self) -> bool {
        std::mem::take(&mut self.events.0.borrow_mut().bell)
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

    pub fn scroll_metrics(&self) -> crate::components::ScrollMetrics {
        let grid = self.term.grid();
        if self.term.mode().contains(TermMode::ALT_SCREEN) {
            return crate::components::ScrollMetrics::new(grid.screen_lines() as f32, 0., 0.);
        }
        crate::components::ScrollMetrics::new(
            grid.screen_lines() as f32,
            grid.history_size() as f32,
            (grid.history_size() - grid.display_offset()) as f32,
        )
    }

    pub fn scroll_to_offset(&mut self, offset: f32) -> bool {
        let grid = self.term.grid();
        let from_bottom = grid.history_size().saturating_sub(offset.round() as usize);
        let delta = from_bottom as i32 - grid.display_offset() as i32;
        if delta != 0 {
            self.scroll(Scroll::Delta(delta));
            true
        } else {
            false
        }
    }

    pub fn scroll_for_edge(&mut self, y: f32, top: f32, bottom: f32) {
        if y < top {
            self.scroll(Scroll::Delta(1));
        } else if y >= bottom {
            self.scroll(Scroll::Delta(-1));
        }
    }

    pub fn mouse_enabled(&self, connected: bool) -> bool {
        connected
            && self.term.grid().display_offset() == 0
            && self.term.mode().intersects(TermMode::MOUSE_MODE)
    }

    pub fn grid_position(&self, x: f32, y: f32) -> (usize, f32) {
        let last = self.term.screen_lines() - 1;
        let row = (y / self.font.cell.height)
            .floor()
            .clamp(0., last as f32) as usize;
        (row, x / self.font.cell.width)
    }

    pub fn mouse_cell(&self, x: f32, y: f32) -> super::mouse::Cell {
        let (row, col) = self.grid_position(x, y);
        super::mouse::Cell {
            col: col
                .floor()
                .clamp(0., self.term.last_column().0 as f32) as u16
                + 1,
            row: row as u16 + 1,
        }
    }

    pub fn wheel_target(&self, connected: bool, bypass_app: bool) -> super::mouse::WheelTarget {
        use super::mouse::WheelTarget;
        if !bypass_app && self.mouse_enabled(connected) {
            WheelTarget::Mouse
        } else if !bypass_app
            && connected
            && self
                .term
                .mode()
                .contains(TermMode::ALT_SCREEN | TermMode::ALTERNATE_SCROLL)
        {
            WheelTarget::Arrows
        } else {
            WheelTarget::History
        }
    }

    pub fn paste_bytes(&self, text: &str) -> Vec<u8> {
        super::keys::paste(text, self.term.mode().contains(TermMode::BRACKETED_PASTE))
    }

    pub fn sync_appearance(
        &mut self,
        app_palette: &'static crate::theme::Palette,
        appearance: &crate::theme::TerminalAppearance,
    ) -> u32 {
        self.set_default_cursor(super::cursor::default_style(appearance));
        self.palette = crate::theme::terminal_palette(appearance.scheme, app_palette);
        super::display::color(
            Color::Named(NamedColor::Background),
            self.palette,
            self.term.colors(),
        )
    }

    pub fn cursor_for_paint(
        &self,
        active: bool,
        composing: bool,
        window: &mut gpui::Window,
        cx: &mut gpui::App,
    ) -> Option<alacritty_terminal::vte::ansi::CursorShape> {
        let style = self.display_cursor_style();
        let blink = style.blinking && active && !composing;
        super::cursor::blink_visible(blink, super::cursor::CursorCell::visible(self), window, cx)
            .then_some(style.shape)
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
    fn osc52_copies_live_utf8_without_answering_reads_or_repeating_history() {
        let mut em = Emulator::new(grid(4, 20));
        em.process(b"\x1b]52;c;b2xk\x07");
        assert!(em.finish_batch().is_empty());

        let mut copied = Vec::new();
        for chunk in [
            b"\x1b]52;;5Lit".as_slice(),
            b"5paH\x1b",
            b"\\\x1b]52;p;eA==\x07\x1b]52;c;?\x07\x1b]52;c;***\x07",
        ] {
            em.process(chunk);
            assert!(em.finish_batch_with_clipboard(|target, text| copied.push((target, text))).is_empty());
        }
        assert_eq!(copied, [
            (ClipboardType::Clipboard, String::from("中文")),
            (ClipboardType::Selection, String::from("x")),
        ]);
    }

    #[test]
    fn pty_titles_survive_split_sequences_and_restore_the_default() {
        let mut em = Emulator::new(grid(4, 20));
        em.process(b"\x1b[22;0t");
        for byte in "\x1b]0;项目; build\x1b\\".as_bytes() {
            em.process(std::slice::from_ref(byte));
            em.finish_batch();
        }
        assert_eq!(em.take_title_update().as_deref(), Some("项目; build"));
        em.process(b"\x1b[22;0t\x1b]2;editor\x07\x1b[5n");
        assert_eq!(em.finish_batch(), b"\x1b[0n");
        assert_eq!(em.take_title_update().as_deref(), Some("editor"));
        em.process(b"\x1b[23;0t");
        assert_eq!(em.take_title_update().as_deref(), Some("项目; build"));
        em.process(b"\x1b[23;0t");
        assert_eq!(em.take_title_update().as_deref(), Some(""));

        em.process(b"\x1b]2;first\x07\x1b]2;last\x07");
        assert_eq!(em.take_title_update().as_deref(), Some("last"));
        em.process(b"\x1b]2;\x07");
        assert_eq!(em.take_title_update().as_deref(), Some(""));
        em.process(b"\x1b]2;another title\x07\x1bc");
        assert_eq!(em.take_title_update().as_deref(), Some(""));
        em.process(b"ordinary output");
        assert_eq!(em.take_title_update(), None);
        assert!(!em.take_bell());
        em.process(b"\x07hello\x07");
        assert!(em.take_bell());
        assert!(!em.take_bell());
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
    fn text_area_reports_the_same_pixels_as_the_pty_after_a_spacing_change() {
        let grid = grid(4, 20);
        let mut em = Emulator::new(grid);
        em.font.cell = CellSize { width: 11., height: 24. };
        let pty = grid.pty(em.font.cell);
        assert_eq!((pty.pixel_width, pty.pixel_height), (220, 96));
        em.process(b"\x1b[14t");
        assert_eq!(em.finish_batch(), b"\x1b[4;96;220t");
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

    #[test]
    fn shared_pointer_wheel_paste_and_scroll_helpers_track_emulator_state() {
        use super::super::mouse::{Cell, WheelTarget};
        let mut em = Emulator::new(grid(4, 10));
        em.font.cell = CellSize { width: 10., height: 20. };
        assert_eq!(em.grid_position(25., 45.), (2, 2.5));
        assert_eq!(em.mouse_cell(25., 45.), Cell { col: 3, row: 3 });
        assert_eq!(em.mouse_cell(999., 999.), Cell { col: 10, row: 4 });
        assert!(!em.mouse_enabled(true));
        assert_eq!(em.wheel_target(true, false), WheelTarget::History);
        assert_eq!(em.paste_bytes("a\nb"), b"a\rb");

        em.process(b"\x1b[?2004h\x1b[?1000h");
        assert!(em.mouse_enabled(true));
        assert!(!em.mouse_enabled(false));
        assert_eq!(em.wheel_target(true, false), WheelTarget::Mouse);
        assert_eq!(em.wheel_target(true, true), WheelTarget::History);
        assert_eq!(em.paste_bytes("a\nb"), b"\x1b[200~a\rb\x1b[201~");

        em.process(b"\x1b[?1000l\x1b[?1049h\x1b[?1007h");
        assert_eq!(em.wheel_target(true, false), WheelTarget::Arrows);
        assert_eq!(em.wheel_target(false, false), WheelTarget::History);
        em.process(b"\x1b[?1049l");

        for i in 0..10 {
            em.process(format!("line {i}\r\n").as_bytes());
        }
        assert!(em.scroll_to_offset(0.));
        assert_eq!(em.term.grid().display_offset(), em.term.grid().history_size());
        assert!(!em.scroll_to_offset(0.));
        assert!(em.scroll_to_offset(em.term.grid().history_size() as f32));
        assert_eq!(em.term.grid().display_offset(), 0);
    }
}
