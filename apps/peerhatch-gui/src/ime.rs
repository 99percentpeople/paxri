//! GPUI tabs share an HWND and therefore a Windows IME context.
use gpui::Window;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use windows_sys::Win32::{
    Foundation::{HWND, LPARAM, LRESULT, WPARAM},
    UI::{
        Input::Ime::{
            CPS_CANCEL, ImmGetContext, ImmNotifyIME, ImmReleaseContext, NI_COMPOSITIONSTR,
        },
        Shell::{DefSubclassProc, GetWindowSubclass, RemoveWindowSubclass, SetWindowSubclass},
        WindowsAndMessaging::{
            PostMessageW, WM_APP, WM_IME_CHAR, WM_IME_COMPOSITION, WM_IME_ENDCOMPOSITION,
            WM_IME_STARTCOMPOSITION, WM_KILLFOCUS, WM_NCDESTROY,
        },
    },
};

const CANCEL: u32 = WM_APP + 1;
const IDLE: usize = 0;
const COMPOSING: usize = 1;
const CANCEL_PENDING: usize = 2;
const CANCELLED: usize = 3;

fn hwnd(window: &Window) -> Option<HWND> {
    match HasWindowHandle::window_handle(window).ok()?.as_raw() {
        RawWindowHandle::Win32(handle) => Some(handle.hwnd.get() as HWND),
        _ => None,
    }
}

fn set_state(hwnd: HWND, state: usize) {
    unsafe { SetWindowSubclass(hwnd, Some(ime_message), 0, state) };
}

pub fn observe(window: &Window) {
    if let Some(hwnd) = hwnd(window) {
        set_state(hwnd, IDLE);
    }
}

pub fn blur(window: &Window) {
    let Some(hwnd) = hwnd(window) else { return };
    let mut state = IDLE;
    if unsafe { GetWindowSubclass(hwnd, Some(ime_message), 0, &mut state) } != 0
        && state == COMPOSING
    {
        // IMM can synchronously send composition events. Leave the GPUI update first.
        set_state(hwnd, CANCEL_PENDING);
        unsafe { PostMessageW(hwnd, CANCEL, 0, 0) };
    }
}

unsafe extern "system" fn ime_message(
    hwnd: HWND,
    msg: u32,
    wp: WPARAM,
    lp: LPARAM,
    _: usize,
    state: usize,
) -> LRESULT {
    match msg {
        WM_IME_STARTCOMPOSITION => set_state(hwnd, COMPOSING),
        WM_IME_ENDCOMPOSITION => set_state(hwnd, IDLE),
        WM_IME_COMPOSITION | WM_IME_CHAR if matches!(state, CANCEL_PENDING | CANCELLED) => {
            return 0;
        }
        WM_KILLFOCUS if matches!(state, COMPOSING | CANCEL_PENDING) => cancel(hwnd),
        CANCEL => {
            // A new composition or a normal end supersedes a queued cancellation.
            if state == CANCEL_PENDING {
                cancel(hwnd);
            }
            return 0;
        }
        WM_NCDESTROY => unsafe {
            RemoveWindowSubclass(hwnd, Some(ime_message), 0);
        },
        _ => {}
    }
    unsafe { DefSubclassProc(hwnd, msg, wp, lp) }
}

fn cancel(hwnd: HWND) {
    set_state(hwnd, CANCELLED);
    unsafe {
        let context = ImmGetContext(hwnd);
        if !context.is_null() {
            ImmNotifyIME(context, NI_COMPOSITIONSTR, CPS_CANCEL, 0);
            ImmReleaseContext(hwnd, context);
        }
    }
}
