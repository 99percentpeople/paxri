//! Client-drawn window frames shared by workspaces and device terminals.
use crate::components::Ui;
use gpui::{prelude::*, *};

pub(super) const TITLEBAR_HEIGHT: f32 = 40.;

pub fn window_options(title: &str, dimensions: Size<Pixels>, cx: &App) -> WindowOptions {
    WindowOptions {
        titlebar: Some(TitlebarOptions {
            title: Some(title.to_owned().into()),
            appears_transparent: true,
            ..Default::default()
        }),
        window_decorations: Some(WindowDecorations::Client),
        window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
            None, dimensions, cx,
        ))),
        ..Default::default()
    }
}

#[cfg(target_os = "macos")]
mod mac {
    use super::*;
    use crate::workspace::{DragEvent, DragSignal, Workspace};
    use block2::RcBlock;
    use objc2::{MainThreadMarker, rc::Retained, runtime::AnyObject};
    use objc2_app_kit::{NSEvent, NSEventMask, NSEventType, NSView, NSWindow, NSWindowButton};
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use std::ptr::NonNull;

    fn with_window<T>(w: &Window, use_window: impl FnOnce(&NSWindow, &NSView) -> T) -> Option<T> {
        if let Ok(handle) = HasWindowHandle::window_handle(w)
            && let RawWindowHandle::AppKit(raw) = handle.as_raw()
        {
            // GPUI supplies its live NSView on the AppKit main thread.
            let view = unsafe { raw.ns_view.cast::<NSView>().as_ref() };
            if let Some(window) = view.window() {
                return Some(use_window(&window, view));
            }
        }
        None
    }

    // GPUI reports the active window as hovered on macOS, even after the pointer leaves it.
    pub fn pointer_over_window(w: &Window) -> bool {
        let Some(mtm) = MainThreadMarker::new() else {
            return false;
        };
        let hit = NSWindow::windowNumberAtPoint_belowWindowWithWindowNumber(
            NSEvent::mouseLocation(),
            0,
            mtm,
        );
        with_window(w, |win, _| win.windowNumber()) == Some(hit)
    }

    // AppKit's hit test honors windows from other applications. Skip the
    // moving window only when it actually covers the pointer.
    pub fn tab_dock_sample(
        w: &Window,
    ) -> Option<(bool, impl Fn(&Window) -> Option<Point<Pixels>>)> {
        let screen = NSEvent::mouseLocation();
        let mtm = MainThreadMarker::new()?;
        let moving = with_window(w, |win, _| win.windowNumber())?;
        let mut hit = NSWindow::windowNumberAtPoint_belowWindowWithWindowNumber(screen, 0, mtm);
        if hit == moving {
            hit = NSWindow::windowNumberAtPoint_belowWindowWithWindowNumber(screen, moving, mtm);
        }
        let local = move |w: &Window| {
            (with_window(w, |win, _| win.windowNumber())? == hit)
                .then(|| tab_pointer(w))?
                .map(|(_, p)| p)
        };
        Some((NSEvent::pressedMouseButtons() & 1 != 0, local))
    }

    pub fn tab_pointer(w: &Window) -> Option<(bool, Point<Pixels>)> {
        with_window(w, |win, view| {
            let p = view
                .convertPoint_fromView(win.convertPointFromScreen(NSEvent::mouseLocation()), None);
            let bounds = view.bounds();
            let y = if view.isFlipped() {
                p.y - bounds.origin.y
            } else {
                bounds.origin.y + bounds.size.height - p.y
            };
            (
                NSEvent::pressedMouseButtons() & 1 != 0,
                point(px((p.x - bounds.origin.x) as f32), px(y as f32)),
            )
        })
    }

    pub(crate) struct DragEvents {
        monitor: Retained<AnyObject>,
        _moved: Subscription,
    }

    impl DragEvents {
        pub(crate) fn pointer(&self, w: &Window) -> Option<(bool, Point<Pixels>)> {
            tab_pointer(w)
        }
    }

    impl Drop for DragEvents {
        fn drop(&mut self) {
            unsafe {
                NSEvent::removeMonitor(&self.monitor);
            }
        }
    }

    pub(crate) fn mac_drag_events(
        w: &mut Window,
        cx: &mut Context<Workspace>,
        signal: DragSignal,
    ) -> Option<DragEvents> {
        let moved = signal.clone();
        // AppKit moves window frames in the Window Server. Its bounds notification
        // also arrives when performWindowDragWithEvent consumes a drag event.
        let moved = cx.observe_window_bounds(w, move |_, _, _| moved.send(DragEvent::Moved));
        let handler = RcBlock::new(move |event: NonNull<NSEvent>| {
            let kind = unsafe { event.as_ref() }.r#type();
            // A new press can follow an omitted AppKit mouse-up. Its location
            // belongs to the next gesture and cannot be used as a drop target.
            let event_kind = if kind == NSEventType::LeftMouseUp {
                DragEvent::Released
            } else if kind == NSEventType::LeftMouseDown {
                DragEvent::Cancelled
            } else {
                DragEvent::Moved
            };
            signal.send(event_kind);
            event.as_ptr()
        });
        let mask = NSEventMask::LeftMouseDragged
            | NSEventMask::LeftMouseUp
            | NSEventMask::LeftMouseDown
            | NSEventMask::MouseMoved;
        let monitor =
            unsafe { NSEvent::addLocalMonitorForEventsMatchingMask_handler(mask, &handler) }?;
        Some(DragEvents {
            monitor,
            _moved: moved,
        })
    }

    pub fn hide_system_buttons(w: &Window) {
        with_window(w, |window, _| {
            for button in [
                NSWindowButton::CloseButton,
                NSWindowButton::MiniaturizeButton,
                NSWindowButton::ZoomButton,
            ] {
                if let Some(button) = window.standardWindowButton(button) {
                    button.setHidden(true);
                }
            }
        });
    }

    pub fn drag(w: &Window) {
        with_window(w, |window, _| {
            if let Some(event) = window.currentEvent() {
                window.performWindowDragWithEvent(&event);
            }
        });
    }
}

#[cfg(target_os = "macos")]
pub use mac::hide_system_buttons;

#[cfg(target_os = "macos")]
pub(super) use mac::{
    drag as start_tab_window_move, mac_drag_events, pointer_over_window, tab_dock_sample,
};

#[cfg(not(target_os = "macos"))]
pub(super) fn pointer_over_window(window: &Window) -> bool {
    window.is_window_hovered()
}

fn caption(content: Div, _w: &mut Window, _cx: &mut App) -> impl IntoElement {
    let area = div()
        .id("window-drag-area")
        .flex_1()
        .min_w(px(24.))
        .h_full()
        .flex()
        .items_center()
        .overflow_hidden()
        .child(content);
    #[cfg(target_os = "windows")]
    let area = area.window_control_area(WindowControlArea::Drag);
    #[cfg(target_os = "macos")]
    let area = area.on_mouse_down(MouseButton::Left, |event, w, _| {
        if event.click_count == 2 {
            w.zoom_window();
        } else {
            mac::drag(w);
        }
    });
    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    let area = {
        // Starting a compositor move on mouse-down consumes the release that
        // distinguishes a double-click from a drag.
        let press = _w.use_keyed_state("caption-press", _cx, |_, _| {
            std::cell::Cell::new(None::<Point<Pixels>>)
        });
        let movement = press.clone();
        area.on_mouse_down(MouseButton::Left, move |e, w, cx| {
            if e.click_count == 2 {
                press.read(cx).set(None);
                w.zoom_window();
            } else {
                press.read(cx).set(Some(e.position));
            }
        })
        .on_mouse_move(move |e, w, cx| {
            let start = movement.read(cx);
            if !e.dragging() {
                start.set(None);
            } else if start
                .get()
                .is_some_and(|p| (e.position - p).magnitude() >= 3.)
            {
                start.set(None);
                w.start_window_move();
            }
        })
    };
    #[cfg(not(target_os = "windows"))]
    let area = area.on_mouse_down(MouseButton::Right, |event, w, _| {
        w.show_window_menu(event.position)
    });
    area
}

#[derive(Clone, Copy)]
enum Control {
    Minimize,
    Maximize,
    Close,
}

impl Control {
    fn button(self, maximized: bool, ui: Ui) -> Stateful<Div> {
        let (id, paths): (_, &[&[(f32, f32)]]) = match self {
            Self::Minimize => ("minimize-window", &[&[(1., 7.), (11., 7.)]]),
            Self::Maximize if maximized => (
                "restore-window",
                &[
                    &[(3.5, 3.5), (3.5, 1.), (11., 1.), (11., 8.5), (8.5, 8.5)],
                    &[(1., 3.5), (8.5, 3.5), (8.5, 11.), (1., 11.), (1., 3.5)],
                ],
            ),
            Self::Maximize => (
                "maximize-window",
                &[&[(1., 1.), (11., 1.), (11., 11.), (1., 11.), (1., 1.)]],
            ),
            Self::Close => (
                "close-window",
                &[&[(1., 1.), (11., 11.)], &[(11., 1.), (1., 11.)]],
            ),
        };
        let icon = canvas(
            |_, _, _| (),
            move |b, _, w, _| {
                let mut path = PathBuilder::stroke(px(1.2));
                for points in paths {
                    let (first, rest) = points.split_first().unwrap();
                    path.move_to(b.origin + point(px(first.0), px(first.1)));
                    for &(x, y) in rest {
                        path.line_to(b.origin + point(px(x), px(y)));
                    }
                }
                if let Ok(path) = path.build() {
                    w.paint_path(path, w.text_style().color);
                }
            },
        )
        .size(px(12.));
        let button = div()
            .id(id)
            .w(px(44.))
            .h_full()
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .text_color(rgb(ui.colors.chrome.foreground))
            .hover(move |s| match self {
                Self::Close => s
                    .bg(rgb(ui.colors.destructive.background))
                    .text_color(rgb(ui.colors.destructive.foreground)),
                _ => s.bg(rgb(ui.colors.secondary_hover)),
            })
            .child(icon);
        match self {
            Self::Close => button.on_click(|_, w, _| w.remove_window()),
            // The native maximize hit area also enables the Windows snap menu.
            #[cfg(target_os = "windows")]
            Self::Maximize => button.window_control_area(WindowControlArea::Max),
            #[cfg(target_os = "windows")]
            Self::Minimize => button.window_control_area(WindowControlArea::Min),
            #[cfg(not(target_os = "windows"))]
            Self::Maximize => button.on_click(|_, w, _| w.zoom_window()),
            #[cfg(not(target_os = "windows"))]
            Self::Minimize => button.on_click(|_, w, _| w.minimize_window()),
        }
    }
}

pub fn titlebar(
    ui: Ui,
    leading: impl IntoElement,
    caption_content: Div,
    actions: impl IntoElement,
    w: &mut Window,
    cx: &mut App,
) -> Div {
    // Some Linux compositors retain the server frame. In that case its own
    // controls handle minimize/maximize/close.
    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    let custom_controls = matches!(w.window_decorations(), Decorations::Client { .. });
    #[cfg(not(any(target_os = "linux", target_os = "freebsd")))]
    let custom_controls = true;
    let supported = w.window_controls();
    div()
        .w_full()
        .h(px(TITLEBAR_HEIGHT))
        .flex_none()
        .flex()
        .items_center()
        .bg(rgb(ui.colors.chrome.background))
        .text_color(rgb(ui.colors.chrome.foreground))
        .border_b_1()
        .border_color(rgb(ui.colors.border))
        .child(leading)
        .child(caption(caption_content, w, cx))
        .child(actions)
        .when(custom_controls, |d| {
            d.child(
                div()
                    .flex_none()
                    .h_full()
                    .flex()
                    .items_center()
                    .when(supported.minimize, |d| {
                        d.child(Control::Minimize.button(false, ui))
                    })
                    .when(supported.maximize, |d| {
                        d.child(Control::Maximize.button(w.is_maximized(), ui))
                    })
                    .child(Control::Close.button(false, ui)),
            )
        })
}

pub fn frame(content: impl IntoElement, _w: &Window) -> Div {
    let frame = div().relative().size_full().child(content);
    // Let the Win32 frame receive top-edge resize hits even above the drag
    // region and caption buttons. Maximized buttons extend to the screen edge.
    #[cfg(target_os = "windows")]
    let frame = frame.when(!_w.is_maximized() && !_w.is_fullscreen(), |f| {
        f.child(
            div()
                .absolute()
                .top_0()
                .left_0()
                .right_0()
                .h(px(4.))
                .occlude(),
        )
    });
    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    if let Decorations::Client { tiling } = _w.window_decorations()
        && !_w.is_maximized()
        && !_w.is_fullscreen()
    {
        use ResizeEdge::*;
        let mut frame = frame;
        for (edge, show, cursor) in [
            (Left, !tiling.left, CursorStyle::ResizeLeftRight),
            (Right, !tiling.right, CursorStyle::ResizeLeftRight),
            (Top, !tiling.top, CursorStyle::ResizeUpDown),
            (Bottom, !tiling.bottom, CursorStyle::ResizeUpDown),
            (
                TopLeft,
                !tiling.top && !tiling.left,
                CursorStyle::ResizeUpLeftDownRight,
            ),
            (
                BottomRight,
                !tiling.bottom && !tiling.right,
                CursorStyle::ResizeUpLeftDownRight,
            ),
            (
                TopRight,
                !tiling.top && !tiling.right,
                CursorStyle::ResizeUpRightDownLeft,
            ),
            (
                BottomLeft,
                !tiling.bottom && !tiling.left,
                CursorStyle::ResizeUpRightDownLeft,
            ),
        ] {
            if !show {
                continue;
            }
            let hit = div().absolute().occlude().cursor(cursor).on_mouse_down(
                MouseButton::Left,
                move |_, w, cx| {
                    cx.stop_propagation();
                    w.start_window_resize(edge);
                },
            );
            frame = frame.child(match edge {
                Left => hit.left_0().top_0().bottom_0().w(px(4.)),
                Right => hit.right_0().top_0().bottom_0().w(px(4.)),
                Top => hit.top_0().left_0().right_0().h(px(4.)),
                Bottom => hit.bottom_0().left_0().right_0().h(px(4.)),
                TopLeft => hit.top_0().left_0().size(px(7.)),
                TopRight => hit.top_0().right_0().size(px(7.)),
                BottomLeft => hit.bottom_0().left_0().size(px(7.)),
                BottomRight => hit.bottom_0().right_0().size(px(7.)),
            });
        }
        return frame;
    }
    frame
}
