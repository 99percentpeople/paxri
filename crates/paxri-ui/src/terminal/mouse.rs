use alacritty_terminal::term::TermMode;
use gpui::{Modifiers, Point};

// xterm pointer coordinates are one-based character cells, including in SGR mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cell {
    pub col: u16,
    pub row: u16,
}

pub fn report(mode: TermMode, cell: Cell, button: u8, release: bool, mods: Modifiers) -> Vec<u8> {
    let modifiers = u8::from(mods.shift) * 4 + u8::from(mods.alt) * 8 + u8::from(mods.control) * 16;
    if mode.contains(TermMode::SGR_MOUSE) {
        let final_byte = if release { 'm' } else { 'M' };
        return format!(
            "\x1b[<{};{};{}{final_byte}",
            button + modifiers,
            cell.col,
            cell.row
        )
        .into_bytes();
    }
    let utf8 = mode.contains(TermMode::UTF8_MOUSE);
    let limit = if utf8 { 2015 } else { 223 };
    if cell.col > limit || cell.row > limit {
        return Vec::new();
    }
    let code = if release { 3 } else { button };
    let mut bytes = vec![0x1b, b'[', b'M', 32 + code + modifiers];
    for coord in [cell.col, cell.row] {
        if utf8 {
            bytes.extend_from_slice(
                char::from_u32(u32::from(coord) + 32)
                    .unwrap()
                    .encode_utf8(&mut [0; 4])
                    .as_bytes(),
            );
        } else {
            bytes.push(coord as u8 + 32);
        }
    }
    bytes
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub enum WheelTarget {
    #[default]
    History,
    Mouse,
    Arrows,
}

#[derive(Default)]
pub struct Wheel {
    target: WheelTarget,
    remainder: Point<f32>,
}

impl Wheel {
    pub fn steps(&mut self, target: WheelTarget, delta: Point<f32>) -> (i32, i32) {
        if self.target != target {
            self.remainder = Point::default();
            self.target = target;
        }
        self.remainder += delta;
        let steps = (
            self.remainder.x.trunc() as i32,
            self.remainder.y.trunc() as i32,
        );
        self.remainder.x = self.remainder.x.fract();
        self.remainder.y = self.remainder.y.fract();
        steps
    }
}

/// Encode wheel movement for a remote application that requested terminal mouse or arrow input.
pub fn wheel_input(
    target: WheelTarget,
    mode: TermMode,
    cell: Cell,
    steps: (i32, i32),
    mods: Modifiers,
) -> Vec<u8> {
    let (x, y) = steps;
    if target == WheelTarget::Arrows && y != 0 {
        let prefix = if mode.contains(TermMode::APP_CURSOR) {
            b'O'
        } else {
            b'['
        };
        let arrow = if y > 0 { b'A' } else { b'B' };
        return [0x1b, prefix, arrow].repeat(y.unsigned_abs() as usize);
    }
    let mut bytes = Vec::new();
    if target == WheelTarget::Mouse {
        for (steps, neg, pos) in [(y, 65, 64), (x, 67, 66)] {
            if steps != 0 {
                let code = if steps > 0 { pos } else { neg };
                bytes.extend(
                    report(mode, cell, code, false, mods).repeat(steps.unsigned_abs() as usize),
                );
            }
        }
    }
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::point;

    #[test]
    fn sgr_click_drag_hover_release_and_wheel_use_one_based_cells_and_modifiers() {
        let mode = TermMode::SGR_MOUSE;
        let cell = Cell { col: 300, row: 7 };
        let plain = Modifiers::default();
        assert_eq!(report(mode, cell, 0, false, plain), b"\x1b[<0;300;7M");
        assert_eq!(
            report(mode, cell, 34, false, Modifiers::control()),
            b"\x1b[<50;300;7M"
        );
        assert_eq!(report(mode, cell, 35, false, plain), b"\x1b[<35;300;7M");
        assert_eq!(report(mode, cell, 2, true, plain), b"\x1b[<2;300;7m");
        assert_eq!(
            report(mode, cell, 65, false, Modifiers::alt()),
            b"\x1b[<73;300;7M"
        );
    }

    #[test]
    fn legacy_reports_preserve_high_bytes_and_utf8_coordinates() {
        let legacy = TermMode::MOUSE_REPORT_CLICK;
        let plain = Modifiers::default();
        let cell = Cell { col: 223, row: 1 };
        assert_eq!(report(legacy, cell, 2, true, plain), b"\x1b[M#\xff!");
        let cell = Cell { col: 224, row: 1 };
        assert!(report(legacy, cell, 2, false, plain).is_empty());
        assert_eq!(
            report(legacy | TermMode::UTF8_MOUSE, cell, 2, false, plain),
            b"\x1b[M\"\xc4\x80!"
        );
    }

    #[test]
    fn wheel_input_uses_the_active_remote_protocol() {
        let cell = Cell { col: 12, row: 4 };
        let mods = Modifiers::default();
        assert_eq!(
            wheel_input(WheelTarget::Mouse, TermMode::SGR_MOUSE, cell, (0, -2), mods),
            b"\x1b[<65;12;4M\x1b[<65;12;4M"
        );
        assert_eq!(
            wheel_input(
                WheelTarget::Arrows,
                TermMode::APP_CURSOR,
                cell,
                (0, 1),
                mods
            ),
            b"\x1bOA"
        );
        assert_eq!(
            wheel_input(WheelTarget::Arrows, TermMode::empty(), cell, (0, -1), mods),
            b"\x1b[B"
        );
    }

    #[test]
    fn fractional_wheel_motion_is_accumulated_only_for_the_same_destination() {
        let mut wheel = Wheel::default();
        assert_eq!(wheel.steps(WheelTarget::Mouse, point(0.6, -0.75)), (0, 0));
        assert_eq!(wheel.steps(WheelTarget::Mouse, point(0.6, -0.75)), (1, -1));
        assert_eq!(wheel.steps(WheelTarget::History, point(0., -0.75)), (0, 0));
    }
}
