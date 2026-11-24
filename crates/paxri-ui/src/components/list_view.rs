use super::*;

/// A list may own its scroll area or live inside an already scrolling page.
#[derive(Default, IntoElement)]
pub struct ListView {
    items: Vec<AnyElement>,
    scroll_id: Option<ElementId>,
    dividers: bool,
    floating_action: bool,
}

impl ListView {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn scrollable(id: impl Into<ElementId>) -> Self {
        Self {
            scroll_id: Some(id.into()),
            ..Self::default()
        }
    }

    pub fn divided(mut self) -> Self {
        self.dividers = true;
        self
    }

    pub fn with_floating_action(mut self, present: bool) -> Self {
        self.floating_action = present;
        self
    }
}

impl ParentElement for ListView {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.items.extend(elements);
    }
}

impl RenderOnce for ListView {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let ui = Ui::new(window, cx);
        let mut list = div()
            .w_full()
            .min_w_0()
            .min_h_0()
            .flex()
            .flex_col()
            .gap(SPACE_1)
            .when(self.floating_action, |list| {
                list.pb(FLOATING_ACTION_CLEARANCE)
            });
        for (index, item) in self.items.into_iter().enumerate() {
            if self.dividers && index > 0 {
                list = list.child(ui.separator());
            }
            list = list.child(item);
        }
        if let Some(id) = self.scroll_id {
            let list = list.id(id).flex_1().overflow_y_scroll();
            #[cfg(target_os = "android")]
            let list = list.restrict_scroll_to_axis();
            list.into_any_element()
        } else {
            list.flex_none().into_any_element()
        }
    }
}
