//! A native window with a tab strip in its caption or to the left of its active panel.
mod sidebar;
mod state;
mod tabs;

use super::{Scrollbar, Ui, track_keyboard_focus};
use crate::{
    chrome,
    theme::{BUTTON_HEIGHT, SPACE_1, SPACE_2, SPACE_3, SPACE_4},
};
use gpui::{prelude::*, *};
pub use sidebar::DockSettings;
pub use state::{DockDrag, DockPlacement, DockState};
pub use tabs::DockTabs;

const TAB_HEIGHT: Pixels = px(31.);
const TAB_MIN_WIDTH: Pixels = px(96.);
const TAB_MAX_WIDTH: Pixels = px(180.);

pub struct DockHeader {
    title: SharedString,
    status: AnyElement,
    actions: AnyElement,
}

impl DockHeader {
    pub fn new(
        title: impl Into<SharedString>,
        status: impl IntoElement,
        actions: impl IntoElement,
    ) -> Self {
        Self {
            title: title.into(),
            status: status.into_any_element(),
            actions: actions.into_any_element(),
        }
    }
}

#[derive(IntoElement)]
pub struct DockPanel {
    content: AnyElement,
    scroll: Option<(ElementId, ScrollHandle)>,
}

impl DockPanel {
    pub fn fill(content: impl IntoElement) -> Self {
        Self {
            content: content.into_any_element(),
            scroll: None,
        }
    }

    pub fn scroll(
        id: impl Into<ElementId>,
        handle: ScrollHandle,
        content: impl IntoElement,
    ) -> Self {
        Self {
            content: content.into_any_element(),
            scroll: Some((id.into(), handle)),
        }
    }
}

impl RenderOnce for DockPanel {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let panel = div().flex_1().min_h_0().min_w_0();
        match self.scroll {
            Some((id, handle)) => panel
                .relative()
                .child(
                    div()
                        .id(id.clone())
                        .size_full()
                        .overflow_y_scroll()
                        .track_scroll(&handle)
                        .p(SPACE_4)
                        .child(self.content),
                )
                .child(Scrollbar::vertical((id, "bar"), handle))
                .into_any_element(),
            None => panel
                .overflow_hidden()
                .child(self.content)
                .into_any_element(),
        }
    }
}

#[derive(IntoElement)]
pub struct DockLayout<K: Clone + PartialEq + 'static> {
    root: Div,
    header: DockHeader,
    tabs: DockTabs<K>,
    panel: DockPanel,
    banner: Option<AnyElement>,
    preview: Option<AnyElement>,
}

impl<K: Clone + PartialEq + 'static> DockLayout<K> {
    pub fn new(header: DockHeader, tabs: DockTabs<K>, panel: DockPanel) -> Self {
        Self {
            root: track_keyboard_focus(div()).on_key_down(|event: &KeyDownEvent, w, cx| {
                let key = &event.keystroke;
                if key.key == "tab"
                    && (!key.modifiers.modified() || key.modifiers == Modifiers::shift())
                {
                    if key.modifiers.shift {
                        w.focus_prev();
                    } else {
                        w.focus_next();
                    }
                    w.prevent_default();
                    cx.stop_propagation();
                }
            }),
            header,
            tabs,
            panel,
            banner: None,
            preview: None,
        }
    }

    pub fn banner(mut self, banner: impl IntoElement) -> Self {
        self.banner = Some(banner.into_any_element());
        self
    }

    /// Paint a proxy above the strip while its resting slot keeps its measured size.
    pub fn dragging(mut self, drag: DockDrag<K>, tab: Div) -> Self {
        if self.tabs.len() > 1 {
            self.preview =
                Some(tabs::preview(self.tabs.state.clone(), drag.clone(), tab).into_any_element());
        }
        self.tabs.drag = Some(drag);
        self
    }
}

impl<K: Clone + PartialEq + 'static> InteractiveElement for DockLayout<K> {
    fn interactivity(&mut self) -> &mut Interactivity {
        self.root.interactivity()
    }
}

impl<K: Clone + PartialEq + 'static> RenderOnce for DockLayout<K> {
    fn render(self, w: &mut Window, cx: &mut App) -> impl IntoElement {
        let ui = Ui::new(w, cx);
        let state = self.tabs.state.clone();
        let mut body = div().flex_1().min_h_0().min_w_0().flex();
        let mut caption = div()
            .size_full()
            .pr(SPACE_2)
            .flex()
            .justify_end()
            .items_center()
            .gap(SPACE_2)
            .text_sm();
        let settings = state.read(cx).settings();
        let resizing = state.read(cx).resizing.is_some();
        let leading = match settings.placement {
            DockPlacement::Top => {
                // The empty part of the native drag caption also accepts incoming tabs.
                caption = caption.child(
                    canvas(
                        move |space, _, cx| {
                            state.update(cx, |dock, _| {
                                dock.zone = Bounds::from_corners(
                                    point(px(0.), space.top()),
                                    space.bottom_right(),
                                );
                            });
                        },
                        |_, _, _, _| {},
                    )
                    .flex_1()
                    .min_w_0()
                    .h_full(),
                );
                self.tabs.into_any_element()
            }
            DockPlacement::Left => {
                let toggle = sidebar::toggle(state.clone(), ui, settings.compact());
                body = body
                    .child(sidebar::sidebar(self.tabs, ui, w, cx))
                    .on_children_prepainted(move |children, _, cx| {
                        if let Some(&strip) = children.first() {
                            state.update(cx, |dock, _| dock.zone = strip);
                        }
                    });
                div()
                    .pl(SPACE_1)
                    .flex()
                    .items_center()
                    .gap(SPACE_2)
                    .child(toggle)
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(self.header.title)
                    .into_any_element()
            }
        };
        let header = chrome::titlebar(
            ui,
            leading,
            caption.child(self.header.status),
            self.header.actions,
            w,
            cx,
        );
        let content = self
            .root
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(ui.colors.canvas.background))
            .text_color(rgb(ui.colors.canvas.foreground))
            .child(header)
            .children(self.banner)
            .child(body.child(self.panel))
            .children(self.preview)
            .when(resizing, |root| {
                root.child(
                    div()
                        .absolute()
                        .inset_0()
                        .occlude()
                        .cursor(CursorStyle::ResizeLeftRight),
                )
            });
        chrome::frame(content, w)
    }
}
