//! A controlled, single-value form control built on the same trigger and popup as action menus.
use super::*;
use std::rc::Rc;

type Change<T> = Rc<dyn Fn(&T, &mut Window, &mut App)>;

struct OptionRow<T> {
    value: T,
    label: SharedString,
    icon: Option<Icon>,
}

#[derive(IntoElement)]
pub struct Select<T: PartialEq + 'static> {
    id: ElementId,
    value: Option<T>,
    options: Vec<OptionRow<T>>,
    placeholder: SharedString,
    icon_trigger: Option<(Icon, SharedString)>,
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
            icon_trigger: None,
            change: Rc::new(change),
        }
    }

    pub fn option(mut self, value: T, label: impl Into<SharedString>) -> Self {
        self.options.push(OptionRow {
            value,
            label: label.into(),
            icon: None,
        });
        self
    }

    pub fn option_with_icon(
        mut self,
        value: T,
        label: impl Into<SharedString>,
        icon: Icon,
    ) -> Self {
        self.options.push(OptionRow {
            value,
            label: label.into(),
            icon: Some(icon),
        });
        self
    }

    pub fn placeholder(mut self, label: impl Into<SharedString>) -> Self {
        self.placeholder = label.into();
        self
    }

    pub fn icon_trigger(mut self, icon: Icon, tooltip: impl Into<SharedString>) -> Self {
        self.icon_trigger = Some((icon, tooltip.into()));
        self
    }
}

impl<T: PartialEq + 'static> RenderOnce for Select<T> {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let state = DropdownState::local(self.id.clone(), window, cx);
        let ui = Ui::new(window, cx);
        let field = self.icon_trigger.is_none();
        let trigger = if let Some((icon, tooltip)) = self.icon_trigger {
            ui.icon_button(ButtonVariant::Secondary, self.id, icon, tooltip)
        } else {
            let label = self
                .options
                .iter()
                .find(|option| Some(&option.value) == self.value.as_ref())
                .map(|option| option.label.clone())
                .unwrap_or(self.placeholder);
            ui.button(
                ButtonVariant::Outline,
                self.id,
                div().truncate().child(label),
            )
            .field()
            .disclosure(state.read(cx).is_open())
        };
        let mut content = DropdownContent::new();
        for (index, option) in self.options.into_iter().enumerate() {
            let change = self.change.clone();
            let checked = Some(&option.value) == self.value.as_ref();
            let label = div()
                .flex()
                .items_center()
                .gap(SPACE_2)
                .min_w_0()
                .when_some(option.icon, |row, icon| row.child(icon.draw(16.)))
                .child(div().truncate().child(option.label));
            content = content.checked_item(index, label, checked, move |window, cx| {
                change(&option.value, window, cx)
            });
        }
        let dropdown = Dropdown::new(&state, DropdownTrigger::new(trigger), content);
        if field { dropdown.field() } else { dropdown }
    }
}
