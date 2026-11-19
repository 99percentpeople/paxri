use super::selection::Selection;
use gpui::{
    App, Bounds, FontStyle, FontWeight, Pixels, Point, TextRun, Window, fill, font, point, px, rgb,
    size,
};
use vt100::{Cell, Color, Screen};

pub const COL: f32 = 9.;
pub const LINE: f32 = 20.;
pub const BACKGROUND: u32 = 0x101917;
const FOREGROUND: u32 = 0xe4ede8;

fn color(value: Color, default: u32) -> u32 {
    const ANSI: [u32; 16] = [
        0x192220, 0xe07572, 0x82c998, 0xe3c780, 0x80aee1, 0xc896d6, 0x70c5c1, 0xcbd4cd, 0x78847e,
        0xff9290, 0xa3eeb2, 0xffdd94, 0xa2c7ff, 0xedb5ff, 0x94e9e2, 0xffffff,
    ];
    match value {
        Color::Default => default,
        Color::Rgb(r, g, b) => u32::from_be_bytes([0, r, g, b]),
        Color::Idx(i @ 0..16) => ANSI[i as usize],
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

impl From<&Cell> for Style {
    fn from(cell: &Cell) -> Self {
        let mut fg = color(cell.fgcolor(), FOREGROUND);
        let mut bg = color(cell.bgcolor(), BACKGROUND);
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
    at: Point<Pixels>,
    window: &mut Window,
    cx: &mut App,
) {
    let width = px(COL * f32::from(cells));
    if style.bg != BACKGROUND {
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
    let (rows, cols) = screen.size();
    let origin =
        |r: u16, c: u16| bounds.origin + point(px(f32::from(c) * COL), px(f32::from(r) * LINE));
    for row in 0..rows.get() {
        let selected =
            selection.map_or(0..0, |s| s.columns(top_row + usize::from(row), cols.get()));
        let cell_style = |cell: &Cell, col| {
            let mut style = Style::from(cell);
            if selected.contains(&col) {
                style.bg = 0x315f72;
                style.fg = 0xf5fcff;
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
            paint_run(&text, col - first, style, origin(row, first), window, cx);
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
                    fg: FOREGROUND,
                    bg: 0x31443a,
                    bold: false,
                    italic: false,
                    underline: true,
                },
                at,
                window,
                cx,
            );
        } else {
            window.paint_quad(fill(
                Bounds::new(at + point(px(0.), px(LINE - 2.)), size(px(COL), px(2.))),
                rgb(0x93ddad),
            ));
        }
    }
}
