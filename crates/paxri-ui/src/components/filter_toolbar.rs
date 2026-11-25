//! List search stays in place while structured filters open over the results.
use super::{input::Input, *};

#[derive(IntoElement)]
pub struct FilterToolbar {
    id: &'static str,
    label: &'static str,
    search: Entity<Input>,
    active: usize,
    fields: Vec<FilterField>,
    clear: Option<Box<dyn Fn(&mut Window, &mut App)>>,
    results: Option<(usize, usize)>,
}

struct FilterField {
    title: &'static str,
    value: Div,
    choices: DropdownContent,
}

fn filter_summary(active: usize, query: &str, shown: usize, total: usize) -> Option<String> {
    ((active > 0 || !query.trim().is_empty()) && total > 0).then(|| format!("{shown} of {total}"))
}

impl FilterToolbar {
    pub fn new(
        id: &'static str,
        label: &'static str,
        search: &Entity<Input>,
        active: usize,
    ) -> Self {
        Self {
            id,
            label,
            search: search.clone(),
            active,
            fields: Vec::new(),
            clear: None,
            results: None,
        }
    }

    pub fn results(mut self, shown: usize, total: usize) -> Self {
        self.results = Some((shown, total));
        self
    }

    pub fn group<T: PartialEq + 'static>(
        mut self,
        title: &'static str,
        choices: Select<T>,
    ) -> Self {
        self.fields.push(FilterField {
            title,
            value: choices.selected_label(),
            choices: choices.append_options(DropdownContent::new(), 0),
        });
        self
    }

    pub fn clear(mut self, reset: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        if self.active > 0 {
            self.clear = Some(Box::new(reset));
        }
        self
    }

    fn menu(&mut self, state: &Entity<DropdownState>, ui: Ui, cx: &mut App) -> DropdownContent {
        if let Some(index) = state.read(cx).parent_row() {
            if index < self.fields.len() {
                let field = self.fields.swap_remove(index);
                return field.choices.with_parent(field.title, state);
            }
            state.update(cx, DropdownState::back);
        }
        let mut menu = DropdownContent::new();
        for (index, field) in self.fields.drain(..).enumerate() {
            let row = div()
                .w_full()
                .min_w_0()
                .flex()
                .items_center()
                .gap_2()
                .child(div().flex_none().child(field.title))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .justify_end()
                        .overflow_hidden()
                        .text_color(rgb(ui.colors.muted_foreground))
                        .child(field.value),
                )
                .child(Icon::ChevronRight.draw(14.));
            let selected = field.choices.checked_index().map(|i| i + 1);
            let parent = state.clone();
            menu = menu.item(field.title, row, move |_, cx| {
                parent.update(cx, |menu, cx| menu.enter(index, selected, cx));
            });
        }
        if let Some(reset) = self.clear.take() {
            menu = menu.item("clear-filters", Icon::FilterX.label("Clear filters"), reset);
        }
        menu
    }
}

impl RenderOnce for FilterToolbar {
    fn render(mut self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let ui = Ui::new(window, cx);
        let state = DropdownState::local(self.id.into(), window, cx);
        let menu = self.menu(&state, ui, cx);
        let open = state.read(cx).is_open();
        let button = if self.active == 0 {
            ui.icon_button(ButtonVariant::Ghost, "filters", Icon::Filter, self.label)
        } else {
            ui.button(ButtonVariant::Secondary, "filters", self.active.to_string())
                .icon(Icon::Filter)
                .tooltip(tooltip(format!("{} ({})", self.label, self.active).into()))
        }
        .pressed(open);
        #[cfg(target_os = "android")]
        let button = button
            .role(Role::Button)
            .aria_label(self.label)
            .aria_expanded(open);
        let summary = self
            .results
            .and_then(|(shown, total)| filter_summary(self.active, self.search.read(cx).text(), shown, total));
        let bar = div()
            .w_full()
            .min_w_0()
            .flex_none()
            .flex()
            .items_center()
            .gap_1()
            .child(div().flex_1().min_w_0().child(self.search))
            .child(Dropdown::new(&state, DropdownTrigger::new(button), menu).filters());
        div()
            .id(self.id)
            .w_full()
            .min_w_0()
            .flex_none()
            .flex()
            .flex_col()
            .gap_1()
            .child(bar)
            .when_some(summary, |root, text| {
                root.child(ui.muted(text).text_xs().px_1())
            })
    }
}

#[cfg(test)]
mod tests {
    use super::filter_summary;

    #[test]
    fn filter_summary_only_appears_when_filtering_non_empty_lists() {
        assert_eq!(filter_summary(0, "", 3, 3), None);
        assert_eq!(filter_summary(0, "   ", 3, 3), None);
        assert_eq!(filter_summary(1, "", 0, 0), None);
        assert_eq!(filter_summary(0, "pi", 1, 3).as_deref(), Some("1 of 3"));
        assert_eq!(filter_summary(1, "", 0, 4).as_deref(), Some("0 of 4"));
        assert_eq!(filter_summary(2, "web", 2, 5).as_deref(), Some("2 of 5"));
    }
}
