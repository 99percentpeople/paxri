use gpui::{prelude::*, *};

const TEXT_FIELD: &str = "TextField";

pub(super) fn text_field(element: Stateful<Div>, focus: &FocusHandle) -> Stateful<Div> {
    let handle = focus.clone();
    element
        .track_focus(focus)
        .key_context(TEXT_FIELD)
        .on_mouse_down_out(move |_, window, _| {
            if handle.is_focused(window) {
                window.blur();
            }
        })
}

pub(crate) fn observe_window_focus<V: 'static>(
    window: &mut Window,
    fallback: &FocusHandle,
    cx: &mut Context<V>,
) {
    let tab = fallback.clone();
    cx.observe_window_activation(window, move |_, window, cx| {
        if !window.is_window_active() {
            // GPUI preserves logical focus across native window switches.
            if window
                .context_stack()
                .iter()
                .any(|ctx| ctx.contains(TEXT_FIELD))
            {
                tab.focus(window);
            }
            if KeyboardFocus::window(cx) == Some(window.window_handle()) {
                KeyboardFocus::set(None, cx);
            }
        }
    })
    .detach();

    let tab = fallback.clone();
    cx.on_focus_lost(window, move |_, window, cx| {
        // An outside press on another control already moved focus there.
        // After a press on background, keep Tab navigation in the workspace.
        if window.focused(cx).is_none() {
            tab.focus(window);
        }
    })
    .detach();
}

// Only show button focus rings in the window navigated with the keyboard.
struct KeyboardFocus(Option<AnyWindowHandle>);
impl Global for KeyboardFocus {}

impl KeyboardFocus {
    fn window(cx: &App) -> Option<AnyWindowHandle> {
        cx.try_global::<Self>().and_then(|focus| focus.0)
    }

    fn set(window: Option<AnyWindowHandle>, cx: &mut App) {
        if Self::window(cx) != window {
            cx.set_global(Self(window));
            cx.refresh_windows();
        }
    }
}

pub(super) fn keyboard_focus_visible(window: &Window, cx: &App) -> bool {
    window.is_window_active() && KeyboardFocus::window(cx) == Some(window.window_handle())
}

pub(super) fn workspace_keyboard(root: Div) -> Div {
    root.capture_key_down(|event, window, cx| {
        if matches!(
            event.keystroke.key.as_str(),
            "tab" | "up" | "down" | "left" | "right" | "enter" | "space" | "escape"
        ) {
            KeyboardFocus::set(Some(window.window_handle()), cx);
        }
    })
    .capture_any_mouse_down(|_, _, cx| KeyboardFocus::set(None, cx))
    .on_key_down(|event: &KeyDownEvent, window, cx| {
        let key = &event.keystroke;
        if key.key == "tab" && (!key.modifiers.modified() || key.modifiers == Modifiers::shift()) {
            if key.modifiers.shift {
                window.focus_prev();
            } else {
                window.focus_next();
            }
            window.prevent_default();
            cx.stop_propagation();
        }
    })
}
