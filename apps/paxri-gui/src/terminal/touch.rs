use super::*;
use gpui::ContactPhase::*;
use paxri_ui::terminal::mouse::{TouchMouseDrag, report};
use std::{collections::BTreeMap, time::Duration};

const SLOP: f64 = 6.;

#[derive(Default)]
pub(super) struct Touch {
    points: BTreeMap<u32, Point<Pixels>>,
    gesture: Gesture,
    hold: Option<Task<()>>,
    mouse: TouchMouseDrag,
    pub handles: bool,
}

#[derive(Clone, Copy, Default)]
enum Gesture {
    #[default]
    Idle,
    Pending(Point<Pixels>),
    Word(Point<Pixels>),
    Select,
    Handle {
        leading: bool,
        offset: Point<Pixels>,
    },
    Scroll,
    Report,
    Multi,
}

struct Grip {
    leading: bool,
    anchor: Point<Pixels>,
    center: Point<Pixels>,
}

impl Terminal {
    fn grips(&self) -> impl Iterator<Item = Grip> {
        let line = px(self.emulator.font.cell.height);
        self.emulator
            .selection_handles()
            .into_iter()
            .flat_map(move |ends| {
                ends.into_iter()
                    .enumerate()
                    .filter_map(move |(index, offset)| {
                        let anchor = self.viewport.origin + offset.map(px);
                        if anchor.y <= self.viewport.top()
                            || anchor.y - line >= self.viewport.bottom()
                        {
                            return None;
                        }
                        let inset = px(9.).min(self.viewport.size.width / 2.);
                        let shift = if cfg!(windows) { px(2.) } else { inset };
                        let x = (anchor.x + if index == 0 { -shift } else { shift })
                            .clamp(self.viewport.left() + inset, self.viewport.right() - inset);
                        let y = if anchor.y + px(18.) <= self.viewport.bottom() {
                            anchor.y + px(9.)
                        } else {
                            (anchor.y - line - px(9.)).max(self.viewport.top() + px(9.))
                        };
                        Some(Grip {
                            leading: index == 0,
                            anchor,
                            center: point(x, y),
                        })
                    })
            })
    }

    fn grip_at(&self, at: Point<Pixels>) -> Option<Grip> {
        if !self.touch.handles {
            return None;
        }
        let distance = |g: &Grip| (g.center - at).magnitude();
        self.grips()
            .filter(|g| distance(g) <= 21.)
            .min_by(|a, b| distance(a).total_cmp(&distance(b)))
    }

    pub(super) fn paint_grips(&self, window: &mut Window) {
        if !self.touch.handles {
            return;
        }
        #[cfg(windows)]
        let (face, border) = {
            use windows_sys::Win32::Graphics::Gdi::{
                COLOR_HIGHLIGHT, COLOR_HIGHLIGHTTEXT, GetSysColor,
            };
            let color = |index| rgb(unsafe { GetSysColor(index) }.swap_bytes() >> 8);
            (color(COLOR_HIGHLIGHT), color(COLOR_HIGHLIGHTTEXT))
        };
        #[cfg(not(windows))]
        let (face, border) = (
            rgb(self.emulator.palette.surface.foreground),
            rgb(self.emulator.palette.surface.background),
        );
        for grip in self.grips() {
            let bounds = Bounds::centered_at(grip.center, size(px(18.), px(18.)));
            let above = grip.center.y < grip.anchor.y;
            let tip = if above {
                grip.anchor.y - px(self.emulator.font.cell.height)
            } else {
                grip.anchor.y
            };
            let tip_on_left = grip.anchor.x < grip.center.x;
            let mut corners: Corners<Pixels> = px(9.).into();
            if !cfg!(windows) {
                match (above, tip_on_left) {
                    (false, true) => corners.top_left = px(0.),
                    (false, false) => corners.top_right = px(0.),
                    (true, true) => corners.bottom_left = px(0.),
                    (true, false) => corners.bottom_right = px(0.),
                }
            }

            window.paint_quad(quad(
                bounds,
                corners,
                face,
                px(1.),
                border,
                BorderStyle::Solid,
            ));
            // Connect the tip when a grip was shifted to stay inside the viewport.
            let x = grip.anchor.x.clamp(bounds.left(), bounds.right());
            let y = if cfg!(windows) {
                grip.center.y
            } else if above {
                bounds.bottom()
            } else {
                bounds.top()
            };
            window.paint_quad(fill(
                Bounds::from_corners(
                    point(x.min(grip.anchor.x), tip.min(y)),
                    point(x.max(grip.anchor.x) + px(1.), tip.max(y) + px(1.)),
                ),
                face,
            ));
        }
    }

    pub(super) fn cancel_touch(&mut self) {
        let release = self.touch.mouse.end(*self.emulator.term.mode());
        if !release.is_empty() {
            self.send(release);
        }
        self.touch.hold = None;
        self.touch.points.clear();
        self.touch.gesture = Gesture::Idle;
    }

    pub(super) fn touch_event(
        &mut self,
        event: &DirectTouchEvent,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let at = event.position;
        match event.phase {
            Down => {
                self.focus.focus(w, cx);
                self.touch.points.insert(event.id, at);
                self.touch.hold = None;
                if self.touch.points.len() > 1 {
                    let release = self.touch.mouse.end(*self.emulator.term.mode());
                    if !release.is_empty() {
                        self.send(release);
                    }
                    self.touch.gesture = Gesture::Multi;
                } else if let Some(grip) = self.grip_at(at) {
                    self.touch.gesture = Gesture::Handle {
                        leading: grip.leading,
                        offset: grip.anchor
                            - point(px(0.), px(self.emulator.font.cell.height / 2.))
                            - at,
                    };
                } else {
                    self.touch.gesture = Gesture::Pending(at);
                    self.touch.hold = Some(cx.spawn(async move |this, cx| {
                        cx.background_executor()
                            .timer(Duration::from_millis(500))
                            .await;
                        let _ = this.update(cx, |v, cx| {
                            if matches!(v.touch.gesture, Gesture::Pending(_)) {
                                let (row, col) = v.cell_at(at);
                                v.emulator.start_touch_selection(row, col);
                                v.touch.handles = true;
                                v.touch.gesture = Gesture::Word(at);
                                cx.notify();
                            }
                        });
                    }));
                }
            }
            Move | Up => {
                let Some(p) = self.touch.points.get_mut(&event.id) else {
                    return;
                };
                let previous = std::mem::replace(p, at);
                self.move_touch(at, previous, w, cx);
                if event.phase == Up {
                    if matches!(self.touch.gesture, Gesture::Pending(_)) {
                        if self.touch.handles {
                            self.emulator.term.selection = None;
                            self.touch.handles = false;
                        } else if self.mouse_enabled() {
                            let mode = *self.emulator.term.mode();
                            let cell = self.mouse_cell(at);
                            let mut bytes = report(mode, cell, 0, false, Modifiers::default());
                            bytes.extend(report(mode, cell, 0, true, Modifiers::default()));
                            self.send(bytes);
                        } else {
                            self.emulator.term.selection = None;
                        }
                    }
                    self.touch.points.remove(&event.id);
                    if self.touch.points.is_empty() {
                        self.cancel_touch();
                    }
                }
            }
            Cancel => self.cancel_touch(),
        }
        cx.stop_propagation();
        cx.notify();
    }

    fn move_touch(
        &mut self,
        at: Point<Pixels>,
        previous: Point<Pixels>,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if at == previous {
            return;
        }
        let mut from = previous;
        let mode = *self.emulator.term.mode();
        match self.touch.gesture {
            Gesture::Pending(origin) => {
                if (at - origin).magnitude() < SLOP {
                    return;
                }
                self.touch.hold = None;
                self.touch.handles = false;
                self.emulator.term.selection = None;
                from = origin;
                self.touch.gesture = Gesture::Scroll;
            }
            Gesture::Word(origin) => {
                if (at - origin).magnitude() < SLOP {
                    return;
                }
                self.touch.gesture = Gesture::Select;
                if self.mouse_enabled() && TouchMouseDrag::requested(mode) {
                    let cell = self.mouse_cell(origin);
                    let bytes = self.touch.mouse.begin(mode, cell);
                    if !bytes.is_empty() {
                        self.touch.handles = false;
                        self.emulator.term.selection = None;
                        self.send(bytes);
                        self.touch.gesture = Gesture::Report;
                    }
                }
            }
            _ => {}
        }
        match self.touch.gesture {
            Gesture::Select | Gesture::Handle { .. } => {
                self.emulator.scroll_for_edge(
                    f32::from(at.y),
                    f32::from(self.viewport.top()),
                    f32::from(self.viewport.bottom()),
                );
                if let Gesture::Handle { leading, offset } = self.touch.gesture {
                    let (row, x) = self.cell_at(at + offset);
                    self.emulator.move_selection_handle(leading, row, x);
                } else {
                    let (row, x) = self.cell_at(at);
                    self.emulator.extend_touch_selection(row, x);
                }
            }
            Gesture::Scroll => self.wheel(
                &ScrollWheelEvent {
                    position: at,
                    delta: ScrollDelta::Pixels(at - from),
                    ..Default::default()
                },
                w,
                cx,
            ),
            Gesture::Report => {
                let cell = self.mouse_cell(at);
                let bytes = self.touch.mouse.motion(mode, cell);
                if !bytes.is_empty() {
                    self.send(bytes);
                }
            }
            Gesture::Multi if self.touch.points.len() == 2 => {
                if let Some(other) = self.touch.points.values().find(|p| **p != at) {
                    let before = (previous - *other).magnitude() as f32;
                    let after = (at - *other).magnitude() as f32;
                    if before >= 1. && after >= 1. {
                        TerminalZoom::apply(Zoom::Pinch(after / before), w, cx);
                    }
                }
            }
            _ => {}
        }
    }

    pub(super) fn listen_touch(entity: &Entity<Self>, hitbox: Hitbox, w: &mut Window) {
        let view = entity.downgrade();
        w.on_mouse_event(move |e: &DirectTouchEvent, phase, w, cx| {
            let _ = view.update(cx, |v, cx| {
                let captured = v.touch.points.contains_key(&e.id)
                    || (e.phase == Down && !v.touch.points.is_empty());
                if (captured && phase == DispatchPhase::Capture)
                    || (!captured
                        && e.phase == Down
                        && phase == DispatchPhase::Bubble
                        && hitbox.is_hovered(w))
                {
                    v.touch_event(e, w, cx);
                }
            });
        });
    }
}
