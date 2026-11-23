use gpui::{AnyWindowHandle, App, Global, Window};

// Only show button focus rings in the window navigated with the keyboard.
pub struct KeyboardFocus(Option<AnyWindowHandle>);
impl Global for KeyboardFocus {}

impl KeyboardFocus {
    pub fn window(cx: &App) -> Option<AnyWindowHandle> {
        cx.try_global::<Self>().and_then(|focus| focus.0)
    }

    pub fn set(window: Option<AnyWindowHandle>, cx: &mut App) {
        if Self::window(cx) != window {
            cx.set_global(Self(window));
            cx.refresh_windows();
        }
    }
}

pub(super) fn keyboard_focus_visible(window: &Window, cx: &App) -> bool {
    window.is_window_active() && KeyboardFocus::window(cx) == Some(window.window_handle())
}
