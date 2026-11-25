use super::*;
use alacritty_terminal::grid::Dimensions;
use paxri_ui::terminal::mouse::{Cell, TouchMouseDrag, WheelTarget, report, wheel_input};

pub(super) enum Gesture {
    Keyboard,
    Tap {
        x: f32,
        y: f32,
    },
    Select {
        x: f32,
        y: f32,
        start: bool,
    },
    AppDrag {
        action: i32,
        x: f32,
        y: f32,
    },
    MoveHandle {
        x: f32,
        y: f32,
        leading: bool,
    },
    FinishSelection {
        copy: bool,
    },
    Paste,
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

// A drag after long press can use the foreground TUI's own selection.
static DRAG_SESSION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

#[unsafe(no_mangle)]
pub extern "system" fn Java_org_paxri_android_TerminalGestures_wantsAppDrag(
    _env: EnvUnowned,
    _class: JClass,
    session: i64,
) -> bool {
    session != 0 && DRAG_SESSION.load(Ordering::Relaxed) == session as u64
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_org_paxri_android_TerminalGestures_appDrag(
    _env: EnvUnowned,
    _class: JClass,
    session: i64,
    action: i32,
    x: f32,
    y: f32,
) {
    post(Update::Gesture(
        session as u64,
        Gesture::AppDrag { action, x, y },
    ));
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
    x: f32,
    y: f32,
) {
    post(Update::Gesture(session as u64, Gesture::Tap { x, y }));
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_org_paxri_android_TerminalGestures_select(
    _env: EnvUnowned,
    _class: JClass,
    session: i64,
    x: f32,
    y: f32,
    start: bool,
) {
    post(Update::Gesture(
        session as u64,
        Gesture::Select { x, y, start },
    ));
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_org_paxri_android_TerminalGestures_moveHandle(
    _env: EnvUnowned,
    _class: JClass,
    session: i64,
    x: f32,
    y: f32,
    leading: bool,
) {
    post(Update::Gesture(
        session as u64,
        Gesture::MoveHandle { x, y, leading },
    ));
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_org_paxri_android_TerminalGestures_finishSelection(
    _env: EnvUnowned,
    _class: JClass,
    session: i64,
    copy: bool,
) {
    post(Update::Gesture(
        session as u64,
        Gesture::FinishSelection { copy },
    ));
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_org_paxri_android_TerminalGestures_paste(
    _env: EnvUnowned,
    _class: JClass,
    session: i64,
) {
    post(Update::Gesture(session as u64, Gesture::Paste));
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_org_paxri_android_TerminalGestures_keyboard(
    _env: EnvUnowned,
    _class: JClass,
    session: i64,
) {
    post(Update::Gesture(session as u64, Gesture::Keyboard));
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

impl Session {
    pub(super) fn handle_positions(&self, scale: f32) -> [i32; 5] {
        let Some([start, end]) = self.screen.selection_handles() else {
            return [0; 5];
        };
        [start.x, start.y, end.x, end.y, self.screen.font.cell.height]
            .map(|v| (v * scale).round() as i32)
    }

    fn touch_position(&self, x: f32, y: f32) -> (usize, f32) {
        let cell = self.screen.font.cell;
        let last_row = self.screen.term.screen_lines() - 1;
        let row = (y / cell.height).floor().clamp(0., last_row as f32);
        (row as usize, x / cell.width)
    }

    fn mouse_cell(&self, x: f32, y: f32) -> Cell {
        let (row, col) = self.touch_position(x, y);
        Cell {
            row: row as u16 + 1,
            col: col
                .floor()
                .clamp(0., self.screen.term.last_column().0 as f32) as u16
                + 1,
        }
    }

    fn mouse_enabled(&self) -> bool {
        self.connected()
            && self.screen.term.grid().display_offset() == 0
            && self.screen.term.mode().intersects(TermMode::MOUSE_MODE)
    }
}

impl Mobile {
    pub(super) fn show_terminal_keyboard(&self, id: u64) {
        if let Err(error) = activity_action(&self.android, ActivityAction::ShowTerminalKeyboard(id)) {
            log::warn!("Showing terminal keyboard: {error}");
        }
    }

    pub(super) fn set_touch_area(&mut self, id: u64, rect: [i32; 5], keys: Arc<KeybarSettings>, touchable: bool) {
        let drag = self
            .sessions
            .get(&id)
            .is_some_and(|s| s.mouse_enabled() && TouchMouseDrag::requested(*s.screen.term.mode()));
        DRAG_SESSION.store(if drag { id } else { 0 }, Ordering::Relaxed);
        if self
            .touch_area
            .as_ref()
            .is_some_and(|(session, bounds, layout, enabled)| {
                *session == id && *bounds == rect && *layout == keys && *enabled == touchable
            })
        {
            return;
        }
        match activity_action(&self.android, ActivityAction::TouchArea(id, rect, &keys, touchable)) {
            Ok(()) => self.touch_area = Some((id, rect, keys, touchable)),
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
            Gesture::Keyboard | Gesture::Tap { .. } if s.connected() => {
                if let Gesture::Tap { x, y } = gesture
                    && s.mouse_enabled()
                {
                    let mode = *s.screen.term.mode();
                    let cell = s.mouse_cell(x, y);
                    let mut click = report(mode, cell, 0, false, Modifiers::default());
                    if !click.is_empty() {
                        click.extend(report(mode, cell, 0, true, Modifiers::default()));
                        let _ = s.input.send(Frame::Data(click));
                    }
                }
                self.show_terminal_keyboard(id);
            }
            Gesture::Keyboard | Gesture::Tap { .. } => {}
            Gesture::Select { x, y, start } => {
                if !start {
                    let height = s.screen.term.screen_lines() as f32 * s.screen.font.cell.height;
                    if y < 0. {
                        s.screen.scroll(Scroll::Delta(1));
                    }
                    if y >= height {
                        s.screen.scroll(Scroll::Delta(-1));
                    }
                }
                let (row, col) = s.touch_position(x, y);
                if start {
                    s.screen.start_touch_selection(row, col);
                } else {
                    s.screen.extend_touch_selection(row, col);
                }
            }
            Gesture::AppDrag { action, x, y } => {
                let cell = s.mouse_cell(x, y);
                let mode = *s.screen.term.mode();
                let bytes = match action {
                    0 => {
                        s.screen.term.selection = None;
                        if s.mouse_enabled() {
                            s.touch_mouse.begin(mode, cell)
                        } else {
                            Vec::new()
                        }
                    }
                    1 => s.touch_mouse.end(mode),
                    2 => s.touch_mouse.motion(mode, cell),
                    _ => return,
                };
                if !bytes.is_empty() {
                    let _ = s.input.send(Frame::Data(bytes));
                }
            }
            Gesture::MoveHandle { x, y, leading } => {
                let (row, col) = s.touch_position(x, y);
                s.screen.move_selection_handle(leading, row, col);
            }
            Gesture::FinishSelection { copy } => {
                if copy
                    && let Some(text) = s.screen.selected_text().filter(|text| !text.is_empty())
                    && let Err(error) = gpui_mobile::packages::clipboard::set_text(&text)
                {
                    Toast::show(error, self.window, cx);
                }
                s.screen.term.selection = None;
            }
            Gesture::Paste => {
                if s.connected() {
                    match gpui_mobile::packages::clipboard::get_text() {
                        Ok(Some(text)) => {
                            let bracketed = s.screen.term.mode().contains(TermMode::BRACKETED_PASTE);
                            s.write_input(keys::paste(&text, bracketed));
                        }
                        Ok(None) => {}
                        Err(error) => Toast::show(error, self.window, cx),
                    }
                }
            }
            Gesture::Scroll { dy, x, y, fling } => {
                let mode = *s.screen.term.mode();
                let target = if s.mouse_enabled() {
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
                    let cell = s.mouse_cell(x, y);
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
