//! A controlled, single-value form control built on the same trigger and popup as action menus.
use super::*;
use std::rc::Rc;

type Change<T> = Rc<dyn Fn(&T, &mut Window, &mut App)>;

struct OptionRow<T> {
    value: T,
    content: Box<dyn Fn() -> Div>,
}

#[derive(IntoElement)]
pub struct Select<T: PartialEq + 'static> {
    id: ElementId,
    value: Option<T>,
    options: Vec<OptionRow<T>>,
    placeholder: SharedString,
    change: Change<T>,
}

impl<T: PartialEq + 'static> Select<T> {
    pub fn new(
        id: impl Into<ElementId>,
        value: impl Into<Option<T>>,
        change: impl Fn(&T, &mut Window, &mut App) + 'static,
    ) -> Self {
        Self {
            id: id.into(),
            value: value.into(),
            options: Vec::new(),
            placeholder: "Select…".into(),
            change: Rc::new(change),
        }
    }

    pub fn option(self, value: T, label: impl Into<SharedString>) -> Self {
        let label = label.into();
        self.option_with_content(value, move || {
            div().min_w_0().truncate().child(label.clone())
        })
    }

    pub fn option_with_icon(self, value: T, label: impl Into<SharedString>, icon: Icon) -> Self {
        let label = label.into();
        self.option_with_content(value, move || icon.label(label.clone()))
    }

    /// Render the same content in the trigger and its option row.
    pub fn option_with_content(mut self, value: T, content: impl Fn() -> Div + 'static) -> Self {
        self.options.push(OptionRow {
            value,
            content: Box::new(content),
        });
        self
    }

    pub(super) fn append_options(
        self,
        mut content: DropdownContent,
        first: usize,
    ) -> DropdownContent {
        for (index, option) in self.options.into_iter().enumerate() {
            let change = self.change.clone();
            let checked = Some(&option.value) == self.value.as_ref();
            content =
                content.checked_item(first + index, option.content, checked, move |window, cx| {
                    change(&option.value, window, cx)
                });
        }
        content
    }

    pub(super) fn selected_label(&self) -> Div {
        self.options
            .iter()
            .find(|option| Some(&option.value) == self.value.as_ref())
            .map(|option| (option.content)())
            .unwrap_or_else(|| div().min_w_0().truncate().child(self.placeholder.clone()))
    }

    pub fn placeholder(mut self, label: impl Into<SharedString>) -> Self {
        self.placeholder = label.into();
        self
    }
}

impl<T: PartialEq + 'static> RenderOnce for Select<T> {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let state = DropdownState::local(self.id.clone(), window, cx);
        let ui = Ui::new(window, cx);
        let label = self.selected_label();
        let trigger = ui
            .button(ButtonVariant::Outline, self.id.clone(), label)
            .field()
            .disclosure(state.read(cx).is_open());
        let content = self.append_options(DropdownContent::new(), 0);
        Dropdown::new(&state, DropdownTrigger::new(trigger), content).field()
    }
}
