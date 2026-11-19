//! Use the system move loop and observe its notifications for eager docking.
use super::*;
use raw_window_handle::RawWindowHandle;
use std::{cell::Cell, rc::Rc};
use windows_sys::Win32::{
    Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM},
    Graphics::{
        Dwm::{
            DWMWA_CLOAKED, DWMWA_TRANSITIONS_FORCEDISABLED, DwmGetWindowAttribute,
            DwmSetWindowAttribute,
        },
        Gdi::{ClientToScreen, RDW_INVALIDATE, RDW_UPDATENOW, RedrawWindow, ScreenToClient},
    },
    UI::{
        HiDpi::GetDpiForWindow,
        Input::KeyboardAndMouse::{
            GetAsyncKeyState, GetCapture, ReleaseCapture, SetCapture, VK_ESCAPE, VK_LBUTTON,
            VK_RBUTTON,
        },
        Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass},
        WindowsAndMessaging::{
            GA_ROOT, GW_HWNDNEXT, GWL_EXSTYLE, GetAncestor, GetCursorPos, GetSystemMetrics,
            GetWindow, GetWindowLongPtrW, GetWindowRect, HTCAPTION, IsIconic, IsWindowVisible,
            IsZoomed, PostMessageW, SC_MOVE, SM_SWAPBUTTON, SW_MAXIMIZE, SW_RESTORE,
            SWP_NOACTIVATE, SWP_NOSIZE, SWP_NOZORDER, SendMessageW, SetWindowPos, ShowWindow,
            WM_CANCELMODE, WM_CAPTURECHANGED, WM_KEYDOWN, WM_LBUTTONUP, WM_MOUSEMOVE, WM_MOVING,
            WM_NCDESTROY, WM_SYSCOMMAND, WS_EX_TRANSPARENT, WindowFromPoint,
        },
    },
};

pub(super) struct Origin {
    window: WindowHandle<Workspace>,
    page: Page,
    index: usize,
    active: Page,
}

impl Origin {
    pub(super) fn new(
        view: &Workspace,
        window: WindowHandle<Workspace>,
        tab: &TabDrag,
    ) -> Option<Self> {
        if view.tabs.len() == 1 {
            return None;
        }
        Some(Self {
            window,
            page: tab.page.clone(),
            index: view.tabs.iter().position(|p| *p == tab.page)?,
            active: tab.previous_active.clone(),
        })
    }

    fn restore_tabs(&self, v: &mut Workspace, w: &mut Window, cx: &mut Context<Workspace>) {
        if !v.tabs.contains(&self.page) {
            v.tabs
                .insert(self.index.min(v.tabs.len()), self.page.clone());
        }
        v.refresh_title(w);
        v.focus_tab(self.active.clone(), w, cx);
    }

    fn restore(&self, cx: &mut App) -> bool {
        self.window
            .update(cx, |v, w, cx| self.restore_tabs(v, w, cx))
            .is_ok()
    }
}

pub(super) fn tear_off(
    tab: TabDrag,
    placement: Option<DropPlacement>,
    grab: Option<Point<Pixels>>,
    indent: Pixels,
    w: &Window,
    cx: &mut Context<Workspace>,
) {
    let source = w
        .window_handle()
        .downcast::<Workspace>()
        .expect("workspace root");
    cx.spawn_in(w, async move |view, cx| {
        let Ok(Some((origin, hwnd))) = view.update_in(cx, |v, w, cx| {
            let origin = Origin::new(v, source, &tab)?;
            v.take_tab(&tab.page, w, cx);
            Some((origin, native_handle(w)))
        }) else {
            return;
        };
        // Creating the DirectX window and entering the system move loop can
        // delay the source's queued paint until mouse-up. Present it first,
        // outside the GPUI update so its WM_PAINT handler can enter the app.
        if let Some(hwnd) = hwnd {
            unsafe {
                RedrawWindow(
                    hwnd,
                    std::ptr::null(),
                    std::ptr::null_mut(),
                    RDW_INVALIDATE | RDW_UPDATENOW,
                )
            };
        }
        let _ = view.update_in(cx, |v, w, cx| {
            let held_indent = if grab.is_some() && button_down() {
                indent
            } else {
                px(0.)
            };
            if let Some(dest) = v.open_tab_window(tab.page, placement, held_indent, cx) {
                if let Some(grab) = grab {
                    cx.defer(move |cx| {
                        let _ = dest.update(cx, |v, w, cx| {
                            follow(v, w, cx, grab, tab.offset, Some(origin));
                        });
                    });
                }
            } else {
                origin.restore_tabs(v, w, cx);
            }
        });
    })
    .detach();
}

fn native_handle(w: &Window) -> Option<HWND> {
    match raw_window_handle::HasWindowHandle::window_handle(w)
        .ok()?
        .as_raw()
    {
        RawWindowHandle::Win32(handle) => Some(handle.hwnd.get() as HWND),
        _ => None,
    }
}

pub(in crate::workspace) fn prepare_detached_window(w: &Window) {
    let Some(hwnd) = native_handle(w) else { return };
    let set_disabled = move |disabled: bool| {
        let value = i32::from(disabled);
        unsafe {
            DwmSetWindowAttribute(
                hwnd,
                DWMWA_TRANSITIONS_FORCEDISABLED as u32,
                (&raw const value).cast(),
                size_of::<i32>() as u32,
            )
        }
    };
    // A tear-off already follows the pointer. Suppress the system's additional
    // fade and scale before GPUI shows the new HWND on activation.
    if set_disabled(true) >= 0 {
        // GPUI runs frame callbacks before painting. Restore normal transitions
        // on the frame following the first presentation.
        w.on_next_frame(move |w, _| {
            w.on_next_frame(move |_, _| {
                set_disabled(false);
            });
        });
    }
}

fn button_down() -> bool {
    // GetAsyncKeyState uses physical buttons, including on a left-handed mouse.
    let primary = if unsafe { GetSystemMetrics(SM_SWAPBUTTON) } != 0 {
        VK_RBUTTON
    } else {
        VK_LBUTTON
    };
    unsafe { GetAsyncKeyState(i32::from(primary)) < 0 }
}

fn cursor_position() -> Option<POINT> {
    let mut pt = POINT::default();
    (unsafe { GetCursorPos(&mut pt) } != 0).then_some(pt)
}

// The moving workspace covers the pixel we need to hit-test. Respect the
// desktop stacking order, including unrelated windows above a potential target.
fn below_cursor(moving: HWND, pt: POINT) -> Option<HWND> {
    unsafe {
        let hit = GetAncestor(WindowFromPoint(pt), GA_ROOT);
        if hit != moving {
            return (!hit.is_null()).then_some(hit);
        }
        let mut next = GetWindow(moving, GW_HWNDNEXT);
        while !next.is_null() {
            let mut rect = RECT::default();
            if IsWindowVisible(next) != 0
                && IsIconic(next) == 0
                && GetWindowRect(next, &mut rect) != 0
                && pt.x >= rect.left
                && pt.x < rect.right
                && pt.y >= rect.top
                && pt.y < rect.bottom
                && GetWindowLongPtrW(next, GWL_EXSTYLE) as u32 & WS_EX_TRANSPARENT == 0
            {
                let mut cloaked = 0u32;
                DwmGetWindowAttribute(
                    next,
                    DWMWA_CLOAKED as u32,
                    (&raw mut cloaked).cast(),
                    size_of::<u32>() as u32,
                );
                if cloaked == 0 {
                    return Some(next);
                }
            }
            next = GetWindow(next, GW_HWNDNEXT);
        }
    }
    None
}

fn target_at(v: &Workspace, w: &Window, hwnd: HWND, pt: POINT, cx: &mut App) -> Option<DockTarget> {
    let hit = below_cursor(hwnd, pt)?;
    v.dock_target_at(w, cx, |_, target| {
        if native_handle(target)? != hit {
            return None;
        }
        let mut local = pt;
        if unsafe { ScreenToClient(hit, &mut local) } == 0 {
            return None;
        }
        Some(point(
            px(local.x as f32 / target.scale_factor()),
            px(local.y as f32 / target.scale_factor()),
        ))
    })
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum MoveLoop {
    Pending,
    Running,
    Docking,
    Finished,
}

struct Listener {
    signal: DragSignal,
    window_grab: Option<Point<Pixels>>,
    system_move: Rc<Cell<MoveLoop>>,
}

pub(super) struct Events {
    hwnd: HWND,
    listener: Box<Listener>,
}

unsafe extern "system" fn window_event(
    hwnd: HWND,
    msg: u32,
    wp: WPARAM,
    lp: LPARAM,
    id: usize,
    data: usize,
) -> LRESULT {
    // SetWindowSubclass and the guard both live on this HWND's UI thread. Only
    // enqueue here: Win32 can call back while GPUI is already borrowing a window.
    let listener = unsafe { &*(data as *const Listener) };
    let moving = listener.window_grab.is_some();
    if let Some(grab) = listener.window_grab
        && msg == WM_SYSCOMMAND
        && wp == (SC_MOVE | HTCAPTION) as usize
    {
        let signal = listener.signal.clone();
        let state = listener.system_move.clone();
        if !button_down() {
            signal.send(DragEvent::Released);
            return 0;
        }
        // Pointer motion may be coalesced between creating the floating HWND and
        // entering the modal loop. Anchor both the frame and the native command
        // to the cursor that actually starts this move.
        let Some(cursor) = cursor_position() else {
            signal.send(DragEvent::Cancelled);
            return 0;
        };
        align_window(hwnd, grab, cursor);
        state.set(MoveLoop::Running);
        // DefSubclassProc enters the modal system move here. Wait for its return
        // before transferring capture: the loop releases capture as it unwinds.
        let result = unsafe { DefSubclassProc(hwnd, msg, wp, screen_lparam(cursor)) };
        let docking = state.replace(MoveLoop::Finished) == MoveLoop::Docking;
        signal.send(if !button_down() {
            DragEvent::Released
        } else if docking {
            DragEvent::Moved
        } else {
            DragEvent::Cancelled
        });
        return result;
    }
    let event = match msg {
        WM_MOVING if moving => Some(DragEvent::Moved),
        WM_MOUSEMOVE if !moving => Some(DragEvent::Moved),
        WM_LBUTTONUP if !moving || listener.system_move.get() == MoveLoop::Pending => {
            Some(DragEvent::Released)
        }
        WM_KEYDOWN if wp == VK_ESCAPE as usize => Some(DragEvent::Cancelled),
        WM_CAPTURECHANGED if !moving => Some(if button_down() {
            DragEvent::Cancelled
        } else {
            DragEvent::Released
        }),
        WM_CANCELMODE if !moving => Some(DragEvent::Cancelled),
        WM_NCDESTROY => {
            unsafe {
                RemoveWindowSubclass(hwnd, Some(window_event), id);
            }
            Some(DragEvent::Cancelled)
        }
        _ => None,
    };
    if let Some(event) = event {
        listener.signal.send(event);
    }
    unsafe { DefSubclassProc(hwnd, msg, wp, lp) }
}

impl Events {
    pub(super) fn subscribe(
        w: &Window,
        signal: DragSignal,
        window_grab: Option<Point<Pixels>>,
    ) -> Option<Self> {
        let hwnd = native_handle(w)?;
        let listener = Box::new(Listener {
            signal,
            window_grab,
            system_move: Rc::new(Cell::new(MoveLoop::Pending)),
        });
        let id = (&*listener as *const Listener) as usize;
        if unsafe { SetWindowSubclass(hwnd, Some(window_event), id, id) } == 0 {
            return None;
        }
        if window_grab.is_none() && button_down() {
            unsafe {
                SetCapture(hwnd);
            }
        }
        Some(Self { hwnd, listener })
    }

    pub(super) fn pointer(&self, w: &Window) -> Option<(bool, Point<Pixels>)> {
        let mut p = cursor_position()?;
        if unsafe { ScreenToClient(self.hwnd, &mut p) } == 0 {
            return None;
        }
        Some((
            button_down(),
            point(
                px(p.x as f32 / w.scale_factor()),
                px(p.y as f32 / w.scale_factor()),
            ),
        ))
    }

    fn finish_before_docking(&self) -> bool {
        match self.listener.system_move.get() {
            MoveLoop::Running => {
                self.listener.system_move.set(MoveLoop::Docking);
                unsafe {
                    SendMessageW(self.hwnd, WM_CANCELMODE, 0, 0);
                }
                true
            }
            MoveLoop::Docking => true,
            _ => false,
        }
    }

    fn end_move(&self) {
        if self.listener.system_move.get() == MoveLoop::Running {
            unsafe {
                SendMessageW(self.hwnd, WM_CANCELMODE, 0, 0);
            }
        }
    }
}

impl Drop for Events {
    fn drop(&mut self) {
        let id = (&*self.listener as *const Listener) as usize;
        unsafe {
            RemoveWindowSubclass(self.hwnd, Some(window_event), id);
            if self.listener.window_grab.is_none() && GetCapture() == self.hwnd {
                ReleaseCapture();
            }
        }
    }
}

fn screen_lparam(cursor: POINT) -> LPARAM {
    (((cursor.y as u16 as u32) << 16) | cursor.x as u16 as u32) as LPARAM
}

fn align_window(hwnd: HWND, grab: Point<Pixels>, cursor: POINT) {
    let mut client = POINT::default();
    let mut frame = RECT::default();
    // Win32 supplies physical desktop coordinates, including on mixed-DPI displays.
    unsafe {
        if ClientToScreen(hwnd, &mut client) == 0 || GetWindowRect(hwnd, &mut frame) == 0 {
            return;
        }
        let grab = grab.scale(GetDpiForWindow(hwnd) as f32 / 96.);
        let dx = cursor.x - client.x - f64::from(grab.x).round() as i32;
        let dy = cursor.y - client.y - f64::from(grab.y).round() as i32;
        if dx != 0 || dy != 0 {
            SetWindowPos(
                hwnd,
                std::ptr::null_mut(),
                frame.left + dx,
                frame.top + dy,
                0,
                0,
                SWP_NOSIZE | SWP_NOACTIVATE | SWP_NOZORDER,
            );
        }
    }
}

pub(super) fn follow(
    v: &mut Workspace,
    w: &mut Window,
    cx: &mut Context<Workspace>,
    window_grab: Point<Pixels>,
    tab_grab: Point<Pixels>,
    origin: Option<Origin>,
) {
    let (Some(hwnd), Some(cursor)) = (native_handle(w), cursor_position()) else {
        v.settle_tab(cx);
        return;
    };
    if !button_down() {
        v.settle_tab(cx);
        if let Some(target) = target_at(v, w, hwnd, cursor, cx) {
            v.dock_at_pointer(target, None, w, cx);
        }
        return;
    }
    let mut initial = RECT::default();
    let maximized = unsafe { IsZoomed(hwnd) } != 0;
    unsafe {
        if GetWindowRect(hwnd, &mut initial) == 0 {
            v.settle_tab(cx);
            return;
        }
        if maximized {
            ShowWindow(hwnd, SW_RESTORE);
        }
        // Hand GPUI's initiating client-area press to the same native move loop
        // that owns a titlebar drag. The OS updates position at input cadence.
        if !GetCapture().is_null() {
            ReleaseCapture();
        }
    }
    align_window(hwnd, window_grab, cursor);
    let mut docking = None;
    v.watch_drag(
        w,
        cx,
        |signal, w, _| Events::subscribe(w, signal, Some(window_grab)),
        move |events, event, v, w, cx| {
            if event == DragEvent::Cancelled {
                events.end_move();
                if let Some(origin) = &origin {
                    if origin.restore(cx) {
                        w.remove_window();
                    }
                } else {
                    unsafe {
                        SetWindowPos(
                            hwnd,
                            std::ptr::null_mut(),
                            initial.left,
                            initial.top,
                            initial.right - initial.left,
                            initial.bottom - initial.top,
                            SWP_NOZORDER | SWP_NOACTIVATE,
                        );
                        if maximized {
                            ShowWindow(hwnd, SW_MAXIMIZE);
                        }
                    }
                }
                return false;
            }
            let held = event != DragEvent::Released && button_down();
            // The pointer can advance while the system move loop exits.
            let target = cursor_position()
                .and_then(|pt| target_at(v, w, hwnd, pt, cx))
                .or(docking.take());
            if let Some(target) = target {
                if held && events.finish_before_docking() {
                    docking = Some(target);
                    return true;
                }
                if v.dock_at_pointer(target, held.then_some(tab_grab), w, cx) {
                    return false;
                }
            }
            held
        },
    );
    if unsafe {
        PostMessageW(
            hwnd,
            WM_SYSCOMMAND,
            (SC_MOVE | HTCAPTION) as usize,
            screen_lparam(cursor),
        )
    } == 0
    {
        v.cancel_tab_drag(w, cx);
    }
}
