//! The trigger owns the focus target. The content owns the selectable rows and popup.
use super::*;
use std::rc::Rc;

type SelectAction = Rc<dyn Fn(&mut Window, &mut App)>;

pub struct DropdownState {
    focus: FocusHandle,
    highlighted: Option<usize>,
    scroll: ScrollHandle,
    trigger_width: Pixels,
}

impl DropdownState {
    pub fn new<V: 'static>(window: &mut Window, cx: &mut Context<V>) -> Entity<Self> {
        let state = cx.new(|cx| Self::init(window, cx));
        cx.observe(&state, |_, _, cx| cx.notify()).detach();
        state
    }

    pub(super) fn local(id: ElementId, window: &mut Window, cx: &mut App) -> Entity<Self> {
        window.use_keyed_state(id, cx, Self::init)
    }

    fn init(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle().tab_stop(true);
        cx.on_blur(&focus, window, |menu, _, cx| menu.dismiss(cx))
            .detach();
        cx.observe_window_activation(window, |menu, window, cx| {
            if !window.is_window_active() {
                menu.dismiss(cx);
            }
        })
        .detach();
        Self {
            focus,
            highlighted: None,
            scroll: ScrollHandle::new(),
            trigger_width: px(0.),
        }
    }

    pub fn is_open(&self) -> bool {
        self.highlighted.is_some()
    }

    pub fn dismiss(&mut self, cx: &mut Context<Self>) {
        self.highlight(None, cx);
    }

    fn highlight(&mut self, item: Option<usize>, cx: &mut Context<Self>) {
        if self.highlighted != item {
            self.highlighted = item;
            cx.notify();
        }
    }

    fn key(
        &mut self,
        event: &KeyDownEvent,
        count: usize,
        checked: Option<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<usize> {
        let key = &event.keystroke;
        if key.modifiers.modified() && !(key.key == "tab" && key.modifiers == Modifiers::shift()) {
            return None;
        }
        let action = navigation(
            self.highlighted,
            checked,
            count,
            key.key.as_str(),
            event.is_held,
        )?;
        let mut activate = None;
        match action {
            MenuAction::Highlight(index) => {
                self.highlight(Some(index), cx);
                self.scroll.scroll_to_item(index);
            }
            MenuAction::Dismiss => self.dismiss(cx),
            MenuAction::Activate(index) => {
                self.dismiss(cx);
                activate = Some(index);
            }
            MenuAction::Traverse => {
                self.dismiss(cx);
                if key.modifiers.shift {
                    window.focus_prev()
                } else {
                    window.focus_next()
                }
            }
        }
        window.prevent_default();
        cx.stop_propagation();
        activate
    }
}

#[derive(Debug, PartialEq, Eq)]
enum MenuAction {
    Highlight(usize),
    Activate(usize),
    Dismiss,
    Traverse,
}

fn navigation(
    current: Option<usize>,
    checked: Option<usize>,
    count: usize,
    key: &str,
    held: bool,
) -> Option<MenuAction> {
    use MenuAction::*;
    if key == "tab" {
        return Some(Traverse);
    }
    let last = count.checked_sub(1)?;
    let current = current.map(|i| i.min(last));
    Some(match (current, key) {
        (None, "down" | "enter" | "space") if !held => Highlight(checked.unwrap_or(0)),
        (None, "up") => Highlight(checked.unwrap_or(last)),
        (Some(_), "home") => Highlight(0),
        (Some(_), "end") => Highlight(last),
        (Some(_), "escape") => Dismiss,
        (Some(i), "down") => Highlight((i + 1) % count),
        (Some(i), "up") => Highlight(if i == 0 { last } else { i - 1 }),
        (Some(i), "enter" | "space") if !held => Activate(i),
        _ => return None,
    })
}

impl Focusable for DropdownState {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

pub struct DropdownTrigger<T>(T);

impl<T: InteractiveElement + IntoElement> DropdownTrigger<T> {
    pub fn new(child: T) -> Self {
        Self(child)
    }

    fn bind(
        self,
        state: Entity<DropdownState>,
        actions: Vec<SelectAction>,
        checked: Option<usize>,
        cx: &App,
    ) -> T {
        let open = state.read(cx).is_open();
        if actions.is_empty() {
            return self.0.tab_stop(false);
        }
        let key_state = state.clone();
        self.0
            .track_focus(&state.read(cx).focus)
            .on_key_down(move |event, window, cx| {
                let selected = key_state.update(cx, |menu, cx| {
                    menu.key(event, actions.len(), checked, window, cx)
                });
                if let Some(index) = selected {
                    actions[index](window, cx);
                }
            })
            .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                state.update(cx, |menu, cx| {
                    // Outside dismissal may already have run on this same press.
                    let index = (!open).then_some(checked.unwrap_or(0));
                    menu.highlight(index, cx);
                    if let Some(i) = index {
                        menu.scroll.scroll_to_item(i);
                    }
                    menu.focus.focus(window);
                });
            })
    }
}

struct MenuItem {
    id: ElementId,
    label: AnyElement,
    on_select: SelectAction,
}

#[derive(Default)]
pub struct DropdownContent {
    items: Vec<MenuItem>,
    checked: Option<usize>,
}

impl DropdownContent {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn item(
        mut self,
        id: impl Into<ElementId>,
        label: impl IntoElement,
        on_select: impl Fn(&mut Window, &mut App) + 'static,
    ) -> Self {
        self.items.push(MenuItem {
            id: id.into(),
            label: label.into_any_element(),
            on_select: Rc::new(on_select),
        });
        self
    }

    /// A checked row represents the committed value independently of keyboard or pointer focus.
    pub fn checked_item(
        mut self,
        id: impl Into<ElementId>,
        label: impl IntoElement,
        checked: bool,
        on_select: impl Fn(&mut Window, &mut App) + 'static,
    ) -> Self {
        if checked {
            self.checked = Some(self.items.len());
        }
        let row = div()
            .w_full()
            .flex()
            .items_center()
            .justify_between()
            .gap(SPACE_2)
            .child(label)
            .when(checked, |row| row.child(Icon::Check.draw(14.)));
        self.item(id, row, on_select)
    }

    fn popup(
        self,
        state: &Entity<DropdownState>,
        highlighted: usize,
        align_start: bool,
        window: &Window,
        cx: &App,
    ) -> Deferred {
        let ui = Ui::new(window, cx);
        let viewport = window.viewport_size();
        let outside = state.clone();
        let width = if align_start {
            state.read(cx).trigger_width
        } else {
            px(234.)
        };
        let mut list = ui
            .popover("content")
            .track_scroll(&state.read(cx).scroll)
            .w(width.min(viewport.width - px(12.)).max(px(1.)))
            .max_h((viewport.height - px(48.)).max(px(32.)))
            .on_mouse_down_out(move |_, _, cx| outside.update(cx, DropdownState::dismiss));
        for (index, item) in self.items.into_iter().enumerate() {
            let hover = state.clone();
            let select = state.clone();
            list = list.child(
                ui.menu_item(item.id, item.label, index == highlighted)
                    .on_mouse_move(move |_, _, cx| {
                        hover.update(cx, |menu, cx| {
                            if menu.is_open() {
                                menu.highlight(Some(index), cx);
                            }
                        });
                    })
                    .on_click(move |event, window, cx| {
                        if event.standard_click() {
                            select.update(cx, DropdownState::dismiss);
                            (item.on_select)(window, cx);
                        }
                    }),
            );
        }
        deferred(
            anchored()
                .anchor(if align_start {
                    Corner::TopLeft
                } else {
                    Corner::TopRight
                })
                .offset(point(px(0.), SPACE_1))
                .snap_to_window_with_margin(px(6.))
                .child(list),
        )
    }
}

#[derive(IntoElement)]
pub struct Dropdown<T: InteractiveElement + IntoElement + 'static> {
    state: Entity<DropdownState>,
    trigger: DropdownTrigger<T>,
    content: DropdownContent,
    field: bool,
}

impl<T: InteractiveElement + IntoElement> Dropdown<T> {
    pub fn new(
        state: &Entity<DropdownState>,
        trigger: DropdownTrigger<T>,
        content: DropdownContent,
    ) -> Self {
        Self {
            state: state.clone(),
            trigger,
            content,
            field: false,
        }
    }

    /// Fill the form column and align the popup with the leading edge of the trigger.
    pub fn field(mut self) -> Self {
        self.field = true;
        self
    }
}

impl<T: InteractiveElement + IntoElement> RenderOnce for Dropdown<T> {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let Self {
            state,
            trigger,
            content,
            field,
        } = self;
        let count = content.items.len();
        if count == 0 && state.read(cx).is_open() {
            state.update(cx, DropdownState::dismiss);
        }
        let selected = state
            .read(cx)
            .highlighted
            .zip(count.checked_sub(1))
            .map(|(i, last)| i.min(last));
        let actions = content.items.iter().map(|i| i.on_select.clone()).collect();
        let measured = state.clone();
        div()
            .when(field, |root| {
                root.on_children_prepainted(move |children, _, cx| {
                    if let Some(trigger) = children.first() {
                        measured.update(cx, |menu, cx| {
                            if menu.trigger_width != trigger.size.width {
                                menu.trigger_width = trigger.size.width;
                                if menu.is_open() {
                                    cx.notify();
                                }
                            }
                        });
                    }
                })
            })
            .id(("dropdown", state.entity_id()))
            .relative()
            .flex_none()
            .flex()
            .items_center()
            .when(field, |root| root.w_full())
            .when(count == 0, |root| root.opacity(0.5))
            .child(trigger.bind(state.clone(), actions, content.checked, cx))
            .when_some(selected, |root, i| {
                root.child(
                    div()
                        .absolute()
                        .bottom_0()
                        .when(field, |anchor| anchor.left_0())
                        .when(!field, |anchor| anchor.right_0())
                        .size(px(0.))
                        .child(content.popup(&state, i, field, window, cx)),
                )
            })
    }
}

#[cfg(test)]
mod form_controls {
    use super::{MenuAction::*, navigation};

    #[test]
    fn select_opens_at_the_committed_value_and_only_activates_on_confirmation() {
        assert_eq!(
            navigation(None, Some(1), 3, "down", false),
            Some(Highlight(1))
        );
        assert_eq!(
            navigation(Some(1), Some(1), 3, "down", false),
            Some(Highlight(2))
        );
        assert_eq!(
            navigation(Some(2), Some(1), 3, "escape", false),
            Some(Dismiss)
        );
        assert_eq!(
            navigation(None, Some(1), 3, "up", false),
            Some(Highlight(1))
        );
        assert_eq!(
            navigation(Some(2), Some(1), 3, "enter", false),
            Some(Activate(2))
        );
        assert_eq!(navigation(Some(2), Some(1), 3, "enter", true), None);
    }

    #[test]
    fn action_menus_wrap_and_empty_or_shortened_lists_are_safe() {
        assert_eq!(navigation(None, None, 3, "up", false), Some(Highlight(2)));
        assert_eq!(
            navigation(Some(2), None, 3, "down", false),
            Some(Highlight(0))
        );
        assert_eq!(
            navigation(Some(0), None, 3, "up", false),
            Some(Highlight(2))
        );
        assert_eq!(
            navigation(Some(5), None, 2, "enter", false),
            Some(Activate(1))
        );
        assert_eq!(navigation(None, None, 0, "enter", false), None);
        assert_eq!(navigation(None, None, 0, "tab", false), Some(Traverse));
    }
}
