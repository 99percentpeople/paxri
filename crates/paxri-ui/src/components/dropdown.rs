//! The trigger owns the focus target. The content owns the selectable rows and popup.
use super::{
    controls::{MENU_PADDING, MENU_ROW_HEIGHT},
    *,
};
use std::rc::Rc;
#[cfg(target_os = "android")]
use gpui::Anchor as Corner;

type SelectAction = Rc<dyn Fn(&mut Window, &mut App)>;

pub struct DropdownState {
    focus: FocusHandle,
    highlighted: Option<usize>,
    parent_row: Option<usize>,
    scroll: ScrollHandle,
    trigger_bounds: Option<Bounds<Pixels>>,
    opening_index: usize,
    expansion: Pixels,
}

impl DropdownState {
    pub fn new<V: 'static>(window: &mut Window, cx: &mut Context<V>) -> Entity<Self> {
        let state = cx.new(|cx| Self::init(window, cx));
        cx.observe(&state, |_, _, cx| cx.notify()).detach();
        state
    }

    pub fn local(id: ElementId, window: &mut Window, cx: &mut App) -> Entity<Self> {
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
            parent_row: None,
            scroll: ScrollHandle::new(),
            trigger_bounds: None,
            opening_index: 0,
            expansion: px(0.),
        }
    }

    pub fn is_open(&self) -> bool {
        self.highlighted.is_some()
    }

    pub fn dismiss(&mut self, cx: &mut Context<Self>) {
        self.parent_row = None;
        self.highlight(None, cx);
    }

    pub(super) fn parent_row(&self) -> Option<usize> {
        self.parent_row
    }

    pub(super) fn enter(&mut self, parent: usize, selected: usize, cx: &mut Context<Self>) {
        self.parent_row = Some(parent);
        self.show_row(selected, cx);
    }

    pub fn back(&mut self, cx: &mut Context<Self>) {
        if let Some(parent) = self.parent_row.take() {
            self.show_row(parent, cx);
        } else {
            self.dismiss(cx);
        }
    }

    fn show_row(&mut self, index: usize, cx: &mut Context<Self>) {
        self.scroll.set_offset(Point::default());
        self.scroll.scroll_to_item(index);
        self.highlighted = Some(index);
        cx.notify();
    }

    fn highlight(&mut self, item: Option<usize>, cx: &mut Context<Self>) {
        if self.highlighted != item {
            self.highlighted = item;
            cx.notify();
        }
    }

    fn focus_item(&mut self, index: usize, count: usize, window: &Window, cx: &mut Context<Self>) {
        let opening = !self.is_open();
        if opening {
            self.opening_index = index;
            self.expansion = px(0.);
        }
        if let Some(trigger) = self.trigger_bounds {
            let placement = self.field_placement(trigger, count, window);
            if opening {
                self.scroll
                    .set_offset(point(px(0.), -placement.opening_scroll));
            } else {
                let offset = -self.scroll.offset().y;
                let start = MENU_ROW_HEIGHT * index as f32;
                let end = start + MENU_ROW_HEIGHT + select_inset() * 2.;
                let distance = if start < offset {
                    start - offset
                } else {
                    (end - offset - placement.height).max(px(0.))
                };
                self.scroll_field(&placement, distance, cx);
            }
        } else {
            self.scroll.scroll_to_item(index);
        }
        self.highlight(Some(index), cx);
    }

    /// Native Android views above the GPUI surface must leave room for an open field menu.
    #[cfg(target_os = "android")]
    pub fn field_popup_top(&self, count: usize, window: &Window) -> Option<Pixels> {
        self.trigger_bounds.filter(|_| self.is_open())
            .map(|trigger| self.field_placement(trigger, count, window).top)
    }

    fn field_placement(
        &self,
        trigger: Bounds<Pixels>,
        count: usize,
        window: &Window,
    ) -> SelectPlacement {
        SelectPlacement::new(
            trigger.center().y,
            self.opening_index,
            count,
            window.viewport_size().height,
            popup_margin(window),
        )
        .expanded(self.expansion)
    }

    fn scroll_field(
        &mut self,
        placement: &SelectPlacement,
        distance: Pixels,
        cx: &mut Context<Self>,
    ) {
        let offset = -self.scroll.offset().y;
        let (growth, next_offset) = placement.scroll_by(offset, distance);
        if growth > px(0.) || next_offset != offset {
            self.expansion += growth;
            self.scroll.set_offset(point(px(0.), -next_offset));
            cx.notify();
        }
    }

    fn key(
        &mut self,
        event: &KeyDownEvent,
        count: usize,
        checked: Option<usize>,
        keep_open: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<usize> {
        let key = &event.keystroke;
        if key.modifiers.modified() && !(key.key == "tab" && key.modifiers == Modifiers::shift()) {
            return None;
        }
        let key_name = if self.parent_row.is_some() && key.key == "left" {
            "escape"
        } else {
            key.key.as_str()
        };
        let action = navigation(
            self.highlighted,
            checked,
            count,
            key_name,
            event.is_held,
        )?;
        let mut activate = None;
        match action {
            MenuAction::Highlight(index) => {
                self.focus_item(index, count, window, cx);
            }
            MenuAction::Dismiss => self.back(cx),
            MenuAction::Activate(index) => {
                if !keep_open {
                    self.dismiss(cx);
                }
                activate = Some(index);
            }
            MenuAction::Traverse => {
                self.dismiss(cx);
                #[cfg(target_os = "android")]
                if key.modifiers.shift {
                    window.focus_prev(cx)
                } else {
                    window.focus_next(cx)
                }
                #[cfg(not(target_os = "android"))]
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
        keep_open: bool,
        cx: &App,
    ) -> T {
        let open = state.read(cx).is_open();
        if actions.is_empty() {
            return self.0.tab_stop(false);
        }
        let count = actions.len();
        let key_state = state.clone();
        self.0
            .track_focus(&state.read(cx).focus)
            .on_key_down(move |event, window, cx| {
                let selected = key_state.update(cx, |menu, cx| {
                    menu.key(event, actions.len(), checked, keep_open, window, cx)
                });
                if let Some(index) = selected {
                    actions[index](window, cx);
                }
            })
            .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                state.update(cx, |menu, cx| {
                    // Outside dismissal may already have run on this same press.
                    if open {
                        menu.dismiss(cx);
                    } else {
                        menu.focus_item(checked.unwrap_or(0), count, window, cx);
                    }
                    #[cfg(target_os = "android")]
                    menu.focus.focus(window, cx);
                    #[cfg(not(target_os = "android"))]
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

// The collapsed field only needs its selected label. Construct the other option
// elements when the popup actually lays them out.
#[derive(IntoElement)]
struct CheckedLabel {
    content: Box<dyn Fn() -> Div>,
    checked: bool,
}

impl RenderOnce for CheckedLabel {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        div()
            .w_full()
            .flex()
            .items_center()
            .justify_between()
            .gap(SPACE_2)
            .child((self.content)())
            .when(self.checked, |row| row.child(Icon::Check.draw(14.)))
    }
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

    pub(super) fn checked_index(&self) -> usize {
        self.checked.unwrap_or(0)
    }

    pub(super) fn with_parent(self, title: &'static str, state: &Entity<DropdownState>) -> Self {
        let parent = state.clone();
        let mut page = Self::new().item("back", Icon::ArrowLeft.label(title), move |_, cx| {
            parent.update(cx, DropdownState::back);
        });
        page.checked = self.checked.map(|i| i + 1);
        for item in self.items {
            let parent = state.clone();
            page = page.item(item.id, item.label, move |w, cx| {
                parent.update(cx, DropdownState::back);
                (item.on_select)(w, cx);
            });
        }
        page
    }

    /// A checked row represents the committed value independently of keyboard or pointer focus.
    pub fn checked_item(
        mut self,
        id: impl Into<ElementId>,
        content: Box<dyn Fn() -> Div>,
        checked: bool,
        on_select: impl Fn(&mut Window, &mut App) + 'static,
    ) -> Self {
        if checked {
            self.checked.get_or_insert(self.items.len());
        }
        self.item(id, CheckedLabel { content, checked }, on_select)
    }

    fn popup(
        self,
        state: &Entity<DropdownState>,
        highlighted: usize,
        layout: DropdownLayout,
        window: &Window,
        cx: &App,
    ) -> Deferred {
        let ui = Ui::new(window, cx);
        let viewport = window.viewport_size();
        let outside = state.clone();
        let menu = state.read(cx);
        let count = self.items.len();
        let mut height = (viewport.height - px(48.)).max(px(32.));
        let mut offset = SPACE_1;
        let mut list = ui
            .popover("content")
            .flex_none()
            .whitespace_nowrap()
            .max_w((viewport.width - popup_margin(window) * 2.).max(px(1.)));
        if layout == DropdownLayout::Filters {
            list = list.w(px(240.));
            height = height.min(px(440.));
        }
        if layout == DropdownLayout::AboveEnd {
            height = (viewport.height - FLOATING_ACTION_CLEARANCE - popup_margin(window)).max(MENU_ROW_HEIGHT);
            offset = -SPACE_2;
            list = list.min_w(px(176.));
        }
        if layout == DropdownLayout::Field
            && let Some(trigger) = menu.trigger_bounds
        {
            let placement = menu.field_placement(trigger, count, window);
            list = list.w(trigger.size.width);
            height = placement.height;
            offset = placement.top - trigger.bottom();
        }
        list = list
            .track_scroll(&menu.scroll)
            .max_h(height)
            .on_mouse_down_out(move |_, _, cx| outside.update(cx, DropdownState::dismiss));
        if layout == DropdownLayout::Field {
            let state = state.clone();
            // A wheel delta is shared between revealing more rows and scrolling the list.
            // Clip and position rows with the handle, without GPUI also applying that delta.
            list = list
                .overflow_hidden()
                .on_scroll_wheel(move |event, window, cx| {
                    state.update(cx, |menu, cx| {
                        if let Some(trigger) = menu.trigger_bounds {
                            let placement = menu.field_placement(trigger, count, window);
                            let distance = -event.delta.pixel_delta(window.line_height()).y;
                            menu.scroll_field(&placement, distance, cx);
                        }
                    });
                    cx.stop_propagation();
                });
        }
        for (index, item) in self.items.into_iter().enumerate() {
            let hover = state.clone();
            let select = state.clone();
            let row = ui
                .menu_item(item.id, item.label, index == highlighted)
                .when(layout == DropdownLayout::Filters, |row| {
                    row.on_mouse_down(MouseButton::Left, |_, window, _| window.prevent_default())
                })
                .on_mouse_move(move |_, _, cx| {
                    hover.update(cx, |menu, cx| {
                        if menu.is_open() {
                            menu.highlight(Some(index), cx);
                        }
                    });
                })
                .on_click(move |event, window, cx| {
                    if event.standard_click() {
                        select.update(cx, |menu, cx| {
                            if layout == DropdownLayout::Filters {
                                menu.highlight(Some(index), cx);
                            } else {
                                menu.dismiss(cx);
                            }
                        });
                        (item.on_select)(window, cx);
                    }
                });
            list = list.child(row);
        }
        deferred(
            anchored()
                .anchor(match layout {
                    DropdownLayout::End | DropdownLayout::Filters => Corner::TopRight,
                    DropdownLayout::AboveEnd => Corner::BottomRight,
                    _ => Corner::TopLeft,
                })
                .offset(point(px(0.), offset))
                .snap_to_window_with_margin(px(6.))
                .child(list),
        )
    }
}

fn popup_margin(window: &Window) -> Pixels {
    px(6.) + window.client_inset().unwrap_or_default()
}

fn select_inset() -> Pixels {
    MENU_PADDING + px(1.)
}

struct SelectPlacement {
    top: Pixels,
    height: Pixels,
    opening_scroll: Pixels,
    content_height: Pixels,
    growth_left: Pixels,
}

impl SelectPlacement {
    fn new(center: Pixels, index: usize, count: usize, viewport: Pixels, margin: Pixels) -> Self {
        // The outer border and padding surround a list of equal-height menu items.
        let total = select_inset() * 2. + MENU_ROW_HEIGHT * count as f32;
        let max_height = total
            .min(select_inset() * 2. + MENU_ROW_HEIGHT * 9.)
            .min((viewport - margin * 2.).max(px(1.)));
        let row_center =
            select_inset() + MENU_ROW_HEIGHT * (index.min(count.saturating_sub(1)) as f32 + 0.5);
        let below = (viewport - margin - center).max(px(0.));
        // Keep the checked row over the field. Near the bottom, open with only the
        // rows that fit around it. At the end of the list its short tail already fits.
        let height = if total - row_center <= below {
            max_height
        } else {
            (below + below.min(row_center))
                .max(MENU_ROW_HEIGHT + select_inset() * 2.)
                .min(max_height)
        };
        let half_row = (select_inset() + MENU_ROW_HEIGHT * 0.5).min(height * 0.5);
        let center = center.clamp(
            margin + half_row,
            (viewport - margin - half_row).max(margin + half_row),
        );
        let overflow = total - height;
        let centered_scroll = (row_center - height * 0.5).clamp(px(0.), overflow);
        let top = (center - row_center + centered_scroll)
            .clamp(margin, (viewport - margin - height).max(margin));
        Self {
            top,
            height,
            opening_scroll: (top + row_center - center).clamp(px(0.), overflow),
            content_height: total,
            growth_left: (max_height - height).min(top - margin),
        }
    }

    fn expanded(mut self, amount: Pixels) -> Self {
        let growth = amount.min(self.growth_left);
        self.top -= growth;
        self.height += growth;
        self.growth_left -= growth;
        self
    }

    fn scroll_by(&self, offset: Pixels, distance: Pixels) -> (Pixels, Pixels) {
        let target = (offset + distance).clamp(px(0.), self.content_height - self.height);
        let growth = (target - offset).clamp(px(0.), self.growth_left);
        (growth, target - growth)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum DropdownLayout {
    End,
    Filters,
    AboveEnd,
    Start,
    Field,
}

#[derive(IntoElement)]
pub struct Dropdown<T: InteractiveElement + IntoElement + 'static> {
    state: Entity<DropdownState>,
    trigger: DropdownTrigger<T>,
    content: DropdownContent,
    layout: DropdownLayout,
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
            layout: DropdownLayout::End,
        }
    }

    /// Grouped filters stay open while the user combines choices.
    pub(super) fn filters(mut self) -> Self {
        self.layout = DropdownLayout::Filters;
        self
    }

    pub fn above_end(mut self) -> Self {
        self.layout = DropdownLayout::AboveEnd;
        self
    }

    pub fn align_start(mut self) -> Self {
        self.layout = DropdownLayout::Start;
        self
    }

    /// Fill the form column and align the popup with the leading edge of the trigger.
    pub fn field(mut self) -> Self {
        self.layout = DropdownLayout::Field;
        self
    }
}

impl<T: InteractiveElement + IntoElement> RenderOnce for Dropdown<T> {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let Self {
            state,
            trigger,
            content,
            layout,
        } = self;
        let field = layout == DropdownLayout::Field;
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
                            if menu.trigger_bounds != Some(*trigger) {
                                menu.trigger_bounds = Some(*trigger);
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
            .child(trigger.bind(
                state.clone(),
                actions,
                content.checked,
                layout == DropdownLayout::Filters,
                cx,
            ))
            .when_some(selected, |root, i| {
                root.child(
                    div()
                        .absolute()
                        .when(layout == DropdownLayout::AboveEnd, |anchor| anchor.top_0().right_0())
                        .when(layout != DropdownLayout::AboveEnd, |anchor| anchor.bottom_0())
                        .when(matches!(layout, DropdownLayout::Start | DropdownLayout::Field), |anchor| anchor.left_0())
                        .when(matches!(layout, DropdownLayout::End | DropdownLayout::Filters), |anchor| anchor.right_0())
                        .size(px(0.))
                        .child(content.popup(&state, i, layout, window, cx)),
                )
            })
    }
}

#[cfg(test)]
mod form_controls {
    use super::{MenuAction::*, SelectPlacement, navigation};
    use gpui::px;

    #[test]
    fn selected_row_aligns_with_the_trigger_and_scrolls_into_view_on_open() {
        // Three choices fit without scrolling. Changing the selection moves the popup.
        for (i, top) in [(0, 277.), (1, 243.), (2, 209.)] {
            let p = SelectPlacement::new(px(300.), i, 3, px(600.), px(6.));
            assert_eq!(
                (p.top, p.opening_scroll, p.height),
                (px(top), px(0.), px(114.))
            );
        }
        // A long list can align the current row even when the window constrains its top.
        for center in [40., 180., 560.] {
            let p = SelectPlacement::new(px(center), 24, 60, px(600.), px(6.));
            assert_eq!(p.top + px(6. + 24.5 * 34.) - p.opening_scroll, px(center));
            assert!(p.top >= px(6.) && p.top + p.height <= px(594.));
        }
        let p = SelectPlacement::new(px(585.), 0, 3, px(600.), px(6.));
        assert_eq!(
            (p.top, p.height, p.opening_scroll),
            (px(548.), px(46.), px(0.))
        );
    }

    #[test]
    fn bottom_constrained_select_grows_by_the_downward_scroll_before_moving_its_contents() {
        let open = || SelectPlacement::new(px(550.), 24, 60, px(600.), px(6.));
        let initial = open();
        let offset = initial.opening_scroll;
        assert_eq!((initial.top, initial.height), (px(506.), px(88.)));
        assert_eq!(
            initial.scroll_by(offset, px(-8.)),
            (px(0.), offset - px(8.))
        );
        let (first, scroll) = initial.scroll_by(offset, px(12.5));
        assert_eq!((first, scroll), (px(12.5), offset));
        let next = open().expanded(first);
        assert_eq!((next.top, next.height), (px(493.5), px(100.5)));
        let (rest, scroll) = next.scroll_by(scroll, px(240.));
        let full = open().expanded(first + rest);
        assert_eq!((full.height, full.top + full.height), (px(318.), px(594.)));
        assert_eq!(scroll - offset, px(22.5));
        assert_eq!(full.scroll_by(scroll, px(34.)), (px(0.), scroll + px(34.)));
        assert_eq!(
            full.scroll_by(scroll, px(9999.)).1,
            full.content_height - full.height
        );
    }

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
