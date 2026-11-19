use super::selection::Selection;
use crate::theme::{self, TerminalPalette};
use gpui::{
    App, Bounds, FontStyle, FontWeight, Pixels, Point, TextRun, Window, fill, font, point, px, rgb,
    size,
};
use vt100::{Cell, Color, Screen};

pub const COL: f32 = 9.;
pub const LINE: f32 = 20.;
fn color(value: Color, default: u32, palette: &TerminalPalette) -> u32 {
    match value {
        Color::Default => default,
        Color::Rgb(r, g, b) => u32::from_be_bytes([0, r, g, b]),
        Color::Idx(i @ 0..16) => palette.ansi[i as usize],
        Color::Idx(i @ 16..232) => {
            let i = i - 16;
            let scale = [0, 95, 135, 175, 215, 255];
            u32::from_be_bytes([
                0,
                scale[(i / 36) as usize],
                scale[(i / 6 % 6) as usize],
                scale[(i % 6) as usize],
            ])
        }
        Color::Idx(i) => {
            let grey = 8 + (i - 232) * 10;
            u32::from_be_bytes([0, grey, grey, grey])
        }
    }
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
    fn from_cell(cell: &Cell, palette: &TerminalPalette) -> Self {
        let mut fg = color(cell.fgcolor(), palette.surface.foreground, palette);
        let mut bg = color(cell.bgcolor(), palette.surface.background, palette);
        if cell.inverse() {
            std::mem::swap(&mut fg, &mut bg);
        }
        Self {
            fg,
            bg,
            bold: cell.bold(),
            italic: cell.italic(),
            underline: cell.underline(),
        }
    }
}

fn paint_run(
    text: &str,
    cells: u16,
    style: Style,
    palette: &TerminalPalette,
    at: Point<Pixels>,
    window: &mut Window,
    cx: &mut App,
) {
    let width = px(COL * f32::from(cells));
    if style.bg != palette.surface.background {
        window.paint_quad(fill(Bounds::new(at, size(width, px(LINE))), rgb(style.bg)));
    }
    if !text.trim_end_matches(' ').is_empty() {
        let mut face = font(if cfg!(windows) {
            "Consolas"
        } else if cfg!(target_os = "macos") {
            "Menlo"
        } else {
            "DejaVu Sans Mono"
        });
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
        let shaped =
            window
                .text_system()
                .shape_line(text.to_owned().into(), px(15.), &[run], Some(px(COL)));
        let _ = shaped.paint(at, px(LINE), window, cx);
    }
    if style.underline {
        window.paint_quad(fill(
            Bounds::new(at + point(px(0.), px(LINE - 2.)), size(width, px(1.))),
            rgb(style.fg),
        ));
    }
}

pub fn paint(
    screen: &Screen,
    bounds: Bounds<Pixels>,
    marked: &str,
    selection: Option<&Selection>,
    top_row: usize,
    window: &mut Window,
    cx: &mut App,
) {
    let palette = &theme::palette(window, cx).terminal;
    let (rows, cols) = screen.size();
    let origin =
        |r: u16, c: u16| bounds.origin + point(px(f32::from(c) * COL), px(f32::from(r) * LINE));
    for row in 0..rows.get() {
        let selected =
            selection.map_or(0..0, |s| s.columns(top_row + usize::from(row), cols.get()));
        let cell_style = |cell: &Cell, col| {
            let mut style = Style::from_cell(cell, palette);
            if selected.contains(&col) {
                style.bg = palette.selection.background;
                style.fg = palette.selection.foreground;
            }
            style
        };
        let mut col = 0;
        while col < cols.get() {
            let first = col;
            let cell = screen.cell(row, col).unwrap();
            let style = cell_style(cell, col);
            let mut text = String::new();
            while col < cols.get() {
                let cell = screen.cell(row, col).unwrap();
                if cell_style(cell, col) != style || (col > first && cell.is_wide()) {
                    break;
                }
                if !cell.is_wide_continuation() {
                    if cell.has_contents() {
                        text.push_str(cell.contents());
                    } else {
                        text.push(' ');
                    }
                }
                col += 1;
                if cell.is_wide() {
                    if col < cols.get() && screen.cell(row, col).unwrap().is_wide_continuation() {
                        col += 1;
                    }
                    break;
                }
            }
            paint_run(
                &text,
                col - first,
                style,
                palette,
                origin(row, first),
                window,
                cx,
            );
        }
    }
    if !screen.hide_cursor() && screen.scrollback() == 0 {
        let (r, c) = screen.cursor_position();
        let at = origin(r, c.min(cols.get() - 1));
        if !marked.is_empty() {
            paint_run(
                marked,
                marked.chars().count().min(200) as u16,
                Style {
                    fg: palette.surface.foreground,
                    bg: palette.composition,
                    bold: false,
                    italic: false,
                    underline: true,
                },
                palette,
                at,
                window,
                cx,
            );
        } else {
            window.paint_quad(fill(
                Bounds::new(at + point(px(0.), px(LINE - 2.)), size(px(COL), px(2.))),
                rgb(palette.cursor),
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::Appearance;
    use gpui::WindowAppearance;

    #[test]
    fn theme_changes_defaults_and_ansi_slots_but_keeps_explicit_terminal_colors() {
        let mut parser = vt100::Parser::new(1.try_into().unwrap(), 5.try_into().unwrap(), 0);
        parser.process(b"A\x1b[31mB\x1b[38;5;67mC\x1b[38;2;31;99;227mD\x1b[0;7mE");
        for mode in [Appearance::Light, Appearance::Dark] {
            let palette = &mode.resolve(WindowAppearance::Light).terminal;
            let at = |col| Style::from_cell(parser.screen().cell(0, col).unwrap(), palette);
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
