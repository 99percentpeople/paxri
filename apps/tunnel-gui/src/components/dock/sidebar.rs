//! Sidebar preferences are copied to detached windows. Pointer gestures belong to one window.
use super::*;
use crate::components::{Button, ButtonVariant, Icon};

const RAIL_WIDTH: Pixels = px(44.);
const MIN_WIDTH: Pixels = px(112.);
const MAX_WIDTH: Pixels = px(480.);
const DIVIDER_WIDTH: Pixels = px(6.);

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DockSettings {
    pub(super) placement: DockPlacement,
    expanded_width: Pixels,
    collapsed: bool,
}

impl Default for DockSettings {
    fn default() -> Self {
        Self {
            placement: DockPlacement::Top,
            expanded_width: px(142.),
            collapsed: false,
        }
    }
}

impl DockSettings {
    pub(super) fn with_placement(mut self, placement: DockPlacement) -> Self {
        self.placement = placement;
        self
    }

    pub(super) fn compact(self) -> bool {
        self.placement == DockPlacement::Left && self.collapsed
    }

    pub fn window_grab_offset(self, grab: Point<Pixels>, viewport_width: Pixels) -> Point<Pixels> {
        let (origin, max_grab) = match self.placement {
            DockPlacement::Top => (point(SPACE_1, SPACE_1), point(TAB_MAX_WIDTH, TAB_HEIGHT)),
            DockPlacement::Left => {
                let width = self.width(viewport_width);
                let top = px(chrome::TITLEBAR_HEIGHT) + SPACE_1;
                if self.collapsed {
                    (
                        point((width - BUTTON_HEIGHT) / 2., top),
                        point(BUTTON_HEIGHT, BUTTON_HEIGHT),
                    )
                } else {
                    (
                        point(SPACE_1, top),
                        point((width - SPACE_1 * 2.).max(px(0.)), TAB_HEIGHT),
                    )
                }
            }
        };
        origin + grab.clamp(&Point::default(), &max_grab)
    }

    fn max_width(viewport: Pixels) -> Pixels {
        (viewport / 2.).clamp(px(0.), MAX_WIDTH)
    }

    pub(super) fn width(self, viewport: Pixels) -> Pixels {
        let desired = if self.collapsed {
            RAIL_WIDTH
        } else {
            self.expanded_width
        };
        desired.min(Self::max_width(viewport))
    }

    fn resized(mut self, requested: Pixels, viewport: Pixels) -> Self {
        let max = Self::max_width(viewport);
        let min = MIN_WIDTH.min(max);
        self.collapsed = requested < (RAIL_WIDTH.min(max) + min) / 2.;
        if !self.collapsed {
            self.expanded_width = requested.clamp(min, max);
        }
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
            self.initial
                .resized(self.start_width + (x - self.start_x), viewport)
        }
    }
}

pub(super) fn toggle<K: 'static>(state: Entity<DockState<K>>, ui: Ui, collapsed: bool) -> Button {
    let (icon, label) = if collapsed {
        (Icon::ExpandSidebar, "Expand sidebar")
    } else {
        (Icon::CollapseSidebar, "Collapse sidebar")
    };
    ui.icon_button(ButtonVariant::Ghost, "toggle-sidebar", icon, label)
        .on_click(move |_, _, cx| {
            state.update(cx, |dock, cx| {
                dock.set_sidebar_collapsed(!dock.settings.collapsed, cx);
            });
        })
}

impl<K: 'static> DockState<K> {
    pub fn set_sidebar_collapsed(&mut self, collapsed: bool, cx: &mut Context<Self>) {
        if self.settings.placement == DockPlacement::Left && self.settings.collapsed != collapsed {
            self.settings.collapsed = collapsed;
            cx.notify();
        }
    }
}

pub(super) fn sidebar<K: Clone + PartialEq + 'static>(
    tabs: DockTabs<K>,
    ui: Ui,
    window: &Window,
    cx: &App,
) -> Div {
    let state = tabs.state.clone();
    let width = state.read(cx).settings.width(window.viewport_size().width);
    let press = state.clone();
    let grip = div()
        .id("sidebar-divider")
        .absolute()
        .right_0()
        .top_0()
        .bottom_0()
        .w(DIVIDER_WIDTH)
        .cursor(CursorStyle::ResizeLeftRight)
        .hover(|grip| grip.bg(rgb(ui.colors.border)))
        .on_mouse_down(MouseButton::Left, move |e, window, cx| {
            press.update(cx, |dock, cx| {
                dock.resizing = Some(SidebarResize {
                    start_x: e.position.x,
                    start_width: dock.settings.width(window.viewport_size().width),
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
                            point(bounds.right() - px(1.), bounds.top()),
                            size(px(1.), bounds.size.height),
                        ),
                        rgb(ui.colors.border),
                    ));
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
        );
    div()
        .relative()
        .w(width)
        .h_full()
        .flex_none()
        .child(tabs)
        .child(grip)
}

#[cfg(test)]
mod tests {
    use super::{DockPlacement, DockSettings, SidebarResize};
    use gpui::{point, px};

    #[test]
    fn collapsed_tab_grab_matches_the_centered_icon_button() {
        let settings = DockSettings {
            placement: DockPlacement::Left,
            collapsed: true,
            ..DockSettings::default()
        };
        let grab = |x, y| settings.window_grab_offset(point(px(x), px(y)), px(900.));
        assert_eq!(grab(0., 0.), point(px(6.), px(44.)));
        assert_eq!(grab(16., 16.), point(px(22.), px(60.)));
        assert_eq!(grab(48., 48.), point(px(38.), px(76.)));
    }

    #[test]
    fn collapse_and_reopen_remember_the_expanded_width() {
        let viewport = px(900.);
        let expanded = DockSettings {
            placement: DockPlacement::Left,
            ..DockSettings::default()
        }
        .resized(px(280.), viewport);
        let mut collapsed = expanded.resized(px(48.), viewport);
        assert!(collapsed.compact());
        assert_eq!(collapsed.width(viewport), px(44.));
        collapsed.placement = DockPlacement::Top;
        assert!(!collapsed.compact());
        collapsed.collapsed = false;
        assert_eq!(collapsed.width(viewport), px(280.));
    }

    #[test]
    fn resizing_from_the_rail_preserves_the_grab_offset_and_respects_window_width() {
        let initial = DockSettings {
            placement: DockPlacement::Left,
            collapsed: true,
            ..DockSettings::default()
        };
        let drag = SidebarResize {
            start_x: px(41.),
            start_width: px(44.),
            initial,
        };
        let viewport = px(720.);
        assert_eq!(drag.at(px(41.), viewport), initial);
        assert_eq!(drag.at(px(120.), viewport).width(viewport), px(123.));
        assert_eq!(drag.at(px(92.), viewport).width(viewport), px(112.));
        assert_eq!(drag.at(px(900.), viewport).width(viewport), px(360.));
        let wide = drag.at(px(600.), px(1600.));
        assert_eq!(wide.width(px(1600.)), px(480.));
        assert_eq!(wide.width(px(300.)), px(150.));
        assert_eq!(wide.width(px(1600.)), px(480.));
    }
}
