use super::*;
use std::time::Duration;

/// Feedback for an entire list. A partial failure can also sit above usable rows.
pub struct ListState {
    icon: Icon,
    title: SharedString,
    detail: Option<SharedString>,
    loading: bool,
}

impl ListState {
    pub fn empty(icon: Icon, title: impl Into<SharedString>) -> Self {
        Self {
            icon,
            title: title.into(),
            detail: None,
            loading: false,
        }
    }

    pub fn loading(title: &'static str) -> Self {
        Self {
            loading: true,
            ..Self::empty(Icon::Refresh, title)
        }
    }

    pub fn filtered(title: &'static str) -> Self {
        Self::empty(Icon::Search, title).detail("Try changing your search or filters.")
    }

    pub fn detail(mut self, text: impl Into<SharedString>) -> Self {
        self.detail = Some(text.into());
        self
    }

    pub fn render(self, ui: Ui) -> Div {
        let icon = ui
            .icon_tile(self.icon, 40., 20.)
            .text_color(rgb(ui.colors.muted_foreground));
        let icon = if self.loading {
            icon.with_animation(
                "list-loading",
                Animation::new(Duration::from_millis(1200)).repeat(),
                |icon, t| icon.opacity(0.8 + 0.2 * (2. * t - 1.).abs()),
            )
            .into_any_element()
        } else {
            icon.into_any_element()
        };
        div()
            .w_full()
            .min_w_0()
            .min_h(px(156.))
            .flex_1()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .text_center()
            .px_4()
            .py_8()
            .gap_2()
            .child(icon)
            .child(ui.muted(self.title).font_weight(FontWeight::MEDIUM))
            .when_some(self.detail, |body, detail| {
                body.child(ui.muted(detail).max_w(px(360.)))
            })
    }
}
