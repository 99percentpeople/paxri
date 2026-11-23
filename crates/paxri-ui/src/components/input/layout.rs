//! Byte positions and hit testing for the visual rows produced by GPUI's text shaper.
use crate::components::caret::GlyphSpan;
use gpui::*;
use std::{ops::Range, time::Duration};

// Leave room for the caret at the end of an unwrapped line.
pub(super) const CARET_SPACE: Pixels = px(2.);

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

    pub fn caret_bounds(&self, byte: usize, hint: Option<usize>) -> Bounds<Pixels> {
        let row = self.row_at(byte, hint);
        let line = &self.lines[self.rows[row].logical_line];
        let span =
            GlyphSpan::from_metrics(line.font_size(), self.height, line.ascent(), line.descent());
        span.in_row(Bounds::new(
            self.origin(row) + point(self.x(row, byte), px(0.)),
            size(px(1.5), self.height),
        ))
    }

    pub fn hit(&self, pt: Point<Pixels>) -> (usize, usize) {
        let offset = pt - self.origin(0);
        let row = ((offset.y / self.height) as usize).min(self.rows.len() - 1);
        (self.byte_at(row, offset.x), row)
    }

    pub fn max_scroll_y(&self) -> Pixels {
        (self.height * self.row_count() - self.bounds.size.height).max(px(0.))
    }

    pub fn max_scroll_x(&self) -> Pixels {
        let width = self
            .lines
            .first()
            .map_or(px(0.), |line| line.unwrapped_layout.width);
        (width + CARET_SPACE - self.bounds.size.width).max(px(0.))
    }

    /// Hit-test against the newly scrolled text, even if the pointer stayed still.
    pub fn drag_hit<const MULTILINE: bool>(
        &mut self,
        mut pointer: Point<Pixels>,
        elapsed: Duration,
    ) -> (usize, usize, bool) {
        let moving = if MULTILINE {
            let limit = self.max_scroll_y();
            pan_selection(
                &mut pointer.y,
                self.bounds.top()..self.bounds.bottom(),
                &mut self.scroll.y,
                limit,
                elapsed,
            )
        } else {
            let limit = self.max_scroll_x();
            pan_selection(
                &mut pointer.x,
                self.bounds.left()..self.bounds.right(),
                &mut self.scroll.x,
                limit,
                elapsed,
            )
        };
        let (byte, row) = self.hit(pointer);
        (byte, row, moving)
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

/// Pan faster further into or beyond the edge. Return whether another frame can advance.
fn pan_selection(
    pointer: &mut Pixels,
    viewport: Range<Pixels>,
    scroll: &mut Pixels,
    limit: Pixels,
    elapsed: Duration,
) -> bool {
    let edge = px(12.).min((viewport.end - viewport.start) / 2.);
    let near_start = (viewport.start + edge - *pointer).max(px(0.));
    let near_end = (*pointer - (viewport.end - edge)).max(px(0.));
    let speed = ((near_end - near_start) * 75.).clamp(px(-1800.), px(1800.));
    *scroll = (*scroll + speed * elapsed.as_secs_f32().min(0.1)).clamp(px(0.), limit);

    // The far edge is exclusive. This matters when a textarea ends exactly on a line boundary.
    let end = (viewport.end - px(0.01)).max(viewport.start);
    *pointer = (*pointer).clamp(viewport.start, end);
    (speed < px(0.) && *scroll > px(0.)) || (speed > px(0.) && *scroll < limit)
}

#[cfg(test)]
mod tests {
    use super::{InputLayout, pan_selection};
    use gpui::{Bounds, WrappedLine, point, px, size};
    use std::{sync::Arc, time::Duration};

    #[test]
    fn caret_follows_the_scrolled_row_and_includes_descenders() {
        let lines = [(0., 0.), (13., 4.), (19., 6.)].map(|(ascent, descent)| {
            let mut line = WrappedLine::default();
            let wrapped = Arc::get_mut(&mut *line).unwrap();
            let metrics = Arc::get_mut(&mut wrapped.unwrapped_layout).unwrap();
            metrics.font_size = px(14.);
            metrics.ascent = px(ascent);
            metrics.descent = px(descent);
            line
        });
        let bounds = Bounds::new(point(px(8.), px(10.)), size(px(90.), px(50.)));
        let mut layout = InputLayout::new(lines.into(), bounds, px(20.));
        layout.scroll.y = px(5.);
        for (byte, y, height) in [(0, 8., 14.), (1, 26.5, 17.), (2, 45., 20.)] {
            assert_eq!(
                layout.caret_bounds(byte, None),
                Bounds::new(point(px(8.), px(y)), size(px(1.5), px(height)))
            );
        }
    }

    #[test]
    fn holding_the_edge_advances_until_the_end_and_can_reverse() {
        let mut offset = px(0.);
        let limit = px(250.);
        let dt = Duration::from_millis(16);
        for position in [99., -40.] {
            let mut frames = 0;
            loop {
                // Each frame receives the same physical pointer position.
                let mut pointer = px(position);
                let moving = pan_selection(&mut pointer, px(0.)..px(100.), &mut offset, limit, dt);
                assert!((px(0.)..px(100.)).contains(&pointer));
                if !moving {
                    break;
                }
                frames += 1;
                assert!(frames < 60);
            }
            assert!(frames > 1);
            assert_eq!(offset, if position > 0. { limit } else { px(0.) });
        }
    }

    #[test]
    fn dragging_in_the_middle_or_in_a_field_that_fits_does_not_pan() {
        let dt = Duration::from_millis(16);
        let mut offset = px(30.);
        let mut pointer = px(60.);
        assert!(!pan_selection(
            &mut pointer,
            px(10.)..px(110.),
            &mut offset,
            px(100.),
            dt
        ));
        assert_eq!((offset, pointer), (px(30.), px(60.)));

        pointer = px(108.);
        assert!(!pan_selection(
            &mut pointer,
            px(10.)..px(110.),
            &mut offset,
            px(0.),
            dt
        ));
        assert_eq!((offset, pointer), (px(0.), px(108.)));
    }
}
