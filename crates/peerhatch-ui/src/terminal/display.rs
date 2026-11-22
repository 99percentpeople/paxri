use super::{
    cursor::{CursorCell, Preedit},
    emulator::Emulator,
};
use crate::components::caret::GlyphSpan;
use crate::theme::{TerminalPalette, fonts::TerminalFont};
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
    App, Bounds, FontStyle, FontWeight, Pixels, Point, TextRun, Window, fill, point, px, rgb, size,
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
struct Style {
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
            fg,
            bg,
            bold: cell.flags.contains(Flags::BOLD),
            italic: cell.flags.contains(Flags::ITALIC),
            underline: cell.flags.intersects(Flags::ALL_UNDERLINES),
        }
    }
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

fn paint_run(
    text: &str,
    cells: u16,
    typography: &TerminalFont,
    style: Style,
    background: u32,
    cursor_fill: Option<(Bounds<Pixels>, u32)>,
    at: Point<Pixels>,
    window: &mut Window,
    cx: &mut App,
) {
    let width = px(typography.cell.width * f32::from(cells));
    if style.bg != background {
        window.paint_quad(fill(
            Bounds::new(at, size(width, px(typography.cell.height))),
            rgb(style.bg),
        ));
    }
    if let Some((bounds, color)) = cursor_fill {
        window.paint_quad(fill(bounds, rgb(color)));
    }
    let visible = text.trim_end_matches(' ');
    if !visible.is_empty() {
        // ASCII padding has no glyph ink. Keep the original text for bidirectional shaping.
        let text = if visible.is_ascii() { visible } else { text };
        let mut face = typography.face.clone();
        if style.bold {
            face.weight = FontWeight::BOLD;
        }
        if style.italic {
            face.style = FontStyle::Italic;
        }
        let run = TextRun {
            len: text.len(),
            font: face,
            color: rgb(style.fg).into(),
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let shaped = window.text_system().shape_line(
            text.to_owned().into(),
            px(typography.size),
            &[run],
            Some(px(typography.cell.width)),
        );
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
    if style.underline {
        window.paint_quad(fill(
            Bounds::new(
                at + point(px(0.), px(typography.cell.height - 2.)),
                size(width, px(1.)),
            ),
            rgb(style.fg),
        ));
    }
}

fn cursor_text_color(background: u32) -> u32 {
    // Pick black or white using relative luminance so the glyph under a block stays legible.
    let [_, r, g, b] = background.to_be_bytes();
    let channel = |c: u8| {
        let s = f32::from(c) / 255.;
        if s <= 0.04045 {
            s / 12.92
        } else {
            ((s + 0.055) / 1.055).powf(2.4)
        }
    };
    let luminance = 0.2126 * channel(r) + 0.7152 * channel(g) + 0.0722 * channel(b);
    if luminance > 0.179 {
        0x000000
    } else {
        0xffffff
    }
}

pub fn paint(
    screen: &Emulator,
    bounds: Bounds<Pixels>,
    marked: Option<&Preedit>,
    cursor_shape: Option<CursorShape>,
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
        matches!(
            cursor_shape,
            Some(CursorShape::Block | CursorShape::Underline)
        ) && marked.is_none(),
    );
    let block = cursor
        .filter(|_| marked.is_none() && cursor_shape == Some(CursorShape::Block))
        .map(|cell| (cell, cursor_text_color(cursor_color)));
    let origin = |r: u16, c: u16| {
        bounds.origin
            + point(
                px(f32::from(c) * screen.font.cell.width),
                px(f32::from(r) * screen.font.cell.height),
            )
    };
    for row in 0..grid.screen_lines() as u16 {
        let line = screen.view_line(usize::from(row));
        let coord = |col: u16| GridPoint::new(line, Column(usize::from(col)));
        let cell_style = |cell: &Cell, col| {
            let mut style = Style::from_cell(cell, palette, overrides);
            if selection.is_some_and(|s| s.contains(coord(col))) {
                style.bg = palette.selection.background;
                style.fg = palette.selection.foreground;
            }
            if let Some((cell, fg)) = block
                && cell.covers(coord(col))
            {
                style.fg = fg;
            }
            style
        };
        let mut col = 0;
        while col < cols {
            let first = col;
            let cell = &grid[coord(col)];
            let style = cell_style(cell, col);
            let block_here = block.filter(|(cursor, _)| cursor.covers(coord(first)));
            let mut text = String::new();
            while col < cols {
                let cell = &grid[coord(col)];
                if cell_style(cell, col) != style
                    || block.is_some_and(|(cursor, _)| cursor.covers(coord(col)))
                        != block_here.is_some()
                    || (col > first && cell.flags.contains(Flags::WIDE_CHAR))
                {
                    break;
                }
                append_glyph(&mut text, cell);
                col += 1;
                if cell.flags.contains(Flags::WIDE_CHAR) {
                    if col < cols && grid[coord(col)].flags.contains(Flags::WIDE_CHAR_SPACER) {
                        col += 1;
                    }
                    break;
                }
            }
            paint_run(
                &text,
                col - first,
                &screen.font,
                style,
                background,
                block_here
                    .and_then(|(cursor, _)| {
                        cursor.stroke(
                            CursorShape::Block,
                            origin(row, first),
                            screen.font.cell,
                            span,
                        )
                    })
                    .map(|bounds| (bounds, cursor_color)),
                origin(row, first),
                window,
                cx,
            );
        }
    }
    if let Some(marked) = marked {
        if let Some(preedit) = marked.layout(screen, bounds.origin) {
            paint_run(
                preedit.text,
                preedit.columns,
                &screen.font,
                Style {
                    fg: cursor_text_color(palette.composition),
                    bg: palette.composition,
                    bold: false,
                    italic: false,
                    underline: true,
                },
                background,
                None,
                preedit.at,
                window,
                cx,
            );
            let caret = marked.selection.end;
            window.paint_quad(fill(
                span.in_row(preedit.bounds_for_range(caret..caret)),
                rgb(cursor_text_color(palette.composition)),
            ));
        }
    } else if let Some(cursor) = cursor {
        let at = origin(cursor.point.line.0 as u16, cursor.point.column.0 as u16);
        if let Some(bounds) = cursor_shape
            .filter(|shape| *shape != CursorShape::Block)
            .and_then(|shape| cursor.stroke(shape, at, screen.font.cell, span))
        {
            window.paint_quad(fill(bounds, rgb(cursor_color)));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::{Appearance, TerminalScheme};
    use gpui::WindowAppearance;

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
}
