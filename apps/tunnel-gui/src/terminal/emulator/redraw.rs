use alacritty_terminal::{
    Grid,
    index::Line,
    term::cell::Cell,
    vte::{Params, Parser, Perform},
};

use super::RowAnchor;

// Line editors erase and repaint their prompt using physical rows from their last
// drawing. Keep the old rows addressable while the primary screen is reflowed.
#[derive(Default)]
pub(super) struct Redraw {
    parser: Parser,
    prefix: Prefix,
    rows: Vec<RowAnchor>,
    prompt: Option<RowAnchor>,
    before_up: Option<Line>,
}

impl Redraw {
    pub fn resizing(&mut self, grid: &Grid<Cell>) {
        if self.prompt.is_some() || !self.rows.is_empty() {
            return;
        }
        let cursor = self.motion_start().unwrap_or(grid.cursor.point.line);
        let mut anchor = RowAnchor::at_from(grid, Line(0), cursor);
        for y in 0..=cursor.0 {
            self.rows.push(anchor);
            if let Some(width) = RowAnchor::wrapped_width(grid, Line(y)) {
                anchor.cells_into_line += width;
            } else {
                anchor.hard_breaks_to_cursor = anchor.hard_breaks_to_cursor.saturating_sub(1);
                anchor.cells_into_line = 0;
            }
        }
    }

    pub fn start_up(&mut self, grid: &Grid<Cell>) {
        self.before_up = Some(grid.cursor.point.line);
    }

    fn motion_start(&self) -> Option<Line> {
        self.before_up.filter(|_| self.prefix.up > 0)
    }

    // A window event may arrive between the cursor movement and its erase command.
    pub fn motion_anchor(&self, grid: &Grid<Cell>) -> Option<RowAnchor> {
        self.motion_start().map(|line| RowAnchor::at(grid, line))
    }

    pub fn restore_motion(&mut self, grid: &Grid<Cell>, anchor: Option<RowAnchor>) {
        self.before_up = anchor.map(|a| a.resolve(grid));
    }

    pub fn erase(&mut self, up: usize, grid: &mut Grid<Cell>) {
        let index = self.rows.len().saturating_sub(up.saturating_add(1));
        let anchor = self
            .rows
            .get(index)
            .copied()
            .or(self.prompt.filter(|_| up > 0));
        if let Some(anchor) = anchor {
            let before = self
                .before_up
                .filter(|_| up > 0)
                .unwrap_or(grid.cursor.point.line);
            let line = anchor.resolve_from(grid, before);
            grid.cursor.point.line = line.max(Line(0));
            // A slow peer can still send repaints for intermediate window sizes.
            // Hard line breaks locate the same prompt even when those repaints wrap.
            self.prompt = Some(anchor);
        }
        self.rows.clear();
    }

    pub fn clear(&mut self) {
        self.rows.clear();
        self.prompt = None;
        self.before_up = None;
    }

    pub fn next(&mut self, bytes: &[u8]) -> (usize, Option<Boundary>) {
        self.prefix.event = None;
        let used = self
            .parser
            .advance_until_terminated(&mut self.prefix, bytes);
        (used, self.prefix.event.take())
    }
}

pub(super) enum Boundary {
    Up,
    Erase(usize),
    End,
    ColorQuery,
}

#[derive(Default)]
struct Prefix {
    margin: bool,
    up: usize,
    event: Option<Boundary>,
}

impl Perform for Prefix {
    fn print(&mut self, _: char) {
        self.margin = false;
        self.up = 0;
    }
    fn execute(&mut self, byte: u8) {
        self.margin = byte == b'\r';
        if !self.margin {
            self.up = 0;
        }
    }
    fn csi_dispatch(&mut self, params: &Params, intermediates: &[u8], ignore: bool, action: char) {
        let first = params
            .iter()
            .next()
            .and_then(|p| p.first())
            .copied()
            .unwrap_or(0);
        match (ignore, intermediates, action) {
            (false, [], 'm') => return,
            (false, [], 'A') => {
                if self.up == 0 {
                    self.event = Some(Boundary::Up);
                }
                self.up = self.up.saturating_add(usize::from(first.max(1)));
                return;
            }
            (false, [], 'J') if first == 0 && self.margin => {
                self.event = Some(Boundary::Erase(self.up))
            }
            (false, [], 'J') if matches!(first, 2 | 3) => self.event = Some(Boundary::End),
            (false, b"?", 'h' | 'l')
                if params.iter().any(|p| {
                    matches!(p, [47] | [1047] | [1049]) || (p == [2004] && action == 'l')
                }) =>
            {
                self.event = Some(Boundary::End)
            }
            _ => {}
        }
        self.margin = false;
        self.up = 0;
    }
    fn esc_dispatch(&mut self, intermediates: &[u8], ignore: bool, byte: u8) {
        if !ignore && intermediates.is_empty() && byte == b'c' {
            self.event = Some(Boundary::End);
        }
        self.margin = false;
        self.up = 0;
    }
    fn osc_dispatch(&mut self, params: &[&[u8]], _: bool) {
        if matches!(params.first().copied(), Some(b"10" | b"11" | b"12"))
            && params.contains(&b"?".as_slice())
        {
            self.event = Some(Boundary::ColorQuery);
        }
        self.margin = false;
        self.up = 0;
    }
    fn terminated(&self) -> bool {
        self.event.is_some()
    }
}
