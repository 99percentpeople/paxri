mod contrast;

use super::{
    cursor::{self, CursorCell, Preedit},
    emulator::Emulator,
    grid_glyph,
    links::TerminalLink,
};
use crate::components::caret::GlyphSpan;
use crate::theme::{
    TerminalPalette,
    fonts::{TERMINAL_SYMBOL_FAMILY, TerminalFont},
};
use alacritty_terminal::{
    grid::Dimensions,
    index::{Column, Point as GridPoint},
    term::{
        cell::{Cell, Flags},
        color::Colors,
    },
    vte::ansi::{Color, CursorShape, NamedColor},
};
use gpui::{
    App, Bounds, ContentMask, FontStyle, FontWeight, Pixels, TextRun, Window, fill, font, point,
    px, rgb, size,
};

pub fn color(value: Color, palette: &TerminalPalette, overrides: &Colors) -> u32 {
    let override_rgb = match value {
        Color::Named(name) => overrides[name],
        Color::Indexed(index) => overrides[usize::from(index)],
        Color::Spec(_) => None,
    };
    if let Some(rgb) = override_rgb {
        return u32::from_be_bytes([0, rgb.r, rgb.g, rgb.b]);
    }
    match value {
        Color::Spec(rgb) => u32::from_be_bytes([0, rgb.r, rgb.g, rgb.b]),
        Color::Named(NamedColor::Background) => palette.surface.background,
        Color::Named(NamedColor::Cursor) => palette.cursor,
        Color::Named(name) if name <= NamedColor::BrightWhite => palette.ansi[name as usize],
        Color::Named(name)
            if name == NamedColor::DimForeground
                || (NamedColor::DimBlack..=NamedColor::DimWhite).contains(&name) =>
        {
            faint_color(
                color(Color::Named(name.to_bright()), palette, overrides),
                color(Color::Named(NamedColor::Background), palette, overrides),
            )
        }
        Color::Named(_) => palette.surface.foreground,
        Color::Indexed(i @ 0..16) => palette.ansi[i as usize],
        Color::Indexed(i @ 16..232) => {
            let i = i - 16;
            let scale = [0, 95, 135, 175, 215, 255];
            u32::from_be_bytes([
                0,
                scale[(i / 36) as usize],
                scale[(i / 6 % 6) as usize],
                scale[(i % 6) as usize],
            ])
        }
        Color::Indexed(i) => {
            let grey = 8 + (i - 232) * 10;
            u32::from_be_bytes([0, grey, grey, grey])
        }
    }
}

// SGR 2 reduces contrast against the cell's background, including on light themes.
fn faint_color(foreground: u32, background: u32) -> u32 {
    let foreground = foreground.to_be_bytes();
    let background = background.to_be_bytes();
    u32::from_be_bytes(std::array::from_fn(|i| {
        let fg = u16::from(foreground[i]);
        let bg = u16::from(background[i]);
        ((3 * fg + 2 * bg + 2) / 5) as u8
    }))
}

#[derive(Clone, Copy, PartialEq)]
enum GlyphSource {
    Text,
    WideText,
    TerminalSymbol,
    Grid(char),
}

impl GlyphSource {
    fn for_cell(cell: &Cell) -> Self {
        // Preserve composed text and emoji presentation selectors for the text shaper.
        if cell.flags.intersects(Flags::HIDDEN | Flags::WIDE_CHAR_SPACER) {
            return Self::Text;
        }
        if cell.flags.contains(Flags::WIDE_CHAR) {
            return Self::WideText;
        }
        if cell.zerowidth().is_some_and(|marks| !marks.is_empty()) {
            return Self::Text;
        }
        match cell.c {
            '\u{2500}'..='\u{259f}' | '\u{e0b0}'..='\u{e0b3}' => Self::Grid(cell.c),
            '\u{2190}'..='\u{2195}'
            | '\u{23ce}'
            | '\u{2713}'
            | '\u{2800}'..='\u{28ff}'
            | '\u{e0a0}'..='\u{e0a3}' => Self::TerminalSymbol,
            _ => Self::Text,
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
struct Style {
    glyph: GlyphSource,
    fg: u32,
    bg: u32,
    bold: bool,
    italic: bool,
    underline: bool,
}

impl Style {
    fn from_cell(cell: &Cell, palette: &TerminalPalette, overrides: &Colors) -> Self {
        let mut fg = color(cell.fg, palette, overrides);
        let mut bg = color(cell.bg, palette, overrides);
        if cell.flags.contains(Flags::DIM) {
            fg = faint_color(fg, bg);
        }
        if cell.flags.contains(Flags::INVERSE) {
            std::mem::swap(&mut fg, &mut bg);
        }
        Self {
            glyph: GlyphSource::for_cell(cell),
            fg,
            bg,
            bold: cell.flags.contains(Flags::BOLD),
            italic: cell.flags.contains(Flags::ITALIC),
            underline: cell.flags.intersects(Flags::ALL_UNDERLINES),
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
struct CellPaint {
    fg: u32,
    bg: u32,
    underline: bool,
    block: bool,
}

struct PaintSpan {
    start: u16,
    end: u16,
    byte_len: usize,
    paint: CellPaint,
}

fn has_rtl(text: &str) -> bool {
    !text.is_ascii()
        && text.chars().any(|c| {
            matches!(
                c,
                '\u{0590}'..='\u{08ff}'
                    | '\u{200f}'
                    | '\u{202b}'
                    | '\u{202e}'
                    | '\u{2067}'
                    | '\u{fb1d}'..='\u{fdff}'
                    | '\u{fe70}'..='\u{feff}'
                    | '\u{10800}'..='\u{10fff}'
                    | '\u{1e800}'..='\u{1efff}'
            )
        })
}

fn text_face(typography: &TerminalFont, style: Style) -> gpui::Font {
    let mut face = if style.glyph == GlyphSource::TerminalSymbol {
        font(TERMINAL_SYMBOL_FAMILY)
    } else {
        typography.face.clone()
    };
    if style.bold {
        face.weight = FontWeight::BOLD;
    }
    if style.italic {
        face.style = FontStyle::Italic;
    }
    face
}

fn build_text_runs(
    target_len: usize,
    spans: &[PaintSpan],
    face: &gpui::Font,
    mut ink_for: impl FnMut(CellPaint) -> u32,
) -> Vec<TextRun> {
    let mut remaining = target_len;
    let mut runs: Vec<TextRun> = Vec::with_capacity(spans.len());
    for part in spans {
        let len = part.byte_len.min(remaining);
        if len == 0 {
            break;
        }
        remaining -= len;
        let color = rgb(ink_for(part.paint)).into();
        if let Some(last) = runs.last_mut()
            && last.color == color
        {
            last.len += len;
        } else {
            runs.push(TextRun {
                len,
                font: face.clone(),
                color,
                background_color: None,
                underline: None,
                strikethrough: None,
            });
        }
    }
    runs
}

#[allow(clippy::too_many_arguments)]
fn paint_runs(
    text: &str,
    runs: &[TextRun],
    typography: &TerminalFont,
    baseline: Pixels,
    wide: bool,
    bounds: Bounds<Pixels>,
    window: &mut Window,
    cx: &mut App,
) {
    if text.is_empty() || runs.is_empty() {
        return;
    }
    let shaped = window.text_system().shape_line(
        text.to_owned().into(),
        px(typography.size),
        runs,
        (!wide).then(|| px(typography.cell.width)),
    );
    // Center fullwidth clusters in their grid cells. Use the same primary-font
    // baseline even when a spinner or a CJK character introduces a fallback face.
    let x = if wide {
        (bounds.size.width - shaped.width) / 2.
    } else {
        px(0.)
    };
    let layout_baseline = (px(typography.cell.height) + shaped.ascent - shaped.descent) / 2.;
    let at = bounds.origin + point(x, baseline - layout_baseline);
    #[cfg(not(target_os = "android"))]
    let _ = shaped.paint(at, px(typography.cell.height), window, cx);
    #[cfg(target_os = "android")]
    let _ = shaped.paint(
        at,
        px(typography.cell.height),
        gpui::TextAlign::Left,
        None,
        window,
        cx,
    );
}

fn append_glyph(text: &mut String, cell: &Cell) {
    if cell.flags.contains(Flags::WIDE_CHAR_SPACER) {
        return;
    }
    if cell.flags.contains(Flags::HIDDEN) {
        text.push(' ');
        return;
    }
    text.push(if cell.c == '\t' { ' ' } else { cell.c });
    if let Some(marks) = cell.zerowidth() {
        text.extend(marks);
    }
}

fn paint_ink(
    text: &str,
    typography: &TerminalFont,
    baseline: Pixels,
    style: Style,
    bounds: Bounds<Pixels>,
    window: &mut Window,
    cx: &mut App,
) {
    let visible = text.trim_end_matches(' ');
    if let GlyphSource::Grid(ch) = style.glyph {
        grid_glyph::paint(ch, bounds, style.fg, window);
    } else if !visible.is_empty() {
        // ASCII padding has no glyph ink. Keep the original text for bidirectional shaping.
        let text = if visible.is_ascii() { visible } else { text };
        let run = TextRun {
            len: text.len(),
            font: text_face(typography, style),
            color: rgb(style.fg).into(),
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        paint_runs(
            text,
            &[run],
            typography,
            baseline,
            style.glyph == GlyphSource::WideText,
            bounds,
            window,
            cx,
        );
    }
}

fn paint_underline(bounds: Bounds<Pixels>, fg: u32, window: &mut Window) {
    window.paint_quad(fill(
        Bounds::new(
            point(bounds.left(), bounds.bottom() - px(2.)),
            size(bounds.size.width, px(1.)),
        ),
        rgb(fg),
    ));
}

pub fn paint(
    screen: &Emulator,
    bounds: Bounds<Pixels>,
    marked: Option<&Preedit>,
    cursor_shape: Option<CursorShape>,
    hovered_link: Option<&TerminalLink>,
    palette: &TerminalPalette,
    window: &mut Window,
    cx: &mut App,
) {
    let grid = screen.term.grid();
    let overrides = screen.term.colors();
    let background = color(Color::Named(NamedColor::Background), palette, overrides);
    let cursor_color = color(Color::Named(NamedColor::Cursor), palette, overrides);
    let cols = grid.columns() as u16;
    let selection = screen.selected_range();
    let cursor = CursorCell::visible(screen);
    let span = GlyphSpan::measure(
        cx.text_system(),
        &screen.font.face,
        px(screen.font.size),
        px(screen.font.cell.height),
    );
    let origin = |r: u16, c: u16| {
        bounds.origin
            + point(
                px(f32::from(c) * screen.font.cell.width),
                px(f32::from(r) * screen.font.cell.height),
            )
    };
    let scale = window.scale_factor();
    let cell = screen.font.cell;
    let caret = cursor.and_then(|c| {
        let at = origin(c.point.line.0 as u16, c.point.column.0 as u16);
        cursor::stroke(cursor_shape?, c.bounds(at, cell), span)
    });
    let block = cursor.filter(|_| marked.is_none() && cursor_shape == Some(CursorShape::Block));
    let block_fg = contrast::black_or_white(cursor_color);
    let light_surface = contrast::black_or_white(background) == 0x000000;
    let mut text = String::new();
    let mut spans: Vec<PaintSpan> = Vec::new();
    for row in 0..grid.screen_lines() as u16 {
        let line = screen.view_line(usize::from(row));
        let coord = |col: u16| GridPoint::new(line, Column(usize::from(col)));
        let rect =
            |start, end| grid_glyph::pixel_bounds(origin(row, start), origin(row + 1, end), scale);
        let mut col = 0;
        while col < cols {
            let first = col;
            let face = Style::from_cell(&grid[coord(first)], palette, overrides);
            text.clear();
            spans.clear();
            while col < cols {
                let start = col;
                let c = &grid[coord(col)];
                let style = Style::from_cell(c, palette, overrides);
                // Shaping depends only on glyphs and font. A highlight, including one
                // drawn by a TUI using SGR colors, must never move the rest of the row.
                if col > first
                    && (style.glyph != face.glyph
                        || style.bold != face.bold
                        || style.italic != face.italic
                        || c.flags.contains(Flags::WIDE_CHAR)
                        || matches!(face.glyph, GlyphSource::Grid(ch) if !grid_glyph::joins_horizontally(ch)))
                {
                    break;
                }
                let byte_start = text.len();
                append_glyph(&mut text, c);
                let byte_len = text.len() - byte_start;
                col += 1;
                let wide = c.flags.contains(Flags::WIDE_CHAR);
                if wide && col < cols && grid[coord(col)].flags.contains(Flags::WIDE_CHAR_SPACER) {
                    col += 1;
                }
                let selected = selection.is_some_and(|s| {
                    s.contains(coord(start)) || (wide && s.contains(coord(col - 1)))
                });
                let linked = hovered_link.is_some_and(|link| {
                    link.contains(coord(start)) || (wide && link.contains(coord(col - 1)))
                });
                let block_here = block.is_some_and(|c| c.covers(coord(start)));
                let paint = CellPaint {
                    fg: if block_here {
                        block_fg
                    } else if selected {
                        palette.selection.foreground
                    } else {
                        style.fg
                    },
                    bg: if selected {
                        palette.selection.background
                    } else {
                        style.bg
                    },
                    underline: style.underline || linked,
                    block: block_here,
                };
                if let Some(last) = spans.last_mut()
                    && last.paint == paint
                {
                    last.end = col;
                    last.byte_len += byte_len;
                } else {
                    spans.push(PaintSpan {
                        start,
                        end: col,
                        byte_len,
                        paint,
                    });
                }
                if wide {
                    break;
                }
            }
            let bounds = rect(first, col);
            let text_glyph = matches!(face.glyph, GlyphSource::Text | GlyphSource::WideText);
            let part_ink = |paint: CellPaint| {
                if light_surface && text_glyph && !paint.block {
                    contrast::readable_text(paint.fg, paint.bg)
                } else {
                    paint.fg
                }
            };
            for part in &spans {
                let area = rect(part.start, part.end);
                if part.paint.bg != background {
                    window.paint_quad(fill(area, rgb(part.paint.bg)));
                }
                if part.paint.block
                    && let Some(caret) = caret
                {
                    window.paint_quad(fill(caret, rgb(cursor_color)));
                }
            }
            if let GlyphSource::Grid(ch) = face.glyph {
                for part in &spans {
                    let area = rect(part.start, part.end);
                    grid_glyph::paint(ch, area, part_ink(part.paint), window);
                }
            } else if spans.len() > 1 && has_rtl(&text) {
                for part in &spans {
                    let area = rect(part.start, part.end);
                    let outer = window.content_mask().bounds;
                    let left = if part.start == first {
                        outer.left()
                    } else {
                        area.left()
                    };
                    let right = if part.end == col {
                        outer.right()
                    } else {
                        area.right()
                    };
                    let mask = ContentMask {
                        bounds: Bounds::from_corners(
                            point(left, outer.top()),
                            point(right, outer.bottom()),
                        ),
                    };
                    window.with_content_mask(Some(mask), |window| {
                        let style = Style {
                            fg: part_ink(part.paint),
                            ..face
                        };
                        paint_ink(&text, &screen.font, span.baseline, style, bounds, window, cx);
                    });
                }
            } else {
                let visible = text.trim_end_matches(' ');
                if !visible.is_empty() {
                    let target = if has_rtl(&text) {
                        text.as_str()
                    } else {
                        visible
                    };
                    let run_face = text_face(&screen.font, face);
                    let runs = build_text_runs(target.len(), &spans, &run_face, part_ink);
                    paint_runs(
                        target,
                        &runs,
                        &screen.font,
                        span.baseline,
                        face.glyph == GlyphSource::WideText,
                        bounds,
                        window,
                        cx,
                    );
                }
            }
            for part in &spans {
                if part.paint.underline {
                    let area = rect(part.start, part.end);
                    paint_underline(area, part_ink(part.paint), window);
                }
            }
        }
    }
    if let Some(marked) = marked {
        if let Some(preedit) = marked.layout(screen, bounds.origin) {
            let area = grid_glyph::pixel_bounds(
                preedit.at,
                preedit.at + point(px(f32::from(preedit.columns) * cell.width), px(cell.height)),
                scale,
            );
            let style = Style {
                glyph: GlyphSource::Text,
                fg: contrast::black_or_white(palette.composition),
                bg: palette.composition,
                bold: false,
                italic: false,
                underline: true,
            };
            window.paint_quad(fill(area, rgb(style.bg)));
            let ink = |fg, window: &mut Window, cx: &mut App| {
                for (text, columns, wide) in preedit.runs() {
                    let start = preedit.at + point(px(columns.start as f32 * cell.width), px(0.));
                    let end =
                        preedit.at + point(px(columns.end as f32 * cell.width), px(cell.height));
                    let bounds = grid_glyph::pixel_bounds(start, end, scale);
                    let glyph = if wide { GlyphSource::WideText } else { GlyphSource::Text };
                    let style = Style { fg, glyph, ..style };
                    paint_ink(text, &screen.font, span.baseline, style, bounds, window, cx);
                }
                paint_underline(area, fg, window);
            };
            ink(style.fg, window, cx);
            if let Some(shape) = cursor_shape
                && let Some(bounds) =
                    cursor::stroke(shape, preedit.caret_cell(marked.selection.end), span)
            {
                window.paint_quad(fill(bounds, rgb(cursor_color)));
                if shape == CursorShape::Block {
                    window.with_content_mask(Some(ContentMask { bounds }), |window| {
                        ink(block_fg, window, cx)
                    });
                }
            }
        }
    } else if cursor_shape != Some(CursorShape::Block)
        && let Some(caret) = caret
    {
        window.paint_quad(fill(caret, rgb(cursor_color)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::{Appearance, TerminalScheme};
    use gpui::WindowAppearance;

    #[test]
    fn powerline_symbols_are_single_cells_and_preserve_presentation_and_concealment() {
        let mut em = Emulator::new(super::super::emulator::GridSize { rows: 1, cols: 16 });
        em.process("A│⠕→\u{fe0f}\x1b[8m".as_bytes());
        let line = &em.term.grid()[em.view_line(0)];
        let source = |i| GlyphSource::for_cell(&line[Column(i)]);
        assert!(matches!(source(0), GlyphSource::Text));
        for (i, ch) in "".chars().enumerate() {
            assert!(matches!(source(i + 1), GlyphSource::Grid(c) if c == ch));
        }
        assert!(matches!(source(5), GlyphSource::TerminalSymbol));
        assert!(matches!(source(6), GlyphSource::Grid('│')));
        assert!(matches!(source(7), GlyphSource::TerminalSymbol));
        assert!(matches!(source(8), GlyphSource::Text));
        assert!(matches!(source(9), GlyphSource::Text));
    }

    #[test]
    fn concealed_cells_keep_their_position_without_painting_combining_marks() {
        let mut em = Emulator::new(super::super::emulator::GridSize { rows: 1, cols: 4 });
        em.process("A\x1b[8me\u{301}\x1b[28mB".as_bytes());
        let mut text = String::new();
        for column in 0..3 {
            append_glyph(&mut text, &em.term.grid()[em.view_line(0)][Column(column)]);
        }
        assert_eq!(text, "A B");
    }

    #[test]
    fn faint_text_and_intensity_resets_follow_the_terminal_palette() {
        let mut em = Emulator::new(super::super::emulator::GridSize { rows: 1, cols: 10 });
        em.process(b"A\x1b[2mB\x1b[1mC\x1b[22mD\x1b[2mE\x1b[0mF\x1b[90mG\x1b[38;5;244mH\x1b[38;2;111;111;111mI");
        for (scheme, expected) in [
            (TerminalScheme::Dark, 0x9a9a9a),
            (TerminalScheme::Light, 0x6a6a6a),
            (TerminalScheme::Midnight, 0x8f99a7),
        ] {
            let palette = crate::theme::terminal_palette(
                scheme,
                crate::theme::resolve(Appearance::Dark, WindowAppearance::Dark),
            );
            let at = |col| {
                Style::from_cell(
                    &em.term.grid()[em.view_line(0)][Column(col)],
                    palette,
                    em.term.colors(),
                )
            };
            assert_eq!(at(0).fg, palette.surface.foreground);
            assert_eq!(at(1).fg, expected);
            assert_eq!(
                color(
                    Color::Named(NamedColor::DimForeground),
                    palette,
                    em.term.colors()
                ),
                expected
            );
            assert_eq!(at(1).bg, palette.surface.background);
            assert!(at(2).bold);
            assert_eq!(at(2).fg, expected);
            assert!(!at(3).bold);
            assert_eq!(at(3).fg, at(0).fg);
            assert_eq!(at(4).fg, expected);
            assert_eq!(at(5).fg, at(0).fg);
            assert_eq!(at(6).fg, palette.ansi[8]);
            assert_eq!(at(7).fg, 0x808080);
            assert_eq!(at(8).fg, 0x6f6f6f);
        }
    }

    #[test]
    fn faint_uses_osc_and_cell_colors_before_reverse_video() {
        let mut em = Emulator::new(super::super::emulator::GridSize { rows: 1, cols: 6 });
        em.process(b"\x1b]4;1;#c8643c\x07\x1b[48;2;40;20;20;31mA\x1b[2mB\x1b[38;5;1mC\x1b[38;2;200;100;60mD\x1b[7mE");
        let at = |col| {
            Style::from_cell(
                &em.term.grid()[em.view_line(0)][Column(col)],
                em.palette,
                em.term.colors(),
            )
        };
        assert_eq!(at(0).fg, 0xc8643c);
        for col in 1..=3 {
            let style = at(col);
            assert_eq!((style.fg, style.bg), (0x88442c, 0x281414));
        }
        assert_eq!((at(4).fg, at(4).bg), (0x281414, 0x88442c));
        em.process(b"\x1b]11;#281414\x07");
        assert_eq!(
            color(
                Color::Named(NamedColor::DimRed),
                em.palette,
                em.term.colors()
            ),
            0x88442c
        );
    }

    #[test]
    fn shell_color_queries_see_the_color_at_the_request_and_rendering_uses_the_same_slots() {
        let mut em = Emulator::new(super::super::emulator::GridSize { rows: 2, cols: 4 });
        let commands =
            b"\x1b]10;#102030\x07\x1b]11;#3c4d5e\x07\x1b]12;#abcdef\x07\x1b]4;1;#123456\x07";
        em.process(commands);
        for (color_spec, expected) in [
            (Color::Named(NamedColor::Foreground), 0x102030),
            (Color::Named(NamedColor::Background), 0x3c4d5e),
            (Color::Named(NamedColor::Cursor), 0xabcdef),
            (Color::Named(NamedColor::Red), 0x123456),
            (Color::Indexed(1), 0x123456),
        ] {
            assert_eq!(color(color_spec, em.palette, em.term.colors()), expected);
        }
        em.process(b"\x1b]10;?;?;?\x1b");
        em.process(
            b"\\\x1b[6n\x1b]12;#010203\x07\x1b]12;?\x07\x1b]112\x07\x1b]110\x07\x1b]111\x07",
        );
        assert_eq!(em.finish_batch(), b"\x1b]10;rgb:1010/2020/3030\x1b\\\x1b]11;rgb:3c3c/4d4d/5e5e\x1b\\\x1b]12;rgb:abab/cdcd/efef\x1b\\\x1b[1;1R\x1b]12;rgb:0101/0202/0303\x07");
        assert_eq!(
            color(
                Color::Named(NamedColor::Cursor),
                em.palette,
                em.term.colors()
            ),
            em.palette.cursor
        );
        em.process(b"\x1b]52;c;?\x07");
        assert!(em.finish_batch().is_empty());
    }

    #[test]
    fn theme_changes_defaults_and_ansi_slots_but_keeps_explicit_terminal_colors() {
        let mut em = Emulator::new(super::super::emulator::GridSize { rows: 1, cols: 5 });
        em.process(b"A\x1b[31mB\x1b[38;5;67mC\x1b[38;2;31;99;227mD\x1b[0;7mE");
        for mode in [Appearance::Light, Appearance::Dark] {
            let palette = &crate::theme::resolve(mode, WindowAppearance::Light).terminal;
            let at = |col| {
                Style::from_cell(
                    &em.term.grid()[em.view_line(0)][Column(col)],
                    palette,
                    em.term.colors(),
                )
            };
            assert_eq!(at(0).fg, palette.surface.foreground);
            assert_eq!(at(0).bg, palette.surface.background);
            assert_eq!(at(1).fg, palette.ansi[1]);
            assert_eq!(at(2).fg, u32::from_be_bytes([0, 95, 135, 175]));
            assert_eq!(at(3).fg, u32::from_be_bytes([0, 31, 99, 227]));
            assert_eq!(at(4).fg, palette.surface.background);
            assert_eq!(at(4).bg, palette.surface.foreground);
        }
    }

    #[test]
    fn text_runs_keep_symbol_color_merges_same_ink_and_trims_trailing_spaces() {
        let face = font("monospace");
        let paint = |fg, bg| CellPaint {
            fg,
            bg,
            underline: false,
            block: false,
        };
        // "● ok  ": colored bullet (3 bytes), space+text across two bg spans, trailing spaces
        let text = "● ok  ";
        let spans = [
            PaintSpan {
                start: 0,
                end: 1,
                byte_len: "●".len(),
                paint: paint(0x00ff00, 0x000000),
            },
            PaintSpan {
                start: 1,
                end: 2,
                byte_len: 1,
                paint: paint(0xffffff, 0x000000),
            },
            PaintSpan {
                start: 2,
                end: 6,
                byte_len: 4,
                paint: paint(0xffffff, 0x222222),
            },
        ];
        let visible = text.trim_end_matches(' ');
        let runs = build_text_runs(visible.len(), &spans, &face, |p| p.fg);
        assert_eq!(runs.len(), 2);
        assert_eq!(runs[0].len, "●".len());
        assert_eq!(runs[0].color, rgb(0x00ff00).into());
        assert_eq!(runs[1].len, " ok".len());
        assert_eq!(runs[1].color, rgb(0xffffff).into());
        assert!(!has_rtl("● ok 中文 😀"));
        assert!(has_rtl("مرحبا"));
    }
}
