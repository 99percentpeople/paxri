//! Fit the bundled font's joined terminal graphics to the VT grid.
use crate::theme::fonts::TERMINAL_SYMBOL_FONT;
use gpui::{Bounds, PathBuilder, Pixels, Point, Window, fill, point, rgb};
use std::sync::LazyLock;
use ttf_parser::{Face, OutlineBuilder, Rect};

struct GridFont {
    face: Face<'static>,
    // The full block defines the design-space cell. Its top differs from the font ascent.
    cell: Rect,
}

static FONT: LazyLock<GridFont> = LazyLock::new(|| {
    let face = Face::parse(TERMINAL_SYMBOL_FONT, 0).expect("bundled terminal font");
    let cell = face
        .glyph_index('█')
        .and_then(|id| face.glyph_bounding_box(id))
        .expect("bundled full block");
    GridFont { face, cell }
});

// Adjacent runs share their absolute grid edges. Rounding a run's width
// independently from its origin would leave a pixel gap at fractional scales.
pub(super) fn pixel_bounds(start: Point<Pixels>, end: Point<Pixels>, scale: f32) -> Bounds<Pixels> {
    let snap = |value: Pixels| (value * scale).round() / scale;
    Bounds::from_corners(start.map(snap), end.map(snap))
}

pub(super) fn joins_horizontally(ch: char) -> bool {
    matches!(ch, '─' | '━' | '▀'..='█' | '▔')
}

struct CellOutline {
    bounds: Bounds<Pixels>,
    path: PathBuilder,
}

impl CellOutline {
    fn position(&self, x: f32, y: f32) -> Point<Pixels> {
        let cell = FONT.cell;
        // The outlines slightly overhang their design cell to join in text editors.
        // Each terminal cell owns its pixels, including at a change of ANSI color.
        let x = ((x - f32::from(cell.x_min)) / f32::from(cell.width())).clamp(0., 1.);
        let y = ((f32::from(cell.y_max) - y) / f32::from(cell.height())).clamp(0., 1.);
        self.bounds.origin + point(self.bounds.size.width * x, self.bounds.size.height * y)
    }
}

impl OutlineBuilder for CellOutline {
    fn move_to(&mut self, x: f32, y: f32) {
        self.path.move_to(self.position(x, y));
    }
    fn line_to(&mut self, x: f32, y: f32) {
        self.path.line_to(self.position(x, y));
    }
    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        self.path
            .curve_to(self.position(x, y), self.position(x1, y1));
    }
    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        self.path.cubic_bezier_to(
            self.position(x, y),
            self.position(x1, y1),
            self.position(x2, y2),
        );
    }
    fn close(&mut self) {
        self.path.close();
    }
}

pub(super) fn paint(ch: char, bounds: Bounds<Pixels>, color: u32, window: &mut Window) {
    let Some(id) = FONT.face.glyph_index(ch) else {
        return;
    };
    let mut outline = CellOutline {
        bounds,
        path: PathBuilder::fill(),
    };
    // Straight bars, fractional blocks and single quadrants each have a rectangular outline.
    if matches!(ch, '─'..='┃' | '╴'..='╻' | '▀'..='▐' | '▔'..='▘' | '▝')
        && let Some(ink) = FONT.face.glyph_bounding_box(id)
    {
        let start = outline.position(ink.x_min.into(), ink.y_max.into());
        let end = outline.position(ink.x_max.into(), ink.y_min.into());
        let rect = pixel_bounds(start, end, window.scale_factor());
        window.paint_quad(fill(rect, rgb(color)));
    } else if FONT.face.outline_glyph(id, &mut outline).is_some()
        && let Ok(path) = outline.path.build()
    {
        window.paint_path(path, rgb(color));
    }
}
