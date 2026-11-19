//! Hand a drag from the tab strip to a desktop window while the button is still down.
use super::*;

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub(super) use windows::prepare_detached_window;
#[cfg(any(target_os = "linux", target_os = "freebsd"))]
mod x11;

// Distances are in GPUI logical pixels and scale with the display.
const DETACH_DISTANCE: Pixels = px(24.);

#[derive(Clone)]
pub(super) struct TabDrag {
    pub page: Page,
    pub previous_active: Page,
    pub offset: Point<Pixels>,
    pub cursor: Point<Pixels>,
}

impl TabDrag {
    pub(super) fn projected(
        &self,
        mut bounds: Bounds<Pixels>,
        layout: &TabLayout,
        axis: Axis,
    ) -> Bounds<Pixels> {
        if let Some((_, first)) = layout.tabs.first() {
            let desired = (self.cursor - self.grab_in(bounds)).along(axis);
            let start = first.origin.along(axis).max(layout.zone.origin.along(axis));
            let end = layout.zone.bottom_right().along(axis) - bounds.size.along(axis);
            bounds.origin = bounds
                .origin
                .apply_along(axis, |_| desired.clamp(start, end.max(start)));
        }
        bounds
    }

    fn grab_in(&self, bounds: Bounds<Pixels>) -> Point<Pixels> {
        point(
            self.offset.x.clamp(px(0.), bounds.size.width),
            self.offset.y.clamp(px(0.), bounds.size.height),
        )
    }

    pub(super) fn order(&self, pages: &[Page], layout: &TabLayout, axis: Axis) -> Vec<Page> {
        let mut order: Vec<_> = pages
            .iter()
            .filter(|page| **page != self.page)
            .cloned()
            .collect();
        // A move or release may arrive before the receiver's first layout.
        let Some(slot) = layout.bounds(&self.page) else {
            if let Some(index) = layout.insertion(self.cursor, axis) {
                order.insert(index.min(order.len()), self.page.clone());
                return order;
            }
            return pages.to_vec();
        };
        let projected = self.projected(slot, layout, axis);
        // Cross a neighbour's midpoint with the leading edge of the moving tab,
        // including when the two tabs have different widths.
        let index = layout
            .tabs
            .iter()
            .filter(|(page, bounds)| {
                if *page == self.page {
                    return false;
                }
                let edge = if bounds.origin.along(axis) < slot.origin.along(axis) {
                    projected.origin
                } else {
                    projected.bottom_right()
                };
                // The pointer can enter trailing space while the grabbed tab
                // still overlaps the last neighbour.
                self.cursor.along(axis) >= bounds.bottom_right().along(axis)
                    || edge.along(axis) > bounds.center().along(axis)
            })
            .count();
        order.insert(index.min(order.len()), self.page.clone());
        order
    }
}

#[derive(Clone, Copy)]
pub(super) struct DropPlacement {
    pub display: DisplayId,
    pub bounds: WindowBounds,
    pub focus: bool,
}

impl DropPlacement {
    fn at(release: Point<Pixels>, offset: Point<Pixels>, w: &Window, cx: &App) -> Option<Self> {
        let source_display = w.display(cx)?;
        let screen_point = w.bounds().origin + release;
        let display = if source_display.bounds().contains(&screen_point) {
            source_display
        } else {
            cx.displays()
                .into_iter()
                .find(|d| d.bounds().contains(&screen_point))
                .unwrap_or(source_display)
        };
        let screen = display.bounds();
        let dimensions = w.viewport_size();
        let anchor = screen_point - offset;
        let origin = point(
            anchor
                .x
                .clamp(screen.left(), screen.right() - dimensions.width),
            anchor
                .y
                .clamp(screen.top(), screen.bottom() - dimensions.height),
        );
        Some(Self {
            display: display.id(),
            bounds: WindowBounds::Windowed(Bounds::new(origin, dimensions)),
            focus: true,
        })
    }
}

fn window_grab_offset(grab: Point<Pixels>, sidebar: bool) -> Point<Pixels> {
    point(
        grab.x.clamp(px(0.), px(96.)) + px(4.),
        grab.y.clamp(px(0.), px(31.)) + px(4. + if sidebar { chrome::TITLEBAR_HEIGHT } else { 0. }),
    )
}

fn leaves_strip(
    cursor: Point<Pixels>,
    strip: Bounds<Pixels>,
    viewport: Size<Pixels>,
    sidebar: bool,
) -> bool {
    // Crossing the titlebar or the empty part of a sidebar along its length
    // keeps the drag in the same window.
    let area = if sidebar {
        Bounds::new(
            point(strip.left(), px(0.)),
            size(strip.size.width, viewport.height),
        )
    } else {
        Bounds::new(
            point(px(0.), strip.top()),
            size(viewport.width, strip.size.height),
        )
    };
    !area.dilate(DETACH_DISTANCE).contains(&cursor)
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum DragEvent {
    Moved,
    Released,
    Cancelled,
}

#[derive(Clone)]
pub(crate) struct DragSignal(async_channel::Sender<DragEvent>);

impl DragSignal {
    pub(crate) fn send(&self, event: DragEvent) {
        if event == DragEvent::Moved {
            let _ = self.0.try_send(event);
        } else {
            // A release or capture loss supersedes queued motion. Later events
            // from this gesture cannot replace its final notification.
            let _ = self.0.force_send(event);
            self.0.close();
        }
    }
}

pub(super) struct DragWatch {
    signal: DragSignal,
    _task: Task<()>,
}

impl Workspace {
    fn detached_tab_indent(&self, page: &Page) -> Pixels {
        let Some((_, first)) = self.tab_layout.tabs.first() else {
            return px(0.);
        };
        self.tab_layout.bounds(page).map_or(px(0.), |slot| {
            (slot.origin - first.origin)
                .along(self.tab_axis())
                .max(px(0.))
        })
    }

    fn watch_drag<G: 'static>(
        &mut self,
        w: &mut Window,
        cx: &mut Context<Self>,
        subscribe: impl FnOnce(DragSignal, &mut Window, &mut Context<Self>) -> Option<G>,
        mut handle: impl FnMut(&G, DragEvent, &mut Self, &mut Window, &mut Context<Self>) -> bool
        + 'static,
    ) -> bool {
        let (tx, rx) = async_channel::bounded(1);
        let signal = DragSignal(tx);
        let Some(guard) = subscribe(signal.clone(), w, cx) else {
            self.settle_tab(cx);
            return false;
        };
        signal.send(DragEvent::Moved);
        let guard_signal = signal.0.clone();
        let task = cx.spawn_in(w, async move |this, cx| {
            while let Ok(event) = rx.recv().await {
                if !matches!(
                    this.update_in(cx, |v, w, cx| {
                        if handle(&guard, event, v, w, cx) && event == DragEvent::Moved {
                            return true;
                        }
                        if v.drag_watch
                            .as_ref()
                            .is_some_and(|watch| watch.signal.0.same_channel(&guard_signal))
                        {
                            v.drag_watch = None;
                            v.settle_tab(cx);
                        }
                        false
                    }),
                    Ok(true)
                ) {
                    break;
                }
            }
        });
        self.drag_watch = Some(DragWatch {
            signal,
            _task: task,
        });
        true
    }

    pub(super) fn continue_tab_drag(&mut self, page: Page, w: &mut Window, cx: &mut Context<Self>) {
        let source = w
            .window_handle()
            .downcast::<Workspace>()
            .expect("workspace root");
        #[cfg(any(target_os = "linux", target_os = "freebsd"))]
        let receiver = self.window_title();
        let watching = self.watch_drag(
            w,
            cx,
            move |signal, w, _cx| {
                #[cfg(windows)]
                {
                    windows::Events::subscribe(w, signal, None)
                }
                #[cfg(target_os = "macos")]
                {
                    chrome::mac_drag_events(w, _cx, signal)
                }
                #[cfg(any(target_os = "linux", target_os = "freebsd"))]
                {
                    let _ = w;
                    x11::Events::subscribe(&receiver, signal, _cx)
                }
            },
            move |events, event, v, w, cx| {
                if event == DragEvent::Cancelled {
                    v.clear_strip_drag(w, cx);
                    return false;
                }
                let Some((pressed, cursor)) = events.pointer(w) else {
                    v.clear_strip_drag(w, cx);
                    return false;
                };
                if event == DragEvent::Released || !pressed {
                    v.finish_tab_drag(cursor, w, cx);
                    return false;
                }
                let Some(tab) = &v.dragged_tab else {
                    return false;
                };
                if tab.cursor == cursor {
                    return true;
                }
                v.update_tab_drag(
                    DraggedTab {
                        page: page.clone(),
                        source,
                    },
                    cursor,
                    w,
                    cx,
                );
                v.dragged_tab.is_some()
            },
        );
        if !watching {
            self.clear_strip_drag(w, cx);
        }
    }

    pub(super) fn cancel_tab_drag(&mut self, w: &mut Window, cx: &mut Context<Self>) -> bool {
        if let Some(watch) = &self.drag_watch {
            watch.signal.send(DragEvent::Cancelled);
            return true;
        }
        self.clear_strip_drag(w, cx)
    }

    fn clear_strip_drag(&mut self, w: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(tab) = self.dragged_tab.take() else {
            return false;
        };
        cx.stop_active_drag(w);
        self.select(tab.previous_active, cx);
        self.focus_content(w, cx);
        true
    }

    pub(super) fn move_tab_drag(
        &mut self,
        e: &DragMoveEvent<DraggedTab>,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if e.event.dragging() && self.update_tab_drag(e.drag(cx).clone(), e.event.position, w, cx) {
            cx.stop_propagation();
        }
    }

    pub(super) fn update_tab_drag(
        &mut self,
        drag: DraggedTab,
        cursor: Point<Pixels>,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if drag.source.window_id() != w.window_handle().window_id() {
            return false;
        }
        let single = self.tabs.len() == 1;
        let Some(mut tab) = self.dragged_tab.take() else {
            return false;
        };
        tab.cursor = cursor;
        if !single
            && !leaves_strip(
                cursor,
                self.tab_layout.zone,
                w.viewport_size(),
                self.sidebar,
            )
        {
            self.dragged_tab = Some(tab);
            cx.notify();
            return true;
        }
        self.dragged_tab = None;
        let page = tab.page.clone();
        if let Some(bounds) = self.tab_layout.bounds(&page) {
            tab.offset = tab.grab_in(bounds);
        }
        cx.stop_active_drag(w);

        // AppKit and Wayland associate the initiating mouse event / seat
        // serial with the pressed surface. Move that window and leave its
        // remaining pages in a second workspace at the source bounds.
        #[cfg(target_os = "macos")]
        {
            if self.keep_pressed_tab(page.clone(), w, cx) {
                self.watch_drag(
                    w,
                    cx,
                    |signal, w, cx| chrome::mac_drag_events(w, cx, signal),
                    move |_, event, v, w, cx| {
                        if event == DragEvent::Cancelled {
                            return false;
                        }
                        let Some((held, local)) = chrome::tab_dock_sample(w) else {
                            return false;
                        };
                        let held = held && event != DragEvent::Released;
                        if let Some(target) = v.dock_target_at(w, cx, |_, target| local(target))
                            && v.dock_at_pointer(target, held.then_some(tab.offset), w, cx)
                        {
                            return false;
                        }
                        held
                    },
                );
                chrome::start_tab_window_move(w);
            }
            return true;
        }
        #[cfg(any(target_os = "linux", target_os = "freebsd"))]
        if cx.compositor_name() == "Wayland" {
            if self.keep_pressed_tab(page.clone(), w, cx) && single {
                w.start_window_move();
            }
            return true;
        }

        #[cfg(not(target_os = "macos"))]
        {
            let source = drag.source;
            let offset = if let Some(bounds) = self.tab_layout.bounds(&page) {
                bounds.origin + tab.offset
            } else {
                window_grab_offset(tab.offset, self.sidebar)
            };
            let placement = DropPlacement::at(cursor, offset, w, cx);
            if !single {
                let indent = self.detached_tab_indent(&page);
                #[cfg(windows)]
                windows::tear_off(tab, placement, Some(offset), indent, w, cx);
                #[cfg(any(target_os = "linux", target_os = "freebsd"))]
                x11::tear_off(self, tab, placement, Some(offset), indent, w, cx);
                return true;
            }

            // Defer the native move until the initiating input handler returns.
            cx.defer(move |cx| {
                let _ = source.update(cx, |view, window, cx| {
                    #[cfg(windows)]
                    windows::follow(view, window, cx, offset, tab.offset, None);
                    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
                    x11::start(cx.weak_entity(), window, tab, None);
                    #[cfg(not(windows))]
                    let _ = (view, cx);
                });
            });
            true
        }
    }

    #[cfg(any(target_os = "macos", target_os = "linux", target_os = "freebsd"))]
    fn keep_pressed_tab(&mut self, page: Page, w: &mut Window, cx: &mut Context<Self>) -> bool {
        let remaining: Vec<_> = self
            .tabs
            .iter()
            .filter(|tab| **tab != page)
            .cloned()
            .collect();
        let Some(first) = remaining.first().cloned() else {
            return true;
        };
        let placement = w.display(cx).map(|d| DropPlacement {
            display: d.id(),
            bounds: w.window_bounds(),
            focus: false,
        });
        match Self::open(
            self.dashboard.clone(),
            first,
            self.sidebar,
            WorkspaceWindow::Detached {
                placement,
                tab_indent: px(0.),
            },
            cx,
        ) {
            Ok(other) => {
                #[cfg(any(target_os = "linux", target_os = "freebsd"))]
                let moving = w
                    .window_handle()
                    .downcast::<Workspace>()
                    .expect("workspace root");
                let _ = other.update(cx, |v, window, cx| {
                    v.tabs = remaining;
                    v.refresh_title(window);
                    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
                    // Let the sibling map before the compositor takes the
                    // interactive move; otherwise it can cover the moving tab.
                    window.on_next_frame(move |sibling, _| {
                        sibling.on_next_frame(move |_, cx| {
                            let _ = moving.update(cx, |_, pressed, _| {
                                pressed.activate_window();
                                pressed.start_window_move();
                            });
                        });
                    });
                    cx.notify();
                });
                self.tab_indent = self.detached_tab_indent(&page);
                #[cfg(any(target_os = "linux", target_os = "freebsd"))]
                {
                    self.wayland_move_left_surface = false;
                }
                self.tabs.retain(|p| *p == page);
                self.focus_tab(page, w, cx);
                self.refresh_title(w);
                true
            }
            Err(e) => {
                self.dashboard.update(cx, |d, cx| {
                    d.action_error = Some(format!("Open window: {e}"));
                    cx.notify();
                });
                false
            }
        }
    }

    pub(super) fn release_tab(&mut self, e: &MouseUpEvent, w: &mut Window, cx: &mut Context<Self>) {
        if e.button != MouseButton::Left {
            return;
        }
        self.tab_press = None;
        self.settle_tab(cx);
        if cx.has_active_drag() && self.dragged_tab.is_some() {
            self.finish_tab_drag(e.position, w, cx);
            cx.stop_propagation();
        }
    }

    fn finish_tab_drag(&mut self, cursor: Point<Pixels>, w: &mut Window, cx: &mut Context<Self>) {
        let Some(mut tab) = self.dragged_tab.take() else {
            return;
        };
        tab.cursor = cursor;
        cx.stop_active_drag(w);
        if self.tabs.len() > 1
            && leaves_strip(
                tab.cursor,
                self.tab_layout.zone,
                w.viewport_size(),
                self.sidebar,
            )
        {
            // The window system can coalesce a final move into mouse-up.
            let offset = self.tab_layout.bounds(&tab.page).map_or_else(
                || window_grab_offset(tab.offset, self.sidebar),
                |bounds| bounds.origin + tab.grab_in(bounds),
            );
            let placement = DropPlacement::at(tab.cursor, offset, w, cx);
            #[cfg(windows)]
            windows::tear_off(tab, placement, None, px(0.), w, cx);
            #[cfg(any(target_os = "linux", target_os = "freebsd"))]
            if cx.compositor_name() != "Wayland" {
                x11::tear_off(self, tab, placement, None, px(0.), w, cx);
                return;
            }
            #[cfg(not(windows))]
            self.detach(tab.page, placement, px(0.), w, cx);
        } else {
            self.tabs = tab.order(&self.tabs, &self.tab_layout, self.tab_axis());
            self.select(tab.page, cx);
            self.refresh_title(w);
            self.focus_content(w, cx);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{leaves_strip, window_grab_offset};
    use gpui::{Axis, Bounds, point, px, size};

    #[test]
    fn release_or_cancel_supersedes_queued_motion() {
        use super::{DragEvent, DragSignal};
        for end in [DragEvent::Released, DragEvent::Cancelled] {
            let (tx, rx) = async_channel::bounded(1);
            let signal = DragSignal(tx);
            signal.send(DragEvent::Moved);
            signal.send(DragEvent::Moved);
            assert!(rx.try_recv() == Ok(DragEvent::Moved));
            assert!(rx.try_recv().is_err());
            signal.send(DragEvent::Moved);
            signal.send(end);
            signal.send(DragEvent::Moved);
            signal.send(DragEvent::Cancelled);
            assert!(rx.try_recv() == Ok(end));
            assert!(rx.is_closed() && rx.is_empty());
        }
    }

    #[test]
    fn tearing_off_requires_clearance() {
        let viewport = size(px(900.), px(650.));
        let top = Bounds::new(point(px(0.), px(0.)), size(px(280.), px(40.)));
        assert!(!leaves_strip(
            point(px(850.), px(60.)),
            top,
            viewport,
            false
        ));
        assert!(!leaves_strip(
            point(px(-15.), px(17.)),
            top,
            viewport,
            false
        ));
        assert!(leaves_strip(point(px(95.), px(65.)), top, viewport, false));
        assert!(leaves_strip(point(px(-25.), px(17.)), top, viewport, false));
        let side = Bounds::new(point(px(0.), px(40.)), size(px(142.), px(610.)));
        assert!(!leaves_strip(
            point(px(164.), px(570.)),
            side,
            viewport,
            true
        ));
        assert!(leaves_strip(
            point(px(167.), px(210.)),
            side,
            viewport,
            true
        ));
    }

    #[test]
    fn dragging_into_blank_space_tracks_the_pointer_and_appends_on_release() {
        use super::{Page, TabDrag, TabLayout};
        let mut layout = TabLayout {
            zone: Bounds::new(point(px(0.), px(0.)), size(px(500.), px(40.))),
            tabs: vec![
                (
                    Page::Home,
                    Bounds::new(point(px(4.), px(4.)), size(px(96.), px(31.))),
                ),
                (
                    Page::Settings,
                    Bounds::new(point(px(104.), px(4.)), size(px(120.), px(31.))),
                ),
            ],
        };
        let mut drag = TabDrag {
            page: Page::Home,
            previous_active: Page::Home,
            offset: point(px(40.), px(12.)),
            cursor: point(px(355.), px(37.)),
        };
        let pages = [Page::Home, Page::Settings];
        let home = layout.bounds(&Page::Home).unwrap();
        let axis = Axis::Horizontal;
        let preview = drag.projected(home, &layout, axis);
        assert_eq!(preview.origin, point(px(315.), px(4.)));
        assert!(preview.left() > layout.bounds(&Page::Settings).unwrap().right());
        assert!(drag.order(&pages, &layout, axis) == [Page::Settings, Page::Home]);
        drag.cursor.x = px(650.);
        assert_eq!(
            drag.projected(home, &layout, axis).right(),
            layout.zone.right()
        );
        drag.cursor.x = px(-8.);
        assert_eq!(drag.projected(home, &layout, axis), home);
        assert!(drag.order(&pages, &layout, axis) == pages);

        // Home is the only other tab when Settings arrives at its right edge.
        drag.page = Page::Settings;
        drag.offset.x = px(110.);
        drag.cursor.x = home.right() + px(1.);
        assert!(drag.order(&pages, &layout, axis) == pages);

        // The button can be released at a new location before Settings has a slot.
        layout.tabs.pop();
        assert!(drag.order(&pages, &layout, axis) == pages);
        drag.cursor.x = home.left();
        assert!(drag.order(&pages, &layout, axis) == [Page::Settings, Page::Home]);
    }

    #[test]
    fn sidebar_preview_can_reach_the_bottom_and_drop_last() {
        use super::{Page, TabDrag, TabLayout};
        let slot = |y| Bounds::new(point(px(4.), px(y)), size(px(134.), px(31.)));
        let home = slot(44.);
        let layout = TabLayout {
            zone: Bounds::new(point(px(0.), px(40.)), size(px(142.), px(480.))),
            tabs: vec![(Page::Home, home), (Page::Settings, slot(79.))],
        };
        let pages = [Page::Home, Page::Settings];
        let axis = Axis::Vertical;
        let mut drag = TabDrag {
            page: Page::Home,
            previous_active: Page::Home,
            offset: point(px(40.), px(28.)),
            cursor: point(px(65.), px(390.)),
        };
        assert_eq!(drag.projected(home, &layout, axis), slot(362.));
        assert!(drag.order(&pages, &layout, axis) == [Page::Settings, Page::Home]);
        drag.cursor.y = layout.zone.bottom() - px(1.);
        assert_eq!(
            drag.projected(home, &layout, axis).bottom(),
            layout.zone.bottom()
        );
        drag.cursor.y = layout.zone.top();
        assert_eq!(drag.projected(home, &layout, axis), home);
        assert!(drag.order(&pages, &layout, axis) == pages);

        // An incoming tab grabbed near its bottom still docks after Home
        // when the pointer is just below Home's slot.
        drag.page = Page::Settings;
        drag.cursor.y = home.bottom() + px(1.);
        assert!(drag.order(&pages, &layout, axis) == pages);
    }

    #[test]
    fn floating_tab_keeps_the_pointer_over_its_label() {
        let grab = point(px(35.), px(15.));
        assert_eq!(window_grab_offset(grab, false), point(px(39.), px(19.)));
        assert_eq!(window_grab_offset(grab, true), point(px(39.), px(59.)));
    }
}
