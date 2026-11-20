use super::*;

impl Ui {
    pub fn navigation_row(
        self,
        id: impl Into<ElementId>,
        leading: impl IntoElement,
        title: SharedString,
        detail: impl Into<SharedString>,
    ) -> Button {
        let text = div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .gap(px(2.))
            .child(single_line(div()).child(title.clone()))
            .child(single_line(
                self.muted(detail).text_xs().font_weight(FontWeight::NORMAL),
            ));
        let content = div()
            .w_full()
            .min_w_0()
            .flex()
            .items_center()
            .py(SPACE_2)
            .gap(SPACE_3)
            .child(leading)
            .child(text)
            .child(
                div()
                    .flex_none()
                    .flex()
                    .text_color(rgb(self.colors.muted_foreground))
                    .child(Icon::ChevronRight.draw(14.)),
            );
        self.button(ButtonVariant::Ghost, id, content)
            .row()
            .tooltip(tooltip(title))
    }
}

/// A wrapping list row with a title, supporting content and optional controls.
#[derive(IntoElement)]
pub struct ListItem {
    body: Div,
    leading: Option<AnyElement>,
    trailing: Option<AnyElement>,
}

impl ListItem {
    pub fn new(title: impl IntoElement) -> Self {
        Self {
            body: div().flex().flex_col().child(title),
            leading: None,
            trailing: None,
        }
    }

    pub fn leading(mut self, leading: impl IntoElement) -> Self {
        self.leading = Some(leading.into_any_element());
        self
    }

    pub fn detail(mut self, detail: impl IntoElement) -> Self {
        self.body = self.body.child(detail);
        self
    }

    pub fn trailing(mut self, trailing: impl IntoElement) -> Self {
        self.trailing = Some(trailing.into_any_element());
        self
    }
}

impl RenderOnce for ListItem {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let body = match self.leading {
            Some(leading) => div()
                .flex()
                .items_center()
                .gap(SPACE_3)
                .child(leading)
                .child(self.body),
            None => self.body,
        };
        row().child(body).children(self.trailing)
    }
}
