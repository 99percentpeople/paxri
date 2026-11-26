//! A focus-scoped control for recording a hardware key combination.
use super::{ButtonVariant, DropdownContent, Icon, Ui, tooltip};
use crate::theme::{TextRole, TypographyExt};
use crate::terminal::keys::modifier_mask;
use gpui::{prelude::*, *};
use paxri_config::shortcuts::{Shortcut, modifier_labels};
use std::rc::Rc;

#[cfg(target_os = "android")]
mod touch;

type Change = Rc<dyn Fn(Option<Shortcut>, &mut Window, &mut App) -> Result<(), String>>;

struct Recording {
    focus: FocusHandle,
    interceptor: Option<Subscription>,
    waiting_for_release: Option<String>,
    modifiers: Modifiers,
    error: Option<String>,
    #[cfg(target_os = "android")]
    touch: Option<touch::Choice>,
}

impl Recording {
    fn is_listening(&self) -> bool {
        self.interceptor.is_some() && self.waiting_for_release.is_none()
    }

    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle().tab_stop(true);
        cx.on_blur(&focus, window, |state, _, cx| state.cancel(cx))
            .detach();
        cx.observe_window_activation(window, |state, window, cx| {
            if !window.is_window_active() {
                state.cancel(cx);
            }
        })
        .detach();
        Self {
            focus,
            interceptor: None,
            waiting_for_release: None,
            modifiers: Modifiers::default(),
            error: None,
            #[cfg(target_os = "android")]
            touch: None,
        }
    }

    fn cancel(&mut self, cx: &mut Context<Self>) {
        self.interceptor = None;
        self.waiting_for_release = None;
        self.modifiers = Modifiers::default();
        self.error = None;
        cx.notify();
    }

    fn begin(&mut self, change: Change, window: &mut Window, cx: &mut Context<Self>) {
        self.cancel(cx);
        #[cfg(target_os = "android")]
        {
            self.touch = None;
        }
        #[cfg(target_os = "android")]
        self.focus.focus(window, cx);
        #[cfg(not(target_os = "android"))]
        self.focus.focus(window);
        let state = cx.weak_entity();
        // Capture before keybindings such as Settings search or the platform paste action.
        self.interceptor = Some(cx.intercept_keystrokes(move |event, window, cx| {
            let _ = state.update(cx, |state, cx| {
                if !state.focus.is_focused(window) {
                    return;
                }
                let key = &event.keystroke;
                if let Some(accepted) = &state.waiting_for_release {
                    if accepted.eq_ignore_ascii_case(&key.key) {
                        window.prevent_default();
                        cx.stop_propagation();
                    } else {
                        state.cancel(cx);
                    }
                    return;
                }
                if key.key == "tab"
                    && (key.modifiers == Modifiers::default()
                        || key.modifiers == Modifiers::shift())
                {
                    state.cancel(cx);
                    return;
                }
                window.prevent_default();
                cx.stop_propagation();
                if key.key == "escape" && !key.modifiers.modified() {
                    state.cancel(cx);
                    return;
                }
                if matches!(
                    key.key.as_str(),
                    "shift" | "control" | "alt" | "platform" | "function"
                ) {
                    return;
                }
                let result = modifier_mask(key.modifiers)
                    .ok_or_else(|| "Choose a combination without Fn.".to_owned())
                    .and_then(|modifiers| Shortcut::from_keys(modifiers, &key.key))
                    .and_then(|chord| change(Some(chord), window, cx));
                match result {
                    Ok(()) => {
                        // Held keys repeat before key-up. Consume those repeats as part of this recording.
                        state.waiting_for_release = Some(key.key.clone());
                        state.error = None;
                        cx.notify();
                    }
                    Err(error) => {
                        state.error = Some(error);
                        cx.notify();
                    }
                }
            });
        }));
    }
}

#[derive(IntoElement)]
pub struct ShortcutRecorder {
    id: ElementId,
    label: SharedString,
    value: Shortcut,
    change: Change,
}

impl ShortcutRecorder {
    pub fn new(
        id: impl Into<ElementId>,
        label: impl Into<SharedString>,
        value: Shortcut,
        change: impl Fn(Option<Shortcut>, &mut Window, &mut App) -> Result<(), String> + 'static,
    ) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            value,
            change: Rc::new(change),
        }
    }
}

fn keycap(label: &str, ui: Ui) -> Div {
    div()
        .flex_none()
        .text_role(TextRole::Caption)
        .px_1()
        .rounded_sm()
        .border_1()
        .border_color(rgb(ui.colors.input_border))
        .bg(rgb(ui.colors.card))
        .child(label.to_owned())
}

impl RenderOnce for ShortcutRecorder {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let ui = Ui::new(window, cx);
        let state = window.use_keyed_state((self.id.clone(), "recording"), cx, Recording::new);
        let recording = state.read(cx).is_listening();
        let focus = state.read(cx).focus.clone();
        let held = modifier_mask(state.read(cx).modifiers).unwrap_or_default();
        let keys = div().flex().min_w_0().items_center().gap_1();
        let keys = if recording {
            keys.children(modifier_labels(held).map(|name| keycap(name, ui)))
                .child(ui.muted(if held == 0 { "Press keys…" } else { "…" }))
        } else if self.value.keys().next().is_some() {
            keys.children(self.value.keys().map(|key| keycap(key, ui)))
        } else {
            keys.child(ui.muted("Disabled"))
        };
        let tip: SharedString = format!("Record shortcut for {}", self.label).into();
        let face = div()
            .flex()
            .w_full()
            .items_center()
            .justify_between()
            .child(keys)
            .when(!recording, |face| {
                face.child(
                    div()
                        .text_color(rgb(ui.colors.muted_foreground))
                        .child(Icon::Pencil.draw(13.)),
                )
            });
        let button = ui
            .button(ButtonVariant::Outline, self.id.clone(), face)
            .field()
            .pressed(recording)
            .track_focus(&focus)
            .when(!recording, |button| button.tooltip(tooltip(tip.clone())))
            .on_key_up({
                let state = state.clone();
                move |event, window, cx| {
                    if state
                        .read(cx)
                        .waiting_for_release
                        .as_deref()
                        .is_some_and(|key| key.eq_ignore_ascii_case(&event.keystroke.key))
                    {
                        state.update(cx, Recording::cancel);
                        window.prevent_default();
                        cx.stop_propagation();
                    }
                }
            })
            .on_click({
                let state = state.clone();
                let change = self.change.clone();
                move |_, w, cx| {
                    state.update(cx, |s, cx| {
                        if s.is_listening() {
                            s.cancel(cx);
                        } else {
                            s.begin(change.clone(), w, cx);
                        }
                    })
                }
            })
            .on_mouse_down_out({
                let state = state.clone();
                move |_, _, cx| {
                    if state.read(cx).interceptor.is_some() {
                        state.update(cx, Recording::cancel);
                    }
                }
            })
            .on_modifiers_changed({
                let state = state.clone();
                move |event, _, cx| {
                    if state.read(cx).interceptor.is_some()
                        && state.read(cx).modifiers != event.modifiers
                    {
                        state.update(cx, |s, cx| {
                            s.modifiers = event.modifiers;
                            cx.notify();
                        });
                    }
                }
            });
        #[cfg(target_os = "android")]
        let button = button.role(Role::Button).aria_label(tip);
        let mut options = DropdownContent::new();
        #[cfg(target_os = "android")]
        {
            let state = state.clone();
            let choosing = state.read(cx).touch.is_some();
            let label = if choosing {
                "Cancel key selection"
            } else {
                "Choose keys on screen"
            };
            options = options.item(label, Icon::Pencil.label(label), move |_, cx| {
                state.update(cx, |s, cx| {
                    s.cancel(cx);
                    s.touch = (!choosing).then(touch::Choice::default);
                });
            });
        }
        for (title, icon, binding) in [
            ("Restore default", Icon::Refresh, None),
            ("Disable shortcut", Icon::Close, Some(Shortcut::disabled())),
        ] {
            let change = self.change.clone();
            let state = state.clone();
            options = options.item(title, icon.label(title), move |w, cx| {
                let result = change(binding.clone(), w, cx);
                state.update(cx, |s, cx| {
                    s.cancel(cx);
                    #[cfg(target_os = "android")]
                    {
                        s.touch = None;
                    }
                    s.error = result.err();
                });
            });
        }
        let actions = ui.actions_menu(
            (self.id.clone(), "actions"),
            format!("{} shortcut options", self.label).into(),
            options,
        );
        let body = div()
            .w(px(236.))
            .max_w_full()
            .flex()
            .flex_col()
            .gap_1()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .child(div().flex_1().min_w_0().child(button))
                    .child(actions),
            );
        #[cfg(target_os = "android")]
        let body = body.when_some(state.read(cx).touch, |b, choice| {
            b.child(touch::render(&self.id, &state, choice, self.change, ui))
        });
        body.when_some(state.read(cx).error.clone(), |d, error| {
            d.child(ui.alert(error))
        })
    }
}
