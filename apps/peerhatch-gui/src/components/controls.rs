use super::*;

impl Ui {
    pub fn form_actions(
        self,
        form: &'static str,
        label: impl IntoElement,
        submit: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
        cancel: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Div {
        actions()
            .child(
                self.button(
                    ButtonVariant::Primary,
                    SharedString::from(format!("save-{form}")),
                    label,
                )
                .on_click(submit),
            )
            .child(
                self.button(
                    ButtonVariant::Secondary,
                    SharedString::from(format!("cancel-{form}")),
                    "Cancel",
                )
                .on_click(cancel),
            )
    }

    pub fn input_frame(self, id: impl Into<ElementId>, focus: &FocusHandle) -> Stateful<Div> {
        let colors = self.colors;
        focus::text_field(div().id(id), focus)
            .w_full()
            .h(px(36.))
            .px(SPACE_2)
            .py(SPACE_1)
            .rounded(CONTROL_RADIUS)
            .border_1()
            .border_color(rgb(colors.input_border))
            .bg(rgb(colors.card))
            .cursor(CursorStyle::IBeam)
            .focus(move |s| s.border_color(rgb(colors.ring)))
    }

    pub fn popover(self, id: impl Into<ElementId>) -> Stateful<Div> {
        div()
            .id(id)
            .occlude()
            .flex()
            .flex_col()
            .overflow_y_scroll()
            .p(px(5.))
            .rounded(PANEL_RADIUS)
            .border_1()
            .border_color(rgb(self.colors.border))
            .bg(rgb(self.colors.card))
            .text_color(rgb(self.colors.canvas.foreground))
            .shadow_lg()
            .text_sm()
    }

    pub fn menu_item(
        self,
        id: impl Into<ElementId>,
        content: impl IntoElement,
        selected: bool,
    ) -> Stateful<Div> {
        div()
            .id(id)
            .flex()
            .flex_none()
            .items_center()
            .w_full()
            .h(px(34.))
            .px(SPACE_2)
            .rounded(CONTROL_RADIUS)
            .cursor_pointer()
            .when(selected, |row| {
                row.bg(rgb(self.colors.accent.background))
                    .text_color(rgb(self.colors.accent.foreground))
            })
            .child(content)
    }
}

impl Ui {
    pub fn switch(self, id: &'static str, on: bool) -> Button {
        let colors = self.colors;
        let track = div()
            .w(px(30.))
            .h(px(18.))
            .px(px(2.))
            .rounded_full()
            .flex()
            .items_center()
            .when(on, |d| d.justify_end())
            .bg(rgb(if on {
                colors.primary.background
            } else {
                colors.border
            }))
            .child(div().size(px(14.)).rounded_full().bg(rgb(if on {
                colors.primary.foreground
            } else {
                colors.card
            })));
        let label = div()
            .flex()
            .items_center()
            .gap(SPACE_2)
            .child(if on { "On" } else { "Off" })
            .child(track);
        self.button(ButtonVariant::Ghost, id, label)
    }
}
