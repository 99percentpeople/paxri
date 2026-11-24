use super::*;

/// Standard slots for a row. The body can also hold columns for a wide list.
#[derive(IntoElement)]
pub struct ListItem {
    body: Div,
    detail: Option<SharedString>,
    detail_icon: Option<AnyElement>,
    leading: Option<AnyElement>,
    trailing: Option<AnyElement>,
}

pub(super) fn list_title(text: impl Into<SharedString>) -> Div {
    let title = single_line(div())
        .font_weight(FontWeight::SEMIBOLD)
        .child(text.into());
    // Desktop GPUI can measure ellipsized text at zero width inside a flex column.
    #[cfg(target_os = "android")]
    let title = title.text_ellipsis();
    title
}

impl ListItem {
    pub fn new(title: impl Into<SharedString>) -> Self {
        Self::custom(list_title(title))
    }

    pub(super) fn custom(content: impl IntoElement) -> Self {
        Self {
            body: div()
                .min_w_0()
                .flex_1()
                .flex()
                .flex_col()
                .gap(px(2.))
                .child(content),
            detail: None,
            detail_icon: None,
            leading: None,
            trailing: None,
        }
    }

    pub fn leading(mut self, leading: impl IntoElement) -> Self {
        self.leading = Some(leading.into_any_element());
        self
    }

    pub fn detail(mut self, text: impl Into<SharedString>) -> Self {
        self.detail = Some(text.into());
        self
    }

    pub fn detail_icon(mut self, icon: impl IntoElement) -> Self {
        self.detail_icon = Some(icon.into_any_element());
        self
    }

    pub fn trailing(mut self, trailing: impl IntoElement) -> Self {
        self.trailing = Some(trailing.into_any_element());
        self
    }

    pub fn chevron(self, ui: Ui) -> Self {
        self.trailing(
            div()
                .flex()
                .text_color(rgb(ui.colors.muted_foreground))
                .child(Icon::ChevronRight.draw(14.)),
        )
    }

    pub fn button(mut self, ui: Ui, id: impl Into<ElementId>, label: SharedString) -> Button {
        self.body = single_line(self.body);
        #[cfg(target_os = "android")]
        {
            self.body = self.body.text_ellipsis();
        }
        let button = ui
            .button(ButtonVariant::Ghost, id, self)
            .row()
            .tooltip(tooltip(label.clone()));
        #[cfg(target_os = "android")]
        let button = button.role(Role::Button).aria_label(label);
        button
    }
}

impl RenderOnce for ListItem {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let slot = |item| div().flex_none().flex().items_center().child(item);
        let body = self.body.when_some(self.detail, |body, text| {
            let detail = Ui::new(window, cx)
                .muted(text)
                .min_w_0()
                .text_xs()
                .font_weight(FontWeight::NORMAL);
            let detail = match self.detail_icon {
                Some(icon) => div()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .gap_1()
                    .child(slot(icon))
                    .child(detail),
                None => detail,
            };
            body.child(detail)
        });
        div()
            .w_full()
            .min_w_0()
            .flex_none()
            .flex()
            .items_center()
            .gap(SPACE_2)
            .py(SPACE_2)
            .children(self.leading.map(slot))
            .child(body)
            .children(self.trailing.map(slot))
    }
}

impl Ui {
    pub fn navigation_row(
        self,
        id: impl Into<ElementId>,
        leading: impl IntoElement,
        title: SharedString,
        detail: impl Into<SharedString>,
    ) -> Button {
        ListItem::new(title.clone())
            .detail(detail)
            .leading(leading)
            .chevron(self)
            .button(self, id, title)
    }
}
