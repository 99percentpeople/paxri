//! Byte positions and hit testing for the visual rows produced by GPUI's text shaper.
use gpui::*;
use std::ops::Range;

struct Row {
    logical_line: usize,
    line_start: usize,
    bytes: Range<usize>,
}

pub(super) struct InputLayout {
    lines: Vec<WrappedLine>,
    rows: Vec<Row>,
    pub bounds: Bounds<Pixels>,
    pub height: Pixels,
    pub scroll: Point<Pixels>,
}

impl InputLayout {
    pub fn new(lines: Vec<WrappedLine>, bounds: Bounds<Pixels>, height: Pixels) -> Self {
        let mut rows = Vec::new();
        let mut line_start = 0;
        for (logical_line, line) in lines.iter().enumerate() {
            let mut start = line_start;
            let ends = line.wrap_boundaries().iter().map(|boundary| {
                let run = &line.runs()[boundary.run_ix];
                run.glyphs[boundary.glyph_ix].index
            });
            for end in ends.chain([line.len()]) {
                let end = line_start + end;
                rows.push(Row {
                    logical_line,
                    line_start,
                    bytes: start..end,
                });
                start = end;
            }
            line_start += line.len() + 1;
        }
        Self {
            lines,
            rows,
            bounds,
            height,
            scroll: Point::default(),
        }
    }

    pub fn row_count(&self) -> usize {
        self.rows.len()
    }

    pub fn row_at(&self, byte: usize, hint: Option<usize>) -> usize {
        // A soft wrap has one byte position with two possible caret locations.
        if let Some(i) = hint
            && let Some(row) = self.rows.get(i)
            && row.bytes.start <= byte
            && byte <= row.bytes.end
        {
            return i;
        }
        self.rows
            .partition_point(|row| row.bytes.start <= byte)
            .saturating_sub(1)
    }

    pub fn range(&self, row: usize) -> Range<usize> {
        self.rows[row].bytes.clone()
    }

    pub fn x(&self, row: usize, byte: usize) -> Pixels {
        let row = &self.rows[row];
        let line = &self.lines[row.logical_line].unwrapped_layout;
        line.x_for_index(byte.clamp(row.bytes.start, row.bytes.end) - row.line_start)
            - line.x_for_index(row.bytes.start - row.line_start)
    }

    pub fn byte_at(&self, index: usize, x: Pixels) -> usize {
        let row = &self.rows[index];
        let line = &self.lines[row.logical_line].unwrapped_layout;
        let start_x = line.x_for_index(row.bytes.start - row.line_start);
        (row.line_start + line.closest_index_for_x(start_x + x))
            .clamp(row.bytes.start, row.bytes.end)
    }

    pub fn origin(&self, row: usize) -> Point<Pixels> {
        self.bounds.origin - self.scroll + point(px(0.), self.height * row)
    }

    pub fn hit(&self, pt: Point<Pixels>) -> (usize, usize) {
        let offset = pt - self.origin(0);
        let row = ((offset.y / self.height) as usize).min(self.rows.len() - 1);
        (self.byte_at(row, offset.x), row)
    }

    pub fn max_scroll_y(&self) -> Pixels {
        (self.height * self.row_count() - self.bounds.size.height).max(px(0.))
    }

    pub fn paint_text(&self, window: &mut Window, cx: &mut App) {
        let mut origin = self.origin(0);
        for line in &self.lines {
            let height = line.size(self.height).height;
            if origin.y >= self.bounds.bottom() {
                break;
            }
            if origin.y + height > self.bounds.top() {
                let _ = line.paint(origin, self.height, TextAlign::Left, None, window, cx);
            }
            origin.y += height;
        }
    }

    pub fn paint_range(
        &self,
        range: Range<usize>,
        underline: bool,
        color: u32,
        window: &mut Window,
    ) {
        if range.is_empty() {
            return;
        }
        for (i, row) in self.rows.iter().enumerate() {
            let origin = self.origin(i);
            if origin.y >= self.bounds.bottom() {
                break;
            }
            if origin.y + self.height <= self.bounds.top()
                || range.end <= row.bytes.start
                || range.start > row.bytes.end
            {
                continue;
            }
            let x = self.x(i, range.start);
            let mut end = self.x(i, range.end);
            if !underline
                && range.end > row.bytes.end
                && self
                    .rows
                    .get(i + 1)
                    .is_some_and(|next| next.bytes.start > row.bytes.end)
            {
                // Make a selected newline visible, including on an otherwise empty row.
                end += px(4.);
            }
            if end > x {
                let (top, height) = if underline {
                    (self.height - px(2.), px(1.))
                } else {
                    (px(0.), self.height)
                };
                window.paint_quad(fill(
                    Bounds::new(origin + point(x, top), size(end - x, height)),
                    rgb(color),
                ));
            }
        }
    }
}
