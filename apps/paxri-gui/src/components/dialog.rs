//! An owned native window for a form or a confirmation. The content model owns its draft.
use super::{ButtonVariant::Secondary, Ui, focus::workspace_keyboard, observe_window_focus};
use gpui::{prelude::*, *};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum DialogMode {
    Modal,
    Modeless,
}

pub(crate) enum DialogSuccess {
    Close,
    KeepOpen,
}

pub(crate) struct DialogContent {
    body: AnyElement,
    dismiss: &'static str,
    actions: Vec<AnyElement>,
}

impl DialogContent {
    pub(crate) fn new(body: impl IntoElement, dismiss: &'static str) -> Self {
        Self {
            body: body.into_any_element(),
            dismiss,
            actions: Vec::new(),
        }
    }

    pub(crate) fn action(mut self, button: impl IntoElement) -> Self {
        self.actions.push(button.into_any_element());
        self
    }
}

pub(crate) struct Dialog {
    title: &'static str,
    owner: AnyWindowHandle,
    mode: DialogMode,
    focus: FocusHandle,
    content: Box<dyn Fn(Ui, &mut App) -> DialogContent>,
    _owner_lifetime: Subscription,
}

impl Dialog {
    fn child_of(
        owner: AnyWindowHandle,
        cx: &App,
        matches: impl Fn(&Self) -> bool,
    ) -> Option<WindowHandle<Self>> {
        cx.windows()
            .into_iter()
            .filter_map(|w| w.downcast::<Self>())
            .find(|w| w.read(cx).is_ok_and(|d| d.owner == owner && matches(d)))
    }

    /// A dialog blocks input only in its owning workspace. Other windows stay interactive.
    pub(crate) fn guard_owner(content: impl IntoElement, owner: &Window, ui: Ui, cx: &App) -> Div {
        let root = div().relative().size_full().child(content);
        let Some(child) =
            Self::child_of(owner.window_handle(), cx, |d| d.mode == DialogMode::Modal)
        else {
            return root;
        };
        let activate = move |w: &mut Window, cx: &mut App| {
            w.prevent_default();
            cx.stop_propagation();
            let _ = child.update(cx, |_, w, _| w.activate_window());
        };
        root.capture_any_mouse_down(move |_, w, cx| activate(w, cx))
            .capture_key_down(move |_, w, cx| activate(w, cx))
            .child(
                deferred(
                    div()
                        .absolute()
                        .inset_0()
                        .occlude()
                        .bg(rgba(ui.colors.modal_overlay)),
                )
                .priority(1),
            )
    }

    pub(crate) fn show_error(window: &Window, error: impl Into<String>, cx: &mut App) {
        super::Toast::show(error.into(), window.window_handle(), cx);
    }

    pub(crate) fn complete(
        handle: AnyWindowHandle,
        result: Result<(), String>,
        on_success: DialogSuccess,
        cx: &mut App,
    ) {
        match result {
            Err(error) => super::Toast::show(error, handle, cx),
            Ok(()) if matches!(on_success, DialogSuccess::Close) => {
                cx.defer(move |cx| {
                    let _ = handle.update(cx, |_, w, _| w.remove_window());
                });
            }
            Ok(()) => {}
        }
    }

    pub(crate) fn open<T: 'static>(
        parent: &mut Window,
        cx: &mut App,
        mode: DialogMode,
        title: &'static str,
        dimensions: Size<Pixels>,
        model: Entity<T>,
        initial_focus: Option<FocusHandle>,
        render: impl Fn(&T, Ui, &Context<T>) -> DialogContent + 'static,
    ) {
        // open_window draws synchronously. Finish the originating control's update first.
        parent.defer(cx, move |parent, cx| {
            let owner = parent.window_handle();
            if let Some(existing) = Self::child_of(owner, cx, |d| d.mode == DialogMode::Modal)
                .or_else(|| Self::child_of(owner, cx, |d| d.title == title))
            {
                let _ = existing.update(cx, |_, w, _| w.activate_window());
                return;
            }
            let return_focus = parent.focused(cx);
            let display = parent.display(cx);
            let mut bounds = Bounds::centered_at(parent.bounds().center(), dimensions);
            if let Some(display) = &display {
                let screen = display.bounds();
                bounds.size.width = bounds.size.width.min(screen.size.width);
                bounds.size.height = bounds.size.height.min(screen.size.height);
                bounds.origin = bounds.origin.clamp(
                    &screen.origin,
                    &(screen.bottom_right() - point(bounds.size.width, bounds.size.height)),
                );
            }
            let options = WindowOptions {
                kind: WindowKind::Floating,
                // Win32 must attach the owner before showing the new top-level window.
                show: !cfg!(windows),
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                display_id: display.map(|d| d.id()),
                titlebar: Some(TitlebarOptions {
                    title: Some(title.into()),
                    appears_transparent: true,
                    ..Default::default()
                }),
                window_decorations: Some(WindowDecorations::Client),
                is_resizable: false,
                is_minimizable: false,
                window_min_size: Some(bounds.size),
                app_id: Some("paxri".into()),
                ..Default::default()
            };
            let result = cx.open_window(options, |w, cx| {
                w.set_window_title(title);
                crate::theme::observe(w);
                attach(parent, w);
                #[cfg(target_os = "macos")]
                crate::chrome::hide_system_buttons(w);
                let child = w.window_handle();
                cx.new(|cx| {
                    let focus = cx.focus_handle();
                    observe_window_focus(w, &focus, cx);
                    w.focus(initial_focus.as_ref().unwrap_or(&focus));
                    cx.observe(&model, |_, _, cx| cx.notify()).detach();
                    let lifetime = cx.on_window_closed(move |cx| {
                        if !cx.windows().contains(&owner) {
                            let _ = child.update(cx, |_, w, _| w.remove_window());
                        }
                    });
                    cx.on_release(move |_, cx| {
                        cx.defer(move |cx| {
                            let _ = owner.update(cx, |_, w, cx| {
                                if mode == DialogMode::Modal
                                    && Self::child_of(owner, cx, |d| d.mode == DialogMode::Modal)
                                        .is_none()
                                {
                                    #[cfg(windows)]
                                    enable_owner(w, true);
                                    w.activate_window();
                                    if let Some(focus) = return_focus {
                                        w.focus(&focus);
                                    }
                                }
                                w.refresh();
                            });
                        });
                    })
                    .detach();
                    Self {
                        title,
                        owner,
                        mode,
                        focus,
                        content: Box::new(move |ui, cx| {
                            model.update(cx, |model, cx| render(model, ui, cx))
                        }),
                        _owner_lifetime: lifetime,
                    }
                })
            });
            match result {
                Ok(dialog) => {
                    #[cfg(windows)]
                    if mode == DialogMode::Modal {
                        enable_owner(parent, false);
                    }
                    let _ = dialog.update(cx, |_, w, _| w.activate_window());
                    parent.refresh();
                }
                Err(error) => {
                    let _ = parent.prompt(
                        PromptLevel::Critical,
                        "Could not open dialog",
                        Some(&error.to_string()),
                        &["Close"],
                        cx,
                    );
                }
            }
        });
    }
}

impl Render for Dialog {
    fn render(&mut self, w: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let ui = Ui::new(w, cx);
        let parts = (self.content)(ui, cx);
        let footer = div()
            .w_full()
            .flex_none()
            .flex()
            .flex_wrap()
            .justify_end()
            .gap_2()
            .px_4()
            .py_3()
            .border_t_1()
            .border_color(rgb(ui.colors.border))
            .child(
                ui.button(Secondary, "dialog-dismiss", parts.dismiss)
                    .on_click(crate::chrome::close_window),
            )
            .children(parts.actions);
        let content = workspace_keyboard(div())
            .size_full()
            .flex()
            .flex_col()
            .min_h_0()
            .track_focus(&self.focus)
            .bg(rgb(ui.colors.canvas.background))
            .text_color(rgb(ui.colors.canvas.foreground))
            .child(crate::chrome::dialog_titlebar(self.title, ui))
            .child(div().relative().flex_1().min_h_0().flex().flex_col()
                .child(div().id("dialog-content").w_full().flex_1().min_h_0()
                    .overflow_y_scroll().p_4().child(parts.body))
                .children(super::Toast::overlay(w, cx)))
            .child(footer)
            .on_key_down(|e, w, cx| {
                if e.keystroke.key == "escape" && !e.keystroke.modifiers.modified() {
                    cx.stop_propagation();
                    w.remove_window();
                }
            });
        content
    }
}

// GPUI sets the transient parent of Floating windows on Wayland and X11.
#[cfg(not(any(windows, target_os = "macos")))]
fn attach(_: &Window, _: &Window) {}

#[cfg(windows)]
fn enable_owner(w: &Window, enabled: bool) {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::EnableWindow;
    if let Ok(handle) = HasWindowHandle::window_handle(w)
        && let RawWindowHandle::Win32(handle) = handle.as_raw()
    {
        unsafe { EnableWindow(handle.hwnd.get() as _, enabled.into()) };
    }
}

#[cfg(windows)]
unsafe extern "system" fn dialog_frame(
    hwnd: windows_sys::Win32::Foundation::HWND,
    msg: u32,
    wp: usize,
    lp: isize,
    id: usize,
    _: usize,
) -> isize {
    use windows_sys::Win32::{
        Foundation::{POINT, RECT},
        Graphics::Gdi::ScreenToClient,
        UI::{
            HiDpi::GetDpiForWindow,
            Shell::{DefSubclassProc, RemoveWindowSubclass},
            WindowsAndMessaging::{
                DefWindowProcW, GetClientRect, HTCAPTION, HTCLIENT, WM_NCDESTROY, WM_NCHITTEST,
                WM_NCLBUTTONDBLCLK, WM_NCLBUTTONDOWN,
            },
        },
    };
    // A fixed dialog has just two hit regions. Use this message's coordinates even
    // when the mouse hasn't moved since the window opened.
    unsafe {
        match msg {
            WM_NCHITTEST => {
                let mut p = POINT {
                    x: lp as i16 as i32,
                    y: (lp >> 16) as i16 as i32,
                };
                let mut client = RECT::default();
                if ScreenToClient(hwnd, &mut p) != 0 && GetClientRect(hwnd, &mut client) != 0 {
                    let scale = GetDpiForWindow(hwnd) as f32 / 96.;
                    let height = (crate::chrome::TITLEBAR_HEIGHT * scale).round() as i32;
                    let close_width = (crate::chrome::WINDOW_BUTTON_WIDTH * scale).round() as i32;
                    if (0..height).contains(&p.y) && (0..client.right - close_width).contains(&p.x)
                    {
                        return HTCAPTION as isize;
                    }
                    return HTCLIENT as isize;
                }
            }
            WM_NCLBUTTONDOWN if wp == HTCAPTION as usize => {
                return DefWindowProcW(hwnd, msg, wp, lp);
            }
            WM_NCLBUTTONDBLCLK if wp == HTCAPTION as usize => return 0,
            WM_NCDESTROY => {
                RemoveWindowSubclass(hwnd, Some(dialog_frame), id);
            }
            _ => {}
        }
        DefSubclassProc(hwnd, msg, wp, lp)
    }
}

#[cfg(windows)]
fn attach(parent: &Window, child: &Window) {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GWL_EXSTYLE, GWLP_HWNDPARENT, GetWindowLongPtrW, SetWindowLongPtrW, WS_EX_APPWINDOW,
    };
    let hwnd = |w: &Window| match HasWindowHandle::window_handle(w).ok()?.as_raw() {
        RawWindowHandle::Win32(h) => Some(h.hwnd.get()),
        _ => None,
    };
    if let (Some(owner), Some(child)) = (hwnd(parent), hwnd(child)) {
        // Set an owner on the top-level dialog, so it moves through the Z order with its workspace.
        unsafe {
            let child = child as windows_sys::Win32::Foundation::HWND;
            SetWindowLongPtrW(child, GWLP_HWNDPARENT, owner);
            let style = GetWindowLongPtrW(child, GWL_EXSTYLE);
            SetWindowLongPtrW(child, GWL_EXSTYLE, style & !(WS_EX_APPWINDOW as isize));
            windows_sys::Win32::UI::Shell::SetWindowSubclass(child, Some(dialog_frame), 1, 0);
        }
    }
}

#[cfg(target_os = "macos")]
fn attach(parent: &Window, child: &Window) {
    use objc2_app_kit::{NSView, NSWindowOrderingMode};
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    let native = |w: &Window| {
        let RawWindowHandle::AppKit(h) = HasWindowHandle::window_handle(w).ok()?.as_raw() else {
            return None;
        };
        // GPUI's NSView belongs to this live window on the AppKit main thread.
        unsafe { h.ns_view.cast::<NSView>().as_ref() }.window()
    };
    if let (Some(owner), Some(child)) = (native(parent), native(child)) {
        unsafe {
            owner.addChildWindow_ordered(&child, NSWindowOrderingMode::Above);
        }
    }
}
