//! Hand a drag from the tab strip to a desktop window while the button is still down.
use super::*;

#[cfg(windows)]
mod windows;
#[cfg(any(target_os = "linux", target_os = "freebsd"))]
mod x11;

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
            anchor.x.clamp(
                screen.left(),
                (screen.right() - dimensions.width).max(screen.left()),
            ),
            anchor.y.clamp(
                screen.top(),
                (screen.bottom() - dimensions.height).max(screen.top()),
            ),
        );
        Some(Self {
            display: display.id(),
            bounds: WindowBounds::Windowed(Bounds::new(origin, dimensions)),
            focus: true,
        })
    }
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

async fn present_docked_page(
    page: &Page,
    target: WindowHandle<Workspace>,
    cx: &mut AsyncWindowContext,
) -> Option<()> {
    let (frame, drawn) = async_channel::bounded(1);
    let _receiver = cx
        .update_window(target.into(), |_, w, _cx| {
            w.refresh();
            w.on_next_frame(move |_, _| {
                let _ = frame.try_send(());
            });
            #[cfg(windows)]
            {
                windows::native_handle(w)
            }
            #[cfg(not(windows))]
            {
                // AppKit and X11 suspend frames for fully covered windows. Build
                // the new scene before raising the receiver to request presentation.
                w.draw(_cx).clear();
                if !w.is_window_active() {
                    w.activate_window();
                }
            }
        })
        .ok()?;
    #[cfg(windows)]
    if let Some(hwnd) = _receiver {
        windows::redraw_now(hwnd);
    }
    // GPUI invokes the callback before drawing. This UI task resumes after
    // GPUI has submitted that frame and returned to the event loop.
    drawn.recv().await.ok()?;
    #[cfg(windows)]
    unsafe {
        windows_sys::Win32::Graphics::Dwm::DwmFlush()
    };
    target
        .update(cx, |v, _w, _| {
            if !v.tabs.contains(page) {
                return None;
            }
            #[cfg(windows)]
            if !_w.is_window_active() {
                _w.activate_window();
            }
            Some(())
        })
        .ok()?
}

pub(super) fn finish_transfer(
    view: &mut Workspace,
    page: &Page,
    target: WindowHandle<Workspace>,
    resume_drag: bool,
    w: &mut Window,
    cx: &mut Context<Workspace>,
) {
    view.drag_watch = None;
    let page = page.clone();
    #[cfg(windows)]
    let outgoing = windows::native_handle(w);
    cx.spawn_in(w, async move |source, cx| {
        if present_docked_page(&page, target, cx).await.is_none() {
            let _ = source.update_in(cx, |v, _, cx| v.settle_tab(cx));
            return;
        }
        let _closing = source
            .update_in(cx, |v, w, cx| {
                if !v.tabs.contains(&page) {
                    return false;
                }
                let last = v.tabs.len() == 1;
                #[cfg(windows)]
                if last && let Some(hwnd) = outgoing {
                    windows::hide_for_transfer(hwnd);
                }
                v.take_tab(&page, w, cx);
                last
            })
            .unwrap_or(true);
        if resume_drag {
            let _ = target.update(cx, |_v, w, cx| {
                #[cfg(windows)]
                windows::resume_after_close(outgoing.filter(|_| _closing), page, w, cx);
                #[cfg(not(windows))]
                _v.resume_docked_tab(page, w, cx);
            });
        }
    })
    .detach();
}

impl Workspace {
    pub(super) fn begin_tab_drag(
        &mut self,
        drag: &DraggedTab,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(mut tab) = self.tab_press.take().filter(|tab| tab.page == drag.page) else {
            return;
        };
        let cursor = w.mouse_position();
        tab.cursor = cursor;
        self.dragged_tab = Some(tab);
        cx.notify();
        // GPUI installs its drag after this callback. Process the first movement
        // even when the next input is already mouse-up.
        let source = cx.weak_entity();
        let drag = drag.clone();
        w.defer(cx, move |w, cx| {
            let _ = source.update(cx, |v, cx| v.update_tab_drag(drag, cursor, w, cx));
        });
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
        if !single && !tab.should_detach(self.dock.read(cx)) {
            self.dragged_tab = Some(tab);
            cx.notify();
            return true;
        }
        self.dragged_tab = None;
        let page = tab.page.clone();
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
            let offset = tab.window_grab(self.dock.read(cx)).unwrap_or_else(|| {
                self.dock
                    .read(cx)
                    .settings()
                    .window_grab_offset(tab.offset, w.viewport_size().width)
            });
            let placement = DropPlacement::at(cursor, offset, w, cx);
            if !single {
                let indent = self.dock.read(cx).indent(&page);
                #[cfg(windows)]
                windows::tear_off(tab, placement, offset, Some(indent), w, cx);
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
                    x11::start(cx.weak_entity(), window, tab, offset);
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
            self.dock.read(cx).settings(),
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
                self.tab_indent = self.dock.read(cx).indent(&page);
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
                crate::components::notify_active(format!("Open window: {e}"), cx);
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
        if self.tabs.len() > 1 && tab.should_detach(self.dock.read(cx)) {
            // The window system can coalesce a final move into mouse-up.
            let offset = tab.window_grab(self.dock.read(cx)).unwrap_or_else(|| {
                self.dock
                    .read(cx)
                    .settings()
                    .window_grab_offset(tab.offset, w.viewport_size().width)
            });
            let placement = DropPlacement::at(tab.cursor, offset, w, cx);
            #[cfg(windows)]
            windows::tear_off(tab, placement, offset, None, w, cx);
            #[cfg(any(target_os = "linux", target_os = "freebsd"))]
            if cx.compositor_name() != "Wayland" {
                x11::tear_off(self, tab, placement, None, px(0.), w, cx);
                return;
            }
            #[cfg(not(windows))]
            self.detach(tab.page, placement, px(0.), w, cx);
        } else {
            tab.reorder(&mut self.tabs, self.dock.read(cx), |page| page);
            self.select(tab.page, cx);
            self.refresh_title(w);
            self.focus_content(w, cx);
        }
    }
}

#[cfg(test)]
mod tests {

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
}
