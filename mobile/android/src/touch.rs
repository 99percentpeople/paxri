use super::*;
use alacritty_terminal::grid::Dimensions;
use paxri_ui::terminal::mouse::{Cell, WheelTarget, wheel_input};

pub(super) enum Gesture {
    Tap,
    Scroll {
        dy: f32,
        x: f32,
        y: f32,
        fling: bool,
    },
    Zoom {
        factor: f32,
        finished: bool,
    },
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_org_paxri_android_PaxriActivity_startKeyboardSession(
    _env: EnvUnowned,
    _class: JClass,
) {
    gpui_mobile::show_keyboard();
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_org_paxri_android_TerminalKeybar_controlRadius(
    _env: EnvUnowned,
    _class: JClass,
) -> f32 {
    theme::CONTROL_RADIUS.into()
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_org_paxri_android_TerminalGestures_tap(
    _env: EnvUnowned,
    _class: JClass,
    session: i64,
) {
    post(Update::Gesture(session as u64, Gesture::Tap));
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_org_paxri_android_TerminalGestures_scroll(
    _env: EnvUnowned,
    _class: JClass,
    session: i64,
    dy: f32,
    x: f32,
    y: f32,
    fling: bool,
) {
    post(Update::Gesture(
        session as u64,
        Gesture::Scroll { dy, x, y, fling },
    ));
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_org_paxri_android_TerminalGestures_zoom(
    _env: EnvUnowned,
    _class: JClass,
    session: i64,
    factor: f32,
    finished: bool,
) {
    post(Update::Gesture(
        session as u64,
        Gesture::Zoom { factor, finished },
    ));
}

impl Mobile {
    pub(super) fn set_touch_area(&mut self, id: u64, rect: [i32; 5], keys: Arc<KeybarSettings>) {
        if self.touch_area.as_ref().is_some_and(|(session, bounds, layout)| {
            *session == id && *bounds == rect && *layout == keys
        }) {
            return;
        }
        match activity_action(&self.android, ActivityAction::TouchArea(id, rect, &keys)) {
            Ok(()) => self.touch_area = Some((id, rect, keys)),
            Err(error) => log::warn!("Setting terminal touch area: {error}"),
        }
    }

    pub(super) fn handle_gesture(&mut self, id: u64, gesture: Gesture, cx: &mut Context<Self>) {
        if self.page != Page::Terminal(id) {
            return;
        }
        let Some(s) = self.sessions.get_mut(&id) else {
            return;
        };
        match gesture {
            Gesture::Tap if s.connected() => {
                if let Err(error) =
                    activity_action(&self.android, ActivityAction::ShowTerminalKeyboard(id))
                {
                    log::warn!("Showing terminal keyboard: {error}");
                }
            }
            Gesture::Tap => {}
            Gesture::Scroll { dy, x, y, fling } => {
                let mode = *s.screen.term.mode();
                let target = if s.connected()
                    && s.screen.term.grid().display_offset() == 0
                    && mode.intersects(TermMode::MOUSE_MODE)
                {
                    WheelTarget::Mouse
                } else if s.connected()
                    && mode.contains(TermMode::ALT_SCREEN | TermMode::ALTERNATE_SCROLL)
                {
                    WheelTarget::Arrows
                } else {
                    WheelTarget::History
                };
                let cell = s.screen.font.cell;
                let steps = s.wheel.steps(target, point(0., dy / cell.height));
                if steps.1 == 0 {
                    return;
                }
                if target == WheelTarget::History {
                    s.screen.scroll(Scroll::Delta(steps.1));
                    let grid = s.screen.term.grid();
                    let at_edge = if steps.1 > 0 {
                        grid.display_offset() == grid.history_size()
                    } else {
                        grid.display_offset() == 0
                    };
                    if fling && at_edge {
                        let _ = activity_action(&self.android, ActivityAction::StopFling(id));
                    }
                } else {
                    let grid = s.screen.term.grid();
                    let cell = Cell {
                        col: (x / cell.width)
                            .floor()
                            .clamp(0., grid.last_column().0 as f32)
                            as u16
                            + 1,
                        row: (y / cell.height)
                            .floor()
                            .clamp(0., (grid.screen_lines() - 1) as f32)
                            as u16
                            + 1,
                    };
                    let bytes = wheel_input(target, mode, cell, steps, Modifiers::default());
                    if !bytes.is_empty() {
                        let _ = s.input.send(Frame::Data(bytes));
                    }
                }
            }
            Gesture::Zoom { factor, finished } => {
                self.apply_zoom(theme::Zoom::Pinch(factor), finished, cx);
            }
        }
    }
}
