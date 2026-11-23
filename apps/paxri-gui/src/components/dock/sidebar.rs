//! Sidebar preferences are copied to detached windows. Pointer gestures belong to one window.
use super::*;

const MIN_WIDTH: Pixels = px(176.);
const MAX_WIDTH: Pixels = px(480.);
const DIVIDER_WIDTH: Pixels = px(6.);

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DockSettings {
    pub(super) placement: DockPlacement,
    sidebar_width: Pixels,
}

impl Default for DockSettings {
    fn default() -> Self {
        Self {
            placement: DockPlacement::Top,
            sidebar_width: DEFAULT_SIDEBAR_WIDTH,
        }
    }
}

impl DockSettings {
    pub(crate) fn with_placement(mut self, placement: DockPlacement) -> Self {
        self.placement = placement;
        self
    }

    pub(super) fn tab_height(self) -> Pixels {
        match self.placement {
            DockPlacement::Top => TOP_TAB_HEIGHT,
            DockPlacement::Left => BUTTON_HEIGHT,
        }
    }

    pub fn window_grab_offset(self, grab: Point<Pixels>, viewport_width: Pixels) -> Point<Pixels> {
        let (origin, tab_width) = match self.placement {
            DockPlacement::Top => (point(SPACE_1, SPACE_1), TAB_MAX_WIDTH),
            DockPlacement::Left => {
                let width = self.width(viewport_width);
                let top = px(chrome::TITLEBAR_HEIGHT) + SPACE_1;
                (point(SPACE_1, top), (width - SPACE_1 * 2.).max(px(0.)))
            }
        };
        origin + grab.clamp(&Point::default(), &point(tab_width, self.tab_height()))
    }

    pub(crate) fn page_width(self, viewport: Pixels) -> Pixels {
        viewport - self.width(viewport)
    }

    fn max_width(viewport: Pixels) -> Pixels {
        (viewport / 2.).clamp(px(0.), MAX_WIDTH)
    }

    pub(super) fn width(self, viewport: Pixels) -> Pixels {
        self.sidebar_width.min(Self::max_width(viewport))
    }

    fn resized(mut self, requested: Pixels, viewport: Pixels) -> Self {
        let max = Self::max_width(viewport);
        self.sidebar_width = requested.clamp(MIN_WIDTH.min(max), max);
        self
    }
}

#[derive(Clone, Copy)]
pub(super) struct SidebarResize {
    start_x: Pixels,
    start_width: Pixels,
    initial: DockSettings,
}

impl SidebarResize {
    fn at(self, x: Pixels, viewport: Pixels) -> DockSettings {
        if x == self.start_x {
            self.initial
        } else {
            let requested = self.start_width + (x - self.start_x);
            self.initial.resized(requested, viewport)
        }
    }
}

pub(super) fn divider<K: 'static>(
    state: Entity<DockState<K>>,
    sidebar_width: Pixels,
    ui: Ui,
) -> impl IntoElement {
    let press = state.clone();
    div()
        .id("sidebar-divider")
        .absolute()
        .left(sidebar_width - DIVIDER_WIDTH / 2.)
        .top_0()
        .bottom_0()
        .w(DIVIDER_WIDTH)
        .block_mouse_except_scroll()
        .cursor(CursorStyle::ResizeLeftRight)
        .hover(|grip| grip.bg(rgb(ui.colors.border)))
        .on_mouse_down(MouseButton::Left, move |e, window, cx| {
            press.update(cx, |dock, cx| {
                dock.resizing = Some(SidebarResize {
                    start_x: e.position.x,
                    start_width: sidebar_width,
                    initial: dock.settings,
                });
                cx.notify();
            });
            window.prevent_default();
            cx.stop_propagation();
        })
        .child(
            canvas(
                |_, _, _| (),
                move |bounds, _, window, _| {
                    window.paint_quad(fill(
                        Bounds::new(
                            point(bounds.center().x - px(0.5), bounds.top()),
                            size(px(1.), bounds.size.height),
                        ),
                        rgb(ui.colors.border),
                    ));
                },
            )
            .size_full(),
        )
}

pub(super) fn capture_resize<K: 'static>(state: Entity<DockState<K>>, cx: &App) -> Div {
    div()
        .absolute()
        .inset_0()
        .when(state.read(cx).resizing.is_some(), |surface| {
            surface.occlude().cursor(CursorStyle::ResizeLeftRight)
        })
        .child(
            canvas(
                |_, _, _| (),
                move |_, _, window, _| {
                    // Capture the remainder of a press even when the pointer crosses the panel or window edge.
                    let moving = state.clone();
                    window.on_mouse_event(move |e: &MouseMoveEvent, phase, w, cx| {
                        if phase.capture()
                            && let Some(drag) = moving.read(cx).resizing
                        {
                            moving.update(cx, |dock, cx| {
                                if e.pressed_button == Some(MouseButton::Left) {
                                    let next = drag.at(e.position.x, w.viewport_size().width);
                                    if dock.settings != next {
                                        dock.settings = next;
                                        cx.notify();
                                    }
                                } else {
                                    dock.resizing = None;
                                    cx.notify();
                                }
                            });
                            cx.stop_propagation();
                        }
                    });
                    let released = state.clone();
                    window.on_mouse_event(move |e: &MouseUpEvent, phase, w, cx| {
                        if phase.capture()
                            && e.button == MouseButton::Left
                            && let Some(drag) = released.read(cx).resizing
                        {
                            released.update(cx, |dock, cx| {
                                dock.settings = drag.at(e.position.x, w.viewport_size().width);
                                dock.resizing = None;
                                cx.notify();
                            });
                            cx.stop_propagation();
                        }
                    });
                },
            )
            .size_full(),
        )
}

#[cfg(test)]
mod tests {
    use super::{DockPlacement, DockSettings, MIN_WIDTH, SidebarResize};
    use gpui::{point, px};

    #[test]
    fn resizing_stops_at_the_minimum_and_preserves_the_grab_offset() {
        let initial = DockSettings::default();
        let drag = SidebarResize {
            start_x: px(218.),
            start_width: px(220.),
            initial,
        };
        let viewport = px(900.);
        assert_eq!(drag.at(px(218.), viewport), initial);
        assert_eq!(drag.at(px(296.), viewport).width(viewport), px(298.));
        assert_eq!(drag.at(px(0.), viewport).width(viewport), MIN_WIDTH);
        assert_eq!(drag.at(px(550.), viewport).width(viewport), px(450.));
        let wide = drag.at(px(600.), px(1600.));
        assert_eq!(wide.width(px(1600.)), px(480.));
        assert_eq!(wide.width(px(640.)), px(320.));
        assert_eq!(wide.width(px(1600.)), px(480.));
    }

    #[test]
    fn both_tab_placements_leave_the_same_width_for_page_content() {
        let viewport = px(900.);
        let top = DockSettings::default().resized(px(260.), viewport);
        let left = top.with_placement(DockPlacement::Left);
        assert_eq!(top.page_width(viewport), px(640.));
        assert_eq!(left.page_width(viewport), top.page_width(viewport));
        assert_eq!(
            left.window_grab_offset(point(px(16.), px(16.)), viewport),
            point(px(20.), px(60.))
        );
    }
}
