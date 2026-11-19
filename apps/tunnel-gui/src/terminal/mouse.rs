use super::{LINE, Terminal};
use alacritty_terminal::{
    grid::{Dimensions, Scroll},
    term::TermMode,
};
use gpui::{
    Context, Modifiers, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Point,
    ScrollWheelEvent, Window, px,
};

// xterm pointer coordinates are one-based character cells, including in SGR mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Cell {
    col: u16,
    row: u16,
}

fn button_code(button: MouseButton) -> Option<u8> {
    match button {
        MouseButton::Left => Some(0),
        MouseButton::Middle => Some(1),
        MouseButton::Right => Some(2),
        MouseButton::Navigate(_) => None,
    }
}

fn report(mode: TermMode, cell: Cell, button: u8, release: bool, mods: Modifiers) -> Vec<u8> {
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
enum WheelTarget {
    #[default]
    History,
    Mouse,
    Arrows,
}

#[derive(Default)]
pub(super) struct Mouse {
    held: u8,
    last_motion: Option<(Cell, u8)>,
    wheel_target: WheelTarget,
    remainder: Point<f32>,
}

impl Mouse {
    fn wheel_steps(&mut self, target: WheelTarget, delta: Point<f32>) -> (i32, i32) {
        if self.wheel_target != target {
            self.remainder = Point::default();
            self.wheel_target = target;
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

    pub(super) fn reset(&mut self) {
        *self = Self::default();
    }
}

impl Terminal {
    pub(super) fn release_pointer(&mut self) {
        if self.mouse_enabled()
            && let Some((cell, _)) = self.mouse.last_motion
        {
            for code in 0..3 {
                if self.mouse.held & (1 << code) != 0 {
                    self.send(report(
                        *self.emulator.term.mode(),
                        cell,
                        code,
                        true,
                        Modifiers::default(),
                    ));
                }
            }
        }
        self.mouse.reset();
    }

    fn mouse_cell(&self, position: Point<Pixels>) -> Cell {
        let (row, x) = self.cell_at(position);
        Cell {
            col: x
                .floor()
                .clamp(0., self.emulator.term.last_column().0 as f32) as u16
                + 1,
            row: row as u16 + 1,
        }
    }

    fn mouse_enabled(&self) -> bool {
        self.is_running()
            && self.emulator.term.grid().display_offset() == 0
            && self.emulator.term.mode().intersects(TermMode::MOUSE_MODE)
    }

    pub(super) fn mouse_down(
        &mut self,
        e: &MouseDownEvent,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.mouse_enabled()
            && !e.modifiers.shift
            && !self.selecting
            && self.viewport.contains(&e.position)
        {
            let Some(code) = button_code(e.button) else {
                return;
            };
            self.focus.focus(w);
            let cell = self.mouse_cell(e.position);
            self.mouse.held |= 1 << code;
            self.mouse.last_motion = Some((cell, code + 32));
            self.emulator.term.selection = None;
            self.send(report(
                *self.emulator.term.mode(),
                cell,
                code,
                false,
                e.modifiers,
            ));
            cx.stop_propagation();
            cx.notify();
        } else if e.button == MouseButton::Left && self.mouse.held == 0 {
            self.start_selection(e, w, cx);
        }
    }

    // Window capture keeps a reported drag paired with its release even across the viewport edge.
    pub(super) fn mouse_up(&mut self, e: &MouseUpEvent, cx: &mut Context<Self>) {
        if let Some(code) = button_code(e.button)
            && self.mouse.held & (1 << code) != 0
        {
            self.mouse.held &= !(1 << code);
            let cell = self.mouse_cell(e.position);
            self.mouse.last_motion = Some((cell, 35));
            if self.mouse_enabled() {
                self.send(report(
                    *self.emulator.term.mode(),
                    cell,
                    code,
                    true,
                    e.modifiers,
                ));
            }
            cx.stop_propagation();
        } else if self.selecting && e.button == MouseButton::Left {
            self.selecting = false;
            self.extend_selection(e.position, cx);
        }
    }

    fn motion(&mut self, e: &MouseMoveEvent, button: u8) {
        let cell = self.mouse_cell(e.position);
        let code = 32 + button;
        if self.mouse.last_motion != Some((cell, code)) {
            self.mouse.last_motion = Some((cell, code));
            self.send(report(
                *self.emulator.term.mode(),
                cell,
                code,
                false,
                e.modifiers,
            ));
        }
    }

    pub(super) fn mouse_drag(&mut self, e: &MouseMoveEvent, cx: &mut Context<Self>) {
        if self.selecting {
            if e.pressed_button != Some(MouseButton::Left) {
                self.selecting = false;
                return;
            }
            if e.position.y < self.viewport.top() {
                self.emulator.scroll(Scroll::Delta(1));
            } else if e.position.y >= self.viewport.bottom() {
                self.emulator.scroll(Scroll::Delta(-1));
            }
            self.extend_selection(e.position, cx);
        } else if let Some(code) = e.pressed_button.and_then(button_code)
            && self.mouse.held & (1 << code) != 0
            && self.mouse_enabled()
            && self
                .emulator
                .term
                .mode()
                .intersects(TermMode::MOUSE_DRAG | TermMode::MOUSE_MOTION)
        {
            self.motion(e, code);
        }
    }

    pub(super) fn mouse_hover(
        &mut self,
        e: &MouseMoveEvent,
        _: &mut Window,
        _: &mut Context<Self>,
    ) {
        if e.pressed_button.is_none()
            && !e.modifiers.shift
            && !self.selecting
            && self.mouse.held == 0
            && self.mouse_enabled()
            && self.emulator.term.mode().contains(TermMode::MOUSE_MOTION)
            && self.viewport.contains(&e.position)
        {
            self.motion(e, 3);
        }
    }

    pub(super) fn wheel(&mut self, e: &ScrollWheelEvent, _: &mut Window, cx: &mut Context<Self>) {
        let mode = *self.emulator.term.mode();
        let target = if self.mouse_enabled() && !e.modifiers.shift && !self.selecting {
            WheelTarget::Mouse
        } else if !e.modifiers.shift
            && self.is_running()
            && mode.contains(TermMode::ALT_SCREEN | TermMode::ALTERNATE_SCROLL)
        {
            WheelTarget::Arrows
        } else {
            WheelTarget::History
        };
        let delta = e.delta.pixel_delta(px(LINE)).map(|v| f32::from(v) / LINE);
        let (x, y) = self.mouse.wheel_steps(target, delta);
        match target {
            WheelTarget::Mouse => {
                let cell = self.mouse_cell(e.position);
                for (steps, neg, pos) in [(y, 65, 64), (x, 67, 66)] {
                    if steps != 0 {
                        let code = if steps > 0 { pos } else { neg };
                        let bytes = report(mode, cell, code, false, e.modifiers);
                        self.send(bytes.repeat(steps.unsigned_abs() as usize));
                    }
                }
            }
            WheelTarget::Arrows if y != 0 => {
                let prefix = if mode.contains(TermMode::APP_CURSOR) {
                    b'O'
                } else {
                    b'['
                };
                let arrow = if y > 0 { b'A' } else { b'B' };
                self.send([0x1b, prefix, arrow].repeat(y.unsigned_abs() as usize));
            }
            WheelTarget::History if y != 0 => {
                self.emulator.scroll(Scroll::Delta(y));
                if self.selecting {
                    self.extend_selection(e.position, cx);
                }
            }
            _ => {}
        }
        cx.stop_propagation();
        cx.notify();
    }
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
    fn fractional_wheel_motion_is_accumulated_only_for_the_same_destination() {
        let mut mouse = Mouse::default();
        assert_eq!(
            mouse.wheel_steps(WheelTarget::Mouse, point(0.6, -0.75)),
            (0, 0)
        );
        assert_eq!(
            mouse.wheel_steps(WheelTarget::Mouse, point(0.6, -0.75)),
            (1, -1)
        );
        assert_eq!(
            mouse.wheel_steps(WheelTarget::History, point(0., -0.75)),
            (0, 0)
        );
    }
}
