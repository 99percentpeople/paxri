use super::*;
use crate::{components::Icon, theme::CONTROL_RADIUS};

/// Only tabs participate in scrolling, measurement and reordering.
#[derive(IntoElement)]
pub struct DockTabs<K: Clone + PartialEq + 'static> {
    pub(super) state: Entity<DockState<K>>,
    root: Div,
    id: ElementId,
    items: Vec<(K, Stateful<Div>)>,
    trailing: Option<AnyElement>,
    sidebar_content: Option<AnyElement>,
    pub(super) drag: Option<DockDrag<K>>,
    indent: Pixels,
}

impl<K: Clone + PartialEq + 'static> DockTabs<K> {
    pub fn new(id: impl Into<ElementId>, state: &Entity<DockState<K>>) -> Self {
        Self {
            state: state.clone(),
            root: div(),
            id: id.into(),
            items: vec![],
            trailing: None,
            sidebar_content: None,
            drag: None,
            indent: px(0.),
        }
    }

    pub fn item(mut self, key: K, tab: Stateful<Div>) -> Self {
        self.items.push((key, tab));
        self
    }

    pub fn indent(mut self, indent: Pixels) -> Self {
        self.indent = indent;
        self
    }

    pub fn sidebar_content(mut self, content: impl IntoElement) -> Self {
        self.sidebar_content = Some(content.into_any_element());
        self
    }

    pub fn trailing(mut self, action: impl IntoElement) -> Self {
        self.trailing = Some(action.into_any_element());
        self
    }

    pub(super) fn len(&self) -> usize {
        self.items.len()
    }
}

impl<K: Clone + PartialEq + 'static> InteractiveElement for DockTabs<K> {
    fn interactivity(&mut self) -> &mut Interactivity {
        self.root.interactivity()
    }
}

impl<K: Clone + PartialEq + 'static> RenderOnce for DockTabs<K> {
    fn render(self, w: &mut Window, cx: &mut App) -> impl IntoElement {
        let ui = Ui::new(w, cx);
        let mut items = self.items;
        let layout = self.state.read(cx);
        let placement = layout.placement();
        let scroll = layout.scroll.clone();
        if let Some(drag) = &self.drag {
            drag.reorder(&mut items, layout, |(key, _)| key);
        }
        // Keep the resting slots while reordering. An incoming tab needs one new measurement.
        let measure = self
            .drag
            .as_ref()
            .is_none_or(|drag| layout.bounds(&drag.page).is_none());
        let mut strip = div();
        let mut keys = Vec::with_capacity(items.len());
        for (key, tab) in items {
            let moving = self.drag.as_ref().is_some_and(|drag| drag.page == key);
            keys.push(key);
            strip = strip.child(
                tab.cursor_pointer()
                    .when(moving, |t| t.opacity(0.))
                    .when(self.indent > px(0.), |t| match placement {
                        DockPlacement::Top => t.ml(self.indent),
                        DockPlacement::Left => t.mt(self.indent),
                    }),
            );
        }
        let strip = strip
            .when(measure, |strip| {
                strip.on_children_prepainted(move |_, _, cx| {
                    self.state.update(cx, |layout, cx| {
                        layout.tabs.clear();
                        layout
                            .tabs
                            .extend(keys.iter().enumerate().filter_map(|(index, key)| {
                                scroll
                                    .bounds_for_item(index)
                                    .map(|bounds| (key.clone(), bounds))
                            }));
                        // The new scroll handle has its viewport after the first layout.
                        if let Some(index) = layout.reveal_after_placement.take() {
                            layout.reveal_tab(index);
                            cx.notify();
                        }
                    });
                })
            })
            .id(self.id)
            .track_scroll(&layout.scroll)
            .flex()
            .gap(SPACE_1)
            .p(SPACE_1);
        let root = self
            .root
            .flex()
            .min_w_0()
            .items_center()
            .bg(rgb(ui.colors.chrome.background));
        match placement {
            DockPlacement::Top => root
                .h_full()
                .child(strip.min_w_0().h_full().items_center().overflow_x_scroll())
                .when_some(self.trailing, |root, action| {
                    root.child(div().flex_none().pr(SPACE_1).child(action))
                }),
            DockPlacement::Left => root
                .size_full()
                .flex_col()
                .child(
                    strip
                        .w_full()
                        .min_h_0()
                        .flex_col()
                        .items_center()
                        .when(self.sidebar_content.is_some(), |strip| {
                            strip.max_h(relative(0.45))
                        })
                        .overflow_y_scroll(),
                )
                .when_some(self.trailing, |root, action| {
                    let row = div().flex_none().w_full().flex().pb(SPACE_1);
                    root.child(row.px(SPACE_1).child(div().w_full().child(action)))
                })
                .when_some(self.sidebar_content, |root, content| {
                    root.child(
                        div()
                            .w_full()
                            .min_h_0()
                            .flex_1()
                            .border_t_1()
                            .border_color(rgb(ui.colors.border))
                            .child(content),
                    )
                }),
        }
    }
}

pub(super) fn preview<K: Clone + PartialEq + 'static>(
    state: Entity<DockState<K>>,
    drag: DockDrag<K>,
    tab: Div,
) -> impl IntoElement {
    canvas(
        move |_, w, cx| {
            // Sample after the strip and the caption have prepainted, including on the first frame after docking.
            let layout = state.read(cx);
            let zone = layout.zone;
            let bounds = drag.projected(layout.bounds(&drag.page)?, layout);
            let mut preview = div()
                .size_full()
                .overflow_hidden()
                .child(
                    tab.absolute()
                        .left(bounds.left() - zone.left())
                        .top(bounds.top() - zone.top())
                        .w(bounds.size.width)
                        .h(bounds.size.height)
                        .shadow_sm(),
                )
                .into_any_element();
            preview.prepaint_as_root(zone.origin, zone.size.into(), w, cx);
            Some(preview)
        },
        |_, preview, w, cx| {
            if let Some(mut preview) = preview {
                preview.paint(w, cx);
            }
        },
    )
    .absolute()
    .inset_0()
}

pub fn tab(
    ui: Ui,
    settings: DockSettings,
    icon: Icon,
    label: impl IntoElement,
    selected: bool,
    close: Option<impl IntoElement>,
) -> Div {
    let tab = div()
        .flex_none()
        .flex()
        .items_center()
        .rounded(CONTROL_RADIUS)
        .h(settings.tab_height())
        .text_sm()
        .font_weight(FontWeight::MEDIUM)
        .border_1()
        .border_color(transparent_black())
        .text_color(rgb(ui.colors.muted_foreground))
        .when(!selected, |tab| tab.hover(|s| s.bg(rgb(ui.colors.tab_hover))))
        .when(selected, |tab| {
            tab.bg(rgb(ui.colors.card))
                .border_color(rgb(ui.colors.border))
                .text_color(rgb(ui.colors.accent.foreground))
        });
    let tab = match settings.placement {
        DockPlacement::Top => tab.min_w(TAB_MIN_WIDTH).max_w(TAB_MAX_WIDTH),
        DockPlacement::Left => tab.w_full().min_w_0(),
    };
    tab.overflow_hidden()
        .px(SPACE_3)
        .gap(SPACE_2)
        .child(icon.draw(16.))
        .child(div().min_w_0().flex_grow().truncate().child(label))
        .children(close)
}

pub fn tab_close(ui: Ui) -> Div {
    div()
        .size(px(20.))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded(SPACE_1)
        .text_color(rgb(ui.colors.muted_foreground))
        .child(Icon::Close.draw(14.))
}

pub fn tab_close_button(
    ui: Ui,
    on_close: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    let hover = ui.colors.secondary_hover;
    tab_close(ui)
        .id("close-tab")
        .cursor_pointer()
        .hover(move |s| s.bg(rgb(hover)))
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_click(move |event, w, cx| {
            cx.stop_propagation();
            on_close(event, w, cx);
        })
}
