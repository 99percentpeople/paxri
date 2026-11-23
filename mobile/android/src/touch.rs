use super::*;
use alacritty_terminal::grid::Dimensions;
use paxri_ui::terminal::mouse::{Cell, WheelTarget, wheel_input};

pub(super) fn init_fonts(cx: &mut App) {
    use theme::fonts::{Face, MonospaceFonts};
    let text = cx.text_system();
    // GPUI's Android bootstrap only registers a subset of the system fonts.
    // Register additional installed monospace families before resolving the chooser.
    let fonts = std::fs::read_dir("/system/fonts")
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.to_lowercase().contains("mono"))
        })
        .filter_map(|path| std::fs::read(path).ok().map(std::borrow::Cow::Owned))
        .collect();
    if let Err(error) = text.add_fonts(fonts) {
        log::warn!("Registering terminal fonts: {error}");
    }
    let mut names = text.all_font_names();
    names.sort_by_cached_key(|name| name.to_lowercase());
    names.dedup();
    let installed: Vec<_> = names
        .into_iter()
        .filter(|name| name.to_lowercase().contains("mono"))
        .map(|name| Face::measure(name.into(), text))
        .collect();
    let default = installed
        .iter()
        .find(|face| face.font.family.as_ref() == "Droid Sans Mono")
        .or_else(|| installed.first())
        .cloned()
        .unwrap_or_else(|| Face::measure(".SystemUIFont".into(), text));
    cx.set_global(MonospaceFonts { installed, default });
}

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
    fn zoom_font(&mut self, size: f32, cx: &App) {
        let range = theme::TerminalAppearance::FONT_SIZES;
        let size = size.clamp(*range.start(), *range.end());
        let mut style = theme::current(cx).terminal;
        style.font_size = Some(size);
        self.font = cx.global::<theme::fonts::MonospaceFonts>().resolve(&style);
        if let Page::Terminal(id) = self.page
            && let Some(session) = self.sessions.get_mut(&id)
        {
            session.screen.font = self.font.clone();
            session.wheel = Default::default();
        }
    }

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
            Gesture::Tap if s.connected() && s.composing.text.is_empty() => {
                gpui_mobile::show_keyboard()
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
                if finished {
                    let size = (self.font.size * 10.).round() / 10.;
                    if size != theme::current(cx).terminal.size() || size != self.font.size {
                        self.change_gui(paxri_config::gui::GuiChange::FontSize(Some(size)), cx);
                    }
                } else if factor.is_finite() && factor > 0. {
                    self.zoom_font(self.font.size * factor, cx);
                }
            }
        }
    }
}
