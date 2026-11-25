//! The trigger owns the focus target. The content owns the selectable rows and popup.
use super::{
    controls::{MENU_PADDING, MENU_ROW_HEIGHT},
    *,
};
use std::rc::Rc;

type SelectAction = Rc<dyn Fn(&mut Window, &mut App)>;

/// Tracks whichever dropdown popup is open so platform navigation and overlays stay in sync.
#[derive(Default)]
pub struct ActiveDropdown(Option<WeakEntity<DropdownState>>);
impl Global for ActiveDropdown {}

pub struct DropdownState {
    focus: FocusHandle,
    open: bool,
    item_count: usize,
    highlighted: Option<usize>,
    parent_row: Option<usize>,
    scroll: ScrollHandle,
    trigger_bounds: Option<Bounds<Pixels>>,
    opening_index: usize,
    expansion_up: Pixels,
    expansion_down: Pixels,
}

impl DropdownState {
    pub fn local(id: ElementId, window: &mut Window, cx: &mut App) -> Entity<Self> {
        window.use_keyed_state(id, cx, Self::init)
    }

    fn active(cx: &App) -> Option<Entity<Self>> {
        cx.try_global::<ActiveDropdown>()?
            .0
            .as_ref()?
            .upgrade()
            .filter(|menu| menu.read(cx).is_open())
    }

    pub fn has_open(cx: &App) -> bool {
        Self::active(cx).is_some()
    }

    pub fn dismiss_active(cx: &mut App) {
        if let Some(menu) = Self::active(cx) {
            menu.update(cx, Self::dismiss);
        }
    }

    pub fn back_active(cx: &mut App) -> bool {
        let Some(menu) = Self::active(cx) else {
            return false;
        };
        menu.update(cx, Self::back);
        true
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
        let id = cx.entity_id();
        cx.on_release(move |_, cx| {
            if cx
                .try_global::<ActiveDropdown>()
                .and_then(|active| active.0.as_ref())
                .is_some_and(|menu| menu.entity_id() == id)
            {
                cx.set_global(ActiveDropdown(None));
            }
        })
        .detach();
        Self {
            focus,
            open: false,
            item_count: 0,
            highlighted: None,
            parent_row: None,
            scroll: ScrollHandle::new(),
            trigger_bounds: None,
            opening_index: 0,
            expansion_up: px(0.),
            expansion_down: px(0.),
        }
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    pub fn dismiss(&mut self, cx: &mut Context<Self>) {
        if self.open {
            self.open = false;
            self.highlighted = None;
            self.parent_row = None;
            let current = cx.entity_id();
            if cx
                .try_global::<ActiveDropdown>()
                .and_then(|active| active.0.as_ref())
                .is_some_and(|menu| menu.entity_id() == current)
            {
                cx.set_global(ActiveDropdown(None));
            }
            cx.notify();
        }
    }

    pub(super) fn parent_row(&self) -> Option<usize> {
        self.parent_row
    }

    pub(super) fn enter(&mut self, parent: usize, selected: Option<usize>, cx: &mut Context<Self>) {
        self.parent_row = Some(parent);
        self.show_row(selected, cx);
    }

    pub fn back(&mut self, cx: &mut Context<Self>) {
        if let Some(parent) = self.parent_row.take() {
            self.show_row(Some(parent), cx);
        } else {
            self.dismiss(cx);
        }
    }

    fn show_row(&mut self, index: Option<usize>, cx: &mut Context<Self>) {
        self.scroll.set_offset(Point::default());
        if let Some(index) = index {
            self.scroll.scroll_to_item(index);
        }
        self.highlighted = index;
        cx.notify();
    }

    fn highlight(&mut self, item: Option<usize>, cx: &mut Context<Self>) {
        if self.highlighted != item {
            self.highlighted = item;
            cx.notify();
        }
    }

    fn open(&mut self, selected: Option<usize>, count: usize, window: &Window, cx: &mut Context<Self>) {
        if self.open { return; }
        self.open = true;
        self.item_count = count;
        self.opening_index = selected.unwrap_or(0);
        self.expansion_up = px(0.);
        self.expansion_down = px(0.);
        let current = cx.entity_id();
        let previous = cx
            .try_global::<ActiveDropdown>()
            .and_then(|active| active.0.clone())
            .filter(|menu| menu.entity_id() != current);
        cx.set_global(ActiveDropdown(Some(cx.weak_entity())));
        if let Some(previous) = previous {
            let _ = previous.update(cx, Self::dismiss);
        }
        if let Some(trigger) = self.trigger_bounds {
            let placement = self.field_placement(trigger, count, window, cx);
            self.scroll.set_offset(point(px(0.), -placement.opening_scroll));
            self.highlighted = selected;
            cx.notify();
        } else {
            self.show_row(selected, cx);
        }
    }

    fn focus_item(&mut self, index: usize, count: usize, window: &Window, cx: &mut Context<Self>) {
        if !self.open {
            self.open(Some(index), count, window, cx);
            return;
        }
        if let Some(trigger) = self.trigger_bounds {
            let placement = self.field_placement(trigger, count, window, cx);
            let offset = -self.scroll.offset().y;
            let start = MENU_ROW_HEIGHT * index as f32;
            let end = start + MENU_ROW_HEIGHT + select_inset() * 2.;
            let distance = if start < offset {
                start - offset
            } else {
                (end - offset - placement.height).max(px(0.))
            };
            self.scroll_field(&placement, distance, cx);
        } else {
            self.scroll.scroll_to_item(index);
        }
        self.highlight(Some(index), cx);
    }

    /// Native Android views above the GPUI surface must leave room for an open field menu.
    #[cfg(target_os = "android")]
    pub fn active_field_popup_top(window: &Window, cx: &App) -> Option<Pixels> {
        let menu = Self::active(cx)?;
        let menu = menu.read(cx);
        menu.trigger_bounds
            .map(|trigger| menu.field_placement(trigger, menu.item_count, window, cx).top)
    }

    fn field_placement(
        &self,
        trigger: Bounds<Pixels>,
        count: usize,
        window: &Window,
        cx: &App,
    ) -> SelectPlacement {
        let insets = popup_insets(window, cx);
        SelectPlacement::new(
            trigger.center().y,
            self.opening_index,
            count,
            window.viewport_size().height,
            insets.top,
            insets.bottom,
        )
        .expanded(self.expansion_up, self.expansion_down)
    }

    fn scroll_field(
        &mut self,
        placement: &SelectPlacement,
        distance: Pixels,
        cx: &mut Context<Self>,
    ) {
        let offset = -self.scroll.offset().y;
        let (grow_up, grow_down, next_offset) = placement.scroll_by(offset, distance);
        if grow_up > px(0.) || grow_down > px(0.) || next_offset != offset {
            self.expansion_up += grow_up;
            self.expansion_down += grow_down;
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
            self.open,
            self.highlighted,
            checked,
            count,
            key_name,
            event.is_held,
        )?;
        let mut activate = None;
        match action {
            MenuAction::Open => self.open(checked, count, window, cx),
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
    Open,
    Highlight(usize),
    Activate(usize),
    Dismiss,
    Traverse,
}

fn navigation(
    open: bool,
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
        (None, "enter" | "space") if !held => Open,
        (None, "down") if !held => Highlight(checked.unwrap_or(0)),
        (None, "up") => Highlight(checked.unwrap_or(last)),
        (_, "home") if open => Highlight(0),
        (_, "end") if open => Highlight(last),
        (_, "escape") if open => Dismiss,
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
                        menu.open(checked, count, window, cx);
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

    pub(super) fn checked_index(&self) -> Option<usize> {
        self.checked
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
        highlighted: Option<usize>,
        layout: DropdownLayout,
        window: &Window,
        cx: &App,
    ) -> Deferred {
        let ui = Ui::new(window, cx);
        let viewport = window.viewport_size();
        let insets = popup_insets(window, cx);
        let safe_width = (viewport.width - insets.left - insets.right).max(px(1.));
        let safe_height = (viewport.height - insets.top - insets.bottom).max(px(1.));
        let outside = state.clone();
        let menu = state.read(cx);
        let count = self.items.len();
        let mut height = safe_height;
        let mut offset = SPACE_1;
        let mut list = ui
            .popover("content")
            .flex_none()
            .whitespace_nowrap()
            .max_w(safe_width);
        if layout == DropdownLayout::Filters {
            list = list.w(px(240.).min(safe_width));
            height = safe_height.min(px(440.));
        }
        if layout == DropdownLayout::AboveEnd {
            height = (safe_height - FLOATING_ACTION_CLEARANCE)
                .max(MENU_ROW_HEIGHT)
                .min(safe_height);
            offset = -SPACE_2;
            list = list.min_w(px(176.).min(safe_width));
        }
        if layout == DropdownLayout::Field
            && let Some(trigger) = menu.trigger_bounds
        {
            let placement = menu.field_placement(trigger, count, window, cx);
            list = list.w(trigger.size.width.min(safe_width));
            height = placement.height;
            offset = placement.top - trigger.bottom();
        }
        list = list
            .track_scroll(&menu.scroll)
            .max_h(height)
            .on_mouse_down_out(move |_, _, cx| outside.update(cx, DropdownState::dismiss));
        if layout == DropdownLayout::Field {
            let state = state.clone();
            // A wheel or touch-pan delta is shared between expanding into safe space and scrolling.
            // Clip and position rows with the handle, without GPUI also applying that delta.
            list = list
                .overflow_hidden()
                .on_scroll_wheel(move |event, window, cx| {
                    state.update(cx, |menu, cx| {
                        if let Some(trigger) = menu.trigger_bounds {
                            let placement = menu.field_placement(trigger, count, window, cx);
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
                .menu_item(item.id, item.label, Some(index) == highlighted)
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
        deferred(ClampedPopup {
            corner: match layout {
                DropdownLayout::End | DropdownLayout::Filters => PopupCorner::TopRight,
                DropdownLayout::AboveEnd => PopupCorner::BottomRight,
                _ => PopupCorner::TopLeft,
            },
            offset: point(px(0.), offset),
            insets,
            child: list.into_any_element(),
        })
    }
}

fn popup_insets(window: &Window, cx: &App) -> Edges<Pixels> {
    let margin = px(6.) + window.client_inset().unwrap_or_default();
    let safe = theme::safe_area(cx);
    Edges {
        top: safe.top + margin,
        right: safe.right + margin,
        bottom: safe.bottom + margin,
        left: safe.left + margin,
    }
}

#[derive(Clone, Copy)]
enum PopupCorner {
    TopLeft,
    TopRight,
    BottomRight,
}

fn clamp_popup(
    anchor: Point<Pixels>,
    corner: PopupCorner,
    size: Size<Pixels>,
    offset: Point<Pixels>,
    viewport: Size<Pixels>,
    insets: Edges<Pixels>,
) -> Point<Pixels> {
    let mut origin = match corner {
        PopupCorner::TopLeft => anchor,
        PopupCorner::TopRight => point(anchor.x - size.width, anchor.y),
        PopupCorner::BottomRight => point(anchor.x - size.width, anchor.y - size.height),
    } + offset;
    let min_x = insets.left;
    let max_x = (viewport.width - insets.right - size.width).max(min_x);
    let min_y = insets.top;
    let max_y = (viewport.height - insets.bottom - size.height).max(min_y);
    origin.x = origin.x.clamp(min_x, max_x);
    origin.y = origin.y.clamp(min_y, max_y);
    origin
}

struct ClampedPopup {
    corner: PopupCorner,
    offset: Point<Pixels>,
    insets: Edges<Pixels>,
    child: AnyElement,
}

impl IntoElement for ClampedPopup {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for ClampedPopup {
    type RequestLayoutState = LayoutId;
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let child_id = self.child.request_layout(window, cx);
        let style = Style {
            position: Position::Absolute,
            display: Display::Flex,
            ..Style::default()
        };
        let layout_id = window.request_layout(style, [child_id], cx);
        (layout_id, child_id)
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        child_id: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let child_bounds = window.layout_bounds(*child_id);
        let origin = clamp_popup(
            bounds.origin,
            self.corner,
            child_bounds.size,
            self.offset,
            window.viewport_size(),
            self.insets,
        );
        let delta = origin - child_bounds.origin;
        let delta = point(delta.x.round(), delta.y.round());
        window.with_element_offset(delta, |window| {
            self.child.prepaint(window, cx);
        });
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _child_id: &mut Self::RequestLayoutState,
        _prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        self.child.paint(window, cx);
    }
}

fn select_inset() -> Pixels {
    MENU_PADDING + px(1.)
}

struct SelectPlacement {
    top: Pixels,
    height: Pixels,
    opening_scroll: Pixels,
    content_height: Pixels,
    max_growth: Pixels,
    room_above: Pixels,
    room_below: Pixels,
}

impl SelectPlacement {
    fn new(
        center: Pixels,
        index: usize,
        count: usize,
        viewport: Pixels,
        top_margin: Pixels,
        bottom_margin: Pixels,
    ) -> Self {
        let safe_height = (viewport - top_margin - bottom_margin).max(px(1.));
        // The outer border and padding surround a list of equal-height menu items.
        let total = select_inset() * 2. + MENU_ROW_HEIGHT * count as f32;
        let max_height = total
            .min(select_inset() * 2. + MENU_ROW_HEIGHT * 9.)
            .min(safe_height);
        let row_center =
            select_inset() + MENU_ROW_HEIGHT * (index.min(count.saturating_sub(1)) as f32 + 0.5);
        let min_height = (MENU_ROW_HEIGHT + select_inset() * 2.).min(max_height);
        let half_min = min_height * 0.5;
        let max_center = (viewport - bottom_margin - half_min).max(top_margin + half_min);
        let center = center.clamp(top_margin + half_min, max_center);

        // Keep the checked row aligned with the trigger center while expanding into all
        // available safe space on both sides instead of capping one side by the other.
        let space_above = (center - top_margin).max(px(0.));
        let space_below = (viewport - bottom_margin - center).max(px(0.));
        let max_above = row_center.min(space_above);
        let max_below = (total - row_center).min(space_below);
        let height = (max_above + max_below).max(min_height).min(max_height);
        let above = (height * 0.5)
            .min(max_above)
            .max((height - max_below).min(max_above));
        let max_top = (viewport - bottom_margin - height).max(top_margin);
        let top = (center - above).clamp(top_margin, max_top);
        let overflow = (total - height).max(px(0.));
        let opening_scroll = (top + row_center - center).clamp(px(0.), overflow);
        Self {
            top,
            height,
            opening_scroll,
            content_height: total,
            max_growth: (max_height - height).max(px(0.)),
            room_above: (top - top_margin).max(px(0.)),
            room_below: (viewport - bottom_margin - (top + height)).max(px(0.)),
        }
    }

    fn expanded(mut self, up: Pixels, down: Pixels) -> Self {
        let up = up.clamp(px(0.), self.max_growth.min(self.room_above));
        self.top -= up;
        self.height += up;
        self.max_growth -= up;
        self.room_above -= up;
        let down = down.clamp(px(0.), self.max_growth.min(self.room_below));
        self.height += down;
        self.max_growth -= down;
        self.room_below -= down;
        self
    }

    fn scroll_by(&self, offset: Pixels, distance: Pixels) -> (Pixels, Pixels, Pixels) {
        let max_offset = (self.content_height - self.height).max(px(0.));
        let target = (offset + distance).clamp(px(0.), max_offset);
        if distance >= px(0.) {
            let grow_up = (target - offset).clamp(px(0.), self.max_growth.min(self.room_above));
            (grow_up, px(0.), target - grow_up)
        } else {
            let grow_down = (offset - target).clamp(px(0.), self.max_growth.min(self.room_below));
            (px(0.), grow_down, target)
        }
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

#[derive(IntoElement)]
pub struct ActionsMenu {
    id: ElementId,
    label: SharedString,
    content: DropdownContent,
    circular: bool,
}

impl ActionsMenu {
    pub fn circular_trigger(mut self) -> Self {
        self.circular = true;
        self
    }
}

impl RenderOnce for ActionsMenu {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let ui = Ui::new(window, cx);
        let state = DropdownState::local((self.id.clone(), "menu").into(), window, cx);
        let open = state.read(cx).is_open();
        let trigger = ui
            .icon_button(ButtonVariant::Ghost, self.id, Icon::Ellipsis, self.label.clone())
            .block_mouse_except_scroll()
            .pressed(open);
        let trigger = if self.circular {
            trigger.circular()
        } else {
            trigger
        };
        #[cfg(target_os = "android")]
        let trigger = trigger
            .role(Role::Button)
            .aria_label(self.label)
            .aria_expanded(open);
        Dropdown::new(&state, DropdownTrigger::new(trigger), self.content)
    }
}

impl Ui {
    pub fn actions_menu(
        self,
        id: impl Into<ElementId>,
        label: SharedString,
        content: DropdownContent,
    ) -> ActionsMenu {
        ActionsMenu {
            id: id.into(),
            label,
            content,
            circular: false,
        }
    }
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
        } else if state.read(cx).item_count != count {
            state.update(cx, |menu, _| menu.item_count = count);
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
            .when(state.read(cx).is_open(), |root| {
                root.child(
                    div()
                        .absolute()
                        .when(layout == DropdownLayout::AboveEnd, |anchor| anchor.top_0().right_0())
                        .when(layout != DropdownLayout::AboveEnd, |anchor| anchor.bottom_0())
                        .when(matches!(layout, DropdownLayout::Start | DropdownLayout::Field), |anchor| anchor.left_0())
                        .when(matches!(layout, DropdownLayout::End | DropdownLayout::Filters), |anchor| anchor.right_0())
                        .size(px(0.))
                        .child(content.popup(&state, selected, layout, window, cx)),
                )
            })
    }
}

#[cfg(test)]
mod form_controls {
    use super::{MenuAction::*, PopupCorner, SelectPlacement, clamp_popup, navigation};
    use gpui::{Edges, point, px, size};

    #[test]
    fn selected_row_aligns_with_the_trigger_and_scrolls_into_view_on_open() {
        // Three choices fit without scrolling. Changing the selection moves the popup.
        for (i, top) in [(0, 277.), (1, 243.), (2, 209.)] {
            let p = SelectPlacement::new(px(300.), i, 3, px(600.), px(6.), px(6.));
            assert_eq!(
                (p.top, p.opening_scroll, p.height),
                (px(top), px(0.), px(114.))
            );
        }
        // A long list can align the current row even when the window constrains its top.
        for center in [40., 180., 560.] {
            let p = SelectPlacement::new(px(center), 24, 60, px(600.), px(6.), px(6.));
            assert_eq!(p.top + px(6. + 24.5 * 34.) - p.opening_scroll, px(center));
            assert!(p.top >= px(6.) && p.top + p.height <= px(594.));
        }
        let p = SelectPlacement::new(px(585.), 0, 3, px(600.), px(6.), px(6.));
        assert_eq!(
            (p.top, p.height, p.opening_scroll),
            (px(548.), px(46.), px(0.))
        );
    }

    #[test]
    fn bottom_constrained_select_aligns_selection_and_expands_upward() {
        // Middle selection near bottom: stays aligned with trigger AND immediately expands upward to 9 rows.
        let p = SelectPlacement::new(px(550.), 24, 60, px(600.), px(6.), px(6.));
        assert_eq!((p.top, p.height), (px(276.), px(318.)));
        assert_eq!(p.top + px(6. + 24.5 * 34.) - p.opening_scroll, px(550.));

        // 3rd of 4 items near bottom safe area: aligns item 2 with center and shows items 0..=2 above it,
        // then scrolling downward grows the top edge upward to reveal item 3.
        let open = || SelectPlacement::new(px(803.), 2, 4, px(880.), px(38.), px(54.));
        let initial = open();
        assert_eq!((initial.top, initial.height, initial.opening_scroll), (px(712.), px(114.), px(0.)));
        assert_eq!(initial.top + px(6. + 2.5 * 34.) - initial.opening_scroll, px(803.));
        let (up, down, next_scroll) = initial.scroll_by(initial.opening_scroll, px(34.));
        assert_eq!((up, down, next_scroll), (px(34.), px(0.), px(0.)));
        let grown = open().expanded(up, down);
        assert_eq!((grown.top, grown.height), (px(678.), px(148.)));

        // Last of 4 items near top safe area: aligns item 3 with center, scrolling upward grows downward.
        let top_open = || SelectPlacement::new(px(61.), 3, 4, px(880.), px(38.), px(54.));
        let top_initial = top_open();
        assert_eq!((top_initial.top, top_initial.height, top_initial.opening_scroll), (px(38.), px(46.), px(102.)));
        let (up, down, next_scroll) = top_initial.scroll_by(top_initial.opening_scroll, px(-68.));
        assert_eq!((up, down, next_scroll), (px(0.), px(68.), px(34.)));
        let top_grown = top_open().expanded(up, down);
        assert_eq!((top_grown.top, top_grown.height), (px(38.), px(114.)));
    }

    #[test]
    fn select_placement_excludes_asymmetric_safe_area_insets() {
        let top_inset = px(38.);
        let bottom_inset = px(54.);
        let viewport = px(880.);
        for center in [45., 120., 440., 790., 840.] {
            let p = SelectPlacement::new(px(center), 24, 60, viewport, top_inset, bottom_inset);
            assert!(p.top >= top_inset, "center={center}: top={:?}", p.top);
            assert!(
                p.top + p.height <= viewport - bottom_inset,
                "center={center}: bottom={:?}",
                p.top + p.height
            );
            assert_eq!(p.height, px(318.));
            let expanded = p.expanded(px(1000.), px(1000.));
            assert!(expanded.top >= top_inset);
            assert!(expanded.top + expanded.height <= viewport - bottom_inset);
        }
        // A tall soft keyboard shrinks the safe height cleanly below the 9-row cap.
        let p = SelectPlacement::new(px(260.), 10, 40, viewport, top_inset, px(640.));
        assert!(p.top >= top_inset);
        assert!(p.top + p.height <= viewport - px(640.));
    }

    #[test]
    fn clamped_popup_excludes_all_safe_area_edges() {
        let viewport = size(px(360.), px(880.));
        let insets = Edges {
            top: px(38.),
            right: px(54.),
            bottom: px(54.),
            left: px(38.),
        };
        // Bottom overflow inside the safe margin (still < viewport height) must clamp above bottom inset.
        let pos = clamp_popup(
            point(px(300.), px(724.)),
            PopupCorner::TopRight,
            size(px(180.), px(120.)),
            point(px(0.), px(4.)),
            viewport,
            insets,
        );
        assert_eq!(pos, point(px(120.), px(706.)));
        // Top overflow into the status bar clamps to top inset.
        let pos = clamp_popup(
            point(px(100.), px(80.)),
            PopupCorner::BottomRight,
            size(px(180.), px(120.)),
            point(px(0.), px(-8.)),
            viewport,
            insets,
        );
        assert_eq!(pos, point(px(38.), px(38.)));
        // Right overflow into landscape navigation bar clamps inside right inset.
        let pos = clamp_popup(
            point(px(260.), px(200.)),
            PopupCorner::TopLeft,
            size(px(180.), px(120.)),
            point(px(0.), px(4.)),
            viewport,
            insets,
        );
        assert_eq!(pos, point(px(126.), px(204.)));
    }

    #[test]
    fn select_opens_at_the_committed_value_and_only_activates_on_confirmation() {
        assert_eq!(
            navigation(false, None, Some(1), 3, "down", false),
            Some(Highlight(1))
        );
        assert_eq!(
            navigation(true, Some(1), Some(1), 3, "down", false),
            Some(Highlight(2))
        );
        assert_eq!(
            navigation(true, Some(2), Some(1), 3, "escape", false),
            Some(Dismiss)
        );
        assert_eq!(
            navigation(false, None, Some(1), 3, "up", false),
            Some(Highlight(1))
        );
        assert_eq!(
            navigation(true, Some(2), Some(1), 3, "enter", false),
            Some(Activate(2))
        );
        assert_eq!(navigation(true, Some(2), Some(1), 3, "enter", true), None);
    }

    #[test]
    fn an_open_action_menu_can_dismiss_without_a_highlight() {
        assert_eq!(navigation(false, None, None, 2, "enter", false), Some(Open));
        assert_eq!(navigation(true, None, None, 2, "escape", false), Some(Dismiss));
        assert_eq!(navigation(true, None, None, 2, "down", false), Some(Highlight(0)));
    }

    #[test]
    fn action_menus_wrap_and_empty_or_shortened_lists_are_safe() {
        assert_eq!(navigation(false, None, None, 3, "up", false), Some(Highlight(2)));
        assert_eq!(
            navigation(true, Some(2), None, 3, "down", false),
            Some(Highlight(0))
        );
        assert_eq!(
            navigation(true, Some(0), None, 3, "up", false),
            Some(Highlight(2))
        );
        assert_eq!(
            navigation(true, Some(5), None, 2, "enter", false),
            Some(Activate(1))
        );
        assert_eq!(navigation(false, None, None, 0, "enter", false), None);
        assert_eq!(navigation(false, None, None, 0, "tab", false), Some(Traverse));
    }
}
