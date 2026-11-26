//! In-window modal overlay matching the active GPUI theme palette.
use super::*;
use crate::theme::PANEL_RADIUS;
use std::rc::Rc;

type ModalAction = Rc<dyn Fn(&mut Window, &mut App)>;

#[derive(IntoElement)]
pub struct ModalDialog {
    id: ElementId,
    title: SharedString,
    body: AnyElement,
    cancel_label: &'static str,
    confirm_label: &'static str,
    confirm_variant: ButtonVariant,
    secondary_action: Option<(&'static str, ModalAction)>,
    on_confirm: ModalAction,
    on_cancel: ModalAction,
}

impl ModalDialog {
    pub fn new(
        id: impl Into<ElementId>,
        title: impl Into<SharedString>,
        body: impl IntoElement,
        confirm_label: &'static str,
        on_confirm: impl Fn(&mut Window, &mut App) + 'static,
        on_cancel: impl Fn(&mut Window, &mut App) + 'static,
    ) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            body: body.into_any_element(),
            cancel_label: "Cancel",
            confirm_label,
            confirm_variant: ButtonVariant::Primary,
            secondary_action: None,
            on_confirm: Rc::new(on_confirm),
            on_cancel: Rc::new(on_cancel),
        }
    }

    pub fn cancel_label(mut self, label: &'static str) -> Self {
        self.cancel_label = label;
        self
    }

    pub fn confirm_variant(mut self, variant: ButtonVariant) -> Self {
        self.confirm_variant = variant;
        self
    }

    pub fn secondary_action(
        mut self,
        label: &'static str,
        on_action: impl Fn(&mut Window, &mut App) + 'static,
    ) -> Self {
        self.secondary_action = Some((label, Rc::new(on_action)));
        self
    }
}

impl RenderOnce for ModalDialog {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let ui = Ui::new(window, cx);
        let dismiss_scrim = self.on_cancel.clone();
        let dismiss_button = self.on_cancel.clone();
        let dismiss_key = self.on_cancel;
        let confirm = self.on_confirm;

        let cancel_btn = ui
            .button(ButtonVariant::Secondary, "modal-cancel", self.cancel_label)
            .on_click(move |_, w, cx| dismiss_button(w, cx));
        #[cfg(target_os = "android")]
        let cancel_btn = cancel_btn.role(Role::Button).aria_label(self.cancel_label);

        let secondary_btn = self.secondary_action.map(|(label, action)| {
            let btn = ui
                .button(ButtonVariant::Secondary, "modal-secondary", label)
                .on_click(move |_, w, cx| action(w, cx));
            #[cfg(target_os = "android")]
            let btn = btn.role(Role::Button).aria_label(label);
            btn
        });

        let confirm_btn = ui
            .button(self.confirm_variant, "modal-confirm", self.confirm_label)
            .on_click(move |_, w, cx| confirm(w, cx));
        #[cfg(target_os = "android")]
        let confirm_btn = confirm_btn.role(Role::Button).aria_label(self.confirm_label);

        let card = div()
            .id(self.id)
            .occlude()
            .w_full()
            .max_w(px(360.))
            .max_h_full()
            .flex()
            .flex_col()
            .overflow_hidden()
            .rounded(PANEL_RADIUS)
            .border_1()
            .border_color(rgb(ui.colors.border))
            .bg(rgb(ui.colors.canvas.background))
            .text_color(rgb(ui.colors.canvas.foreground))
            .shadow_lg()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_key_down(move |event, w, cx| {
                if event.keystroke.key == "escape" && !event.keystroke.modifiers.modified() {
                    cx.stop_propagation();
                    dismiss_key(w, cx);
                }
            })
            .child(
                div()
                    .flex_none()
                    .px_4()
                    .pt_4()
                    .pb_2()
                    .text_role(TextRole::Section)
                    .child(self.title),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .px_4()
                    .pb_4()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .text_role(TextRole::Body)
                    .child(self.body),
            )
            .child(
                div()
                    .flex_none()
                    .flex()
                    .flex_wrap()
                    .justify_end()
                    .gap_2()
                    .px_4()
                    .py_3()
                    .border_t_1()
                    .border_color(rgb(ui.colors.border))
                    .child(cancel_btn)
                    .children(secondary_btn)
                    .child(confirm_btn),
            );

        div()
            .id("modal-overlay")
            .absolute()
            .inset_0()
            .occlude()
            .bg(rgba(ui.colors.modal_overlay))
            .flex()
            .items_center()
            .justify_center()
            .p_4()
            .on_mouse_down(MouseButton::Left, move |_, w, cx| {
                cx.stop_propagation();
                dismiss_scrim(w, cx);
            })
            .child(card)
    }
}
