//! A native window with a tab strip in its caption or to the left of its active panel.
mod navigation;
mod sidebar;
mod state;
mod tabs;

use super::{Scrollbar, Ui, focus::workspace_keyboard};
use crate::{
    chrome,
    theme::{BUTTON_HEIGHT, SPACE_1, SPACE_2, SPACE_3, SPACE_4},
};
use gpui::{prelude::*, *};
pub(crate) use navigation::DockNavigation;
pub use sidebar::DockSettings;
pub use state::{DockDrag, DockPlacement, DockState};
pub use tabs::DockTabs;

const DEFAULT_SIDEBAR_WIDTH: Pixels = px(220.);
const TOP_TAB_HEIGHT: Pixels = px(31.);
const TAB_MIN_WIDTH: Pixels = px(96.);
const TAB_MAX_WIDTH: Pixels = px(180.);

#[derive(IntoElement)]
pub struct DockPanel {
    content: AnyElement,
    scroll: Option<(ElementId, ScrollHandle)>,
    navigation: Option<DockNavigation>,
}

impl DockPanel {
    pub fn fill(content: impl IntoElement) -> Self {
        Self {
            content: content.into_any_element(),
            scroll: None,
            navigation: None,
        }
    }

    pub(crate) fn with_navigation(mut self, navigation: DockNavigation) -> Self {
        self.navigation = Some(navigation);
        self
    }

    pub fn scroll(
        id: impl Into<ElementId>,
        handle: ScrollHandle,
        content: impl IntoElement,
    ) -> Self {
        Self {
            content: content.into_any_element(),
            scroll: Some((id.into(), handle)),
            navigation: None,
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
    title: SharedString,
    actions: AnyElement,
    tabs: DockTabs<K>,
    panel: DockPanel,
    banner: Option<AnyElement>,
    preview: Option<AnyElement>,
}

impl<K: Clone + PartialEq + 'static> DockLayout<K> {
    pub fn new(
        title: impl Into<SharedString>,
        tabs: DockTabs<K>,
        panel: DockPanel,
        actions: impl IntoElement,
    ) -> Self {
        Self {
            root: workspace_keyboard(div()),
            title: title.into(),
            actions: actions.into_any_element(),
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
    fn render(mut self, w: &mut Window, cx: &mut App) -> impl IntoElement {
        let ui = Ui::new(w, cx);
        let state = self.tabs.state.clone();
        let mut body = div().flex_1().min_h_0().min_w_0().flex();
        let mut caption = div().size_full();
        let settings = state.read(cx).settings();
        let navigation = self.panel.navigation.take();
        let (leading, sidebar) = match settings.placement {
            DockPlacement::Top => {
                let zone = state.clone();
                // The empty part of the native drag caption also accepts incoming tabs.
                caption = caption.child(
                    canvas(
                        move |space, _, cx| {
                            zone.update(cx, |dock, _| {
                                dock.zone = Bounds::from_corners(
                                    point(px(0.), space.top()),
                                    space.bottom_right(),
                                );
                            });
                        },
                        |_, _, _, _| {},
                    )
                    .size_full(),
                );
                (
                    self.tabs.into_any_element(),
                    navigation.map(|nav| nav.into_any_element()),
                )
            }
            DockPlacement::Left => {
                caption = caption
                    .flex()
                    .items_center()
                    .px(SPACE_3)
                    .text_sm()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(self.title);
                if let Some(nav) = navigation {
                    self.tabs = self.tabs.sidebar_content(nav);
                }
                (Empty.into_any_element(), Some(self.tabs.into_any_element()))
            }
        };
        let resize = sidebar
            .as_ref()
            .map(|_| sidebar::capture_resize(state.clone(), cx));
        if let Some(sidebar) = sidebar {
            let width = settings.width(w.viewport_size().width);
            body = body
                .relative()
                .child(div().w(width).h_full().flex_none().child(sidebar))
                .child(self.panel)
                .child(sidebar::divider(state.clone(), width, ui))
                .when(settings.placement == DockPlacement::Left, |body| {
                    body.on_children_prepainted(move |children, _, cx| {
                        if let Some(&strip) = children.first() {
                            state.update(cx, |dock, _| dock.zone = strip);
                        }
                    })
                });
        } else {
            body = body.child(self.panel);
        }
        let header = chrome::titlebar(ui, leading, caption, self.actions, w, cx);
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
            .child(body)
            .children(self.preview)
            .children(resize);
        chrome::frame(content, w)
    }
}
