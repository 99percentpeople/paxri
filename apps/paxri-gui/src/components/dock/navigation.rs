//! The active page's navigation sits beside its content or below the vertical tab list.
use super::*;
use crate::components::{heading, row};

#[derive(IntoElement)]
pub(crate) struct DockNavigation {
    title: &'static str,
    body: AnyElement,
    action: Option<AnyElement>,
}

impl DockNavigation {
    pub(crate) fn new(title: &'static str, body: impl IntoElement) -> Self {
        Self {
            title,
            body: body.into_any_element(),
            action: None,
        }
    }

    pub(crate) fn action(mut self, action: impl IntoElement) -> Self {
        self.action = Some(action.into_any_element());
        self
    }
}

impl RenderOnce for DockNavigation {
    fn render(self, w: &mut Window, cx: &mut App) -> impl IntoElement {
        let ui = Ui::new(w, cx);
        div()
            .size_full()
            .min_w_0()
            .min_h_0()
            .flex()
            .flex_col()
            .bg(rgb(ui.colors.canvas.background))
            .p_3()
            .gap_2()
            .child(
                row()
                    .flex_none()
                    .child(heading(self.title))
                    .children(self.action),
            )
            .child(self.body)
    }
}
