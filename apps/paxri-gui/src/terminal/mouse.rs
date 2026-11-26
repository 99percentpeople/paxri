use super::Terminal;
use alacritty_terminal::{grid::Scroll, term::TermMode};
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
    pub(super) held: u8,
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
        let local = position - self.viewport.origin;
        self.emulator
            .mouse_cell(f32::from(local.x), f32::from(local.y))
    }

    pub(super) fn mouse_enabled(&self) -> bool {
        self.emulator.mouse_enabled(self.is_running())
    }

    pub(super) fn mouse_down(
        &mut self,
        e: &MouseDownEvent,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.hidden_pointer_at = None;
        self.cancel_touch();
        self.touch.handles = false;
        if e.button == MouseButton::Left
            && self.mouse.held == 0
            && Self::link_modifier_active(e.modifiers)
            && let Some(link) = self.link_at(e.position, cx)
        {
            self.focus.focus(w, cx);
            self.link_modifier = true;
            self.pending_link = Some(link.clone());
            self.hovered_link = Some(link);
            cx.stop_propagation();
            cx.notify();
            return;
        }
        self.pending_link = None;
        // A selection made locally with Shift can still be copied after Shift is released.
        let copy_local_selection = e.button == MouseButton::Right
            && self.mouse.held == 0
            && self.emulator.selected_range().is_some();
        if self.mouse_enabled()
            && !copy_local_selection
            && !e.modifiers.shift
            && !self.selecting
            && self.viewport.contains(&e.position)
        {
            let Some(code) = button_code(e.button) else {
                return;
            };
            self.focus.focus(w, cx);
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
            self.focus.focus(w, cx);
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
        if e.button == MouseButton::Left
            && let Some(pending) = self.pending_link.take()
        {
            if Self::link_modifier_active(e.modifiers)
                && self
                    .link_at(e.position, cx)
                    .is_some_and(|current| current.uri == pending.uri)
            {
                cx.open_url(&pending.uri);
            }
            cx.stop_propagation();
            cx.notify();
            return;
        }
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
            self.emulator.scroll_for_edge(
                f32::from(e.position.y),
                f32::from(self.viewport.top()),
                f32::from(self.viewport.bottom()),
            );
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
        cx: &mut Context<Self>,
    ) {
        self.update_hovered_link(e.position, e.modifiers, cx);
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
        self.hidden_pointer_at = None;
        let mode = *self.emulator.term.mode();
        let target = self
            .emulator
            .wheel_target(self.is_running(), e.modifiers.shift || self.selecting);
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
