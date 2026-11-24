use super::Terminal;
use alacritty_terminal::{
    grid::{Dimensions, Scroll},
    term::TermMode,
};
use gpui::{
    ClipboardItem, Context, Modifiers, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
    Pixels, Point, ScrollWheelEvent, Window,
};

use paxri_ui::terminal::mouse::{Cell, Wheel, WheelTarget, report, wheel_input};

fn button_code(button: MouseButton) -> Option<u8> {
    match button {
        MouseButton::Left => Some(0),
        MouseButton::Middle => Some(1),
        MouseButton::Right => Some(2),
        MouseButton::Navigate(_) => None,
    }
}

#[derive(Default)]
pub(super) struct Mouse {
    held: u8,
    last_motion: Option<(Cell, u8)>,
    wheel: Wheel,
}

impl Mouse {
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

    pub(super) fn mouse_cell(&self, position: Point<Pixels>) -> Cell {
        let (row, x) = self.cell_at(position);
        Cell {
            col: x
                .floor()
                .clamp(0., self.emulator.term.last_column().0 as f32) as u16
                + 1,
            row: row as u16 + 1,
        }
    }

    pub(super) fn mouse_enabled(&self) -> bool {
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
        self.cancel_touch();
        self.touch.handles = false;
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
        } else if e.button == MouseButton::Right
            && self.mouse.held == 0
            && self.viewport.contains(&e.position)
        {
            self.focus.focus(w);
            if let Some(text) = self.emulator.selected_text() {
                cx.write_to_clipboard(ClipboardItem::new_string(text));
                self.emulator.term.selection = None;
                self.selecting = false;
            } else {
                self.paste(w, cx);
            }
            cx.stop_propagation();
            cx.notify();
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
        let cell = self.emulator.font.cell;
        let delta = match e.delta {
            gpui::ScrollDelta::Pixels(p) => gpui::point(f32::from(p.x) / cell.width, f32::from(p.y) / cell.height),
            gpui::ScrollDelta::Lines(p) => p,
        };
        let (x, y) = self.mouse.wheel.steps(target, delta);
        match target {
            WheelTarget::Mouse | WheelTarget::Arrows => {
                let bytes = wheel_input(target, mode, self.mouse_cell(e.position), (x, y), e.modifiers);
                if !bytes.is_empty() {
                    self.send(bytes);
                }
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
