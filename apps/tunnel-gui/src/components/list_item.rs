use super::*;

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
