use super::*;
use crate::theme::{BUTTON_HEIGHT, PANEL_RADIUS};

pub(super) const MENU_ROW_HEIGHT: Pixels = px(34.);
pub(super) const MENU_PADDING: Pixels = px(5.);

impl Ui {
    pub fn input_frame(self, id: impl Into<ElementId>, focus: &FocusHandle) -> Stateful<Div> {
        let handle = focus.clone();
        let field = div()
            .id(id)
            .track_focus(focus)
            .key_context("TextField")
            .on_mouse_down_out(move |_, window, cx| {
                if handle.is_focused(window) {
                    window.blur(cx);
                }
            });
        self.text_field_frame(field)
            .cursor(CursorStyle::IBeam)
            .focus(move |s| s.border_color(rgb(self.colors.ring)))
    }

    fn text_field_frame<E: Styled>(self, field: E) -> E {
        field
            .w_full()
            .h(BUTTON_HEIGHT)
            .px(SPACE_2)
            .py(SPACE_1)
            .rounded(CONTROL_RADIUS)
            .border_1()
            .border_color(rgb(self.colors.input_border))
            .bg(rgb(self.colors.card))
    }

    pub fn popover(self, id: impl Into<ElementId>) -> Stateful<Div> {
        let panel = div().id(id).occlude().flex().flex_col().overflow_y_scroll();
        #[cfg(target_os = "android")]
        let panel = panel.restrict_scroll_to_axis();
        panel
            .p(MENU_PADDING)
            .rounded(PANEL_RADIUS)
            .border_1()
            .border_color(rgb(self.colors.border))
            .bg(rgb(self.colors.card))
            .text_color(rgb(self.colors.canvas.foreground))
            .shadow_lg()
            .text_role(TextRole::Body)
    }

    pub fn menu_item(
        self,
        id: impl Into<ElementId>,
        content: impl IntoElement,
        selected: bool,
    ) -> Stateful<Div> {
        div()
            .id(id)
            .w_full()
            .min_w_0()
            .overflow_hidden()
            .flex()
            .flex_none()
            .items_center()
            .h(MENU_ROW_HEIGHT)
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
