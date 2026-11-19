//! A workspace window owns its tabs; detached windows share the same gateway controller.
use super::*;

mod dock;
mod drag;
mod menu;

gpui::actions!(workspace, [ReconnectTerminal]);

use dock::{DockTarget, TabLayout};
#[cfg(target_os = "macos")]
pub(super) use drag::{DragEvent, DragSignal};
use drag::{DragWatch, DropPlacement, TabDrag};

const SIDEBAR_WIDTH: f32 = 142.;
const WORKSPACE_SIZE: Size<Pixels> = size(px(990.), px(720.));

#[derive(Clone, PartialEq, Eq)]
enum Page {
    Home,
    Settings,
    Terminal {
        peer: SharedString,
        view: Entity<terminal::Terminal>,
    },
}

impl Page {
    fn window_title(&self) -> SharedString {
        match self {
            Self::Home => "P2P Tunnel".into(),
            Self::Settings => "Settings — P2P Tunnel".into(),
            Self::Terminal { peer, .. } => format!("{peer} — Terminal").into(),
        }
    }

    fn icon(&self) -> Icon {
        match self {
            Self::Home => Icon::Home,
            Self::Settings => Icon::Settings,
            Self::Terminal { .. } => Icon::Terminal,
        }
    }

    fn label(&self) -> SharedString {
        match self {
            Self::Home => "Home".into(),
            Self::Settings => "Settings".into(),
            Self::Terminal { peer, .. } => peer.clone(),
        }
    }

    fn element_id(&self) -> ElementId {
        match self {
            Self::Terminal { view, .. } => ("terminal-tab", view.entity_id()).into(),
            _ => self.label().into(),
        }
    }
}

#[derive(Clone)]
struct DraggedTab {
    page: Page,
    source: WindowHandle<Workspace>,
}

struct TabPress {
    position: Point<Pixels>,
    previous_active: Page,
}

enum WorkspaceWindow {
    Initial,
    Detached {
        placement: Option<DropPlacement>,
        tab_indent: Pixels,
    },
}

pub(super) struct Workspace {
    dashboard: Entity<Dashboard>,
    tabs: Vec<Page>,
    active: Page,
    sidebar: bool,
    menu_index: Option<usize>,
    menu_focus: FocusHandle,
    tab_focus: FocusHandle,
    dragged_tab: Option<TabDrag>,
    tab_press: Option<TabPress>,
    tab_indent: Pixels,
    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    wayland_move_left_surface: bool,
    drag_watch: Option<DragWatch>,
    tab_layout: TabLayout,
}

impl Workspace {
    pub(super) fn open_home(
        dashboard: Entity<Dashboard>,
        cx: &mut App,
    ) -> anyhow::Result<WindowHandle<Self>> {
        Self::open(dashboard, Page::Home, false, WorkspaceWindow::Initial, cx)
    }

    fn open(
        dashboard: Entity<Dashboard>,
        page: Page,
        sidebar: bool,
        kind: WorkspaceWindow,
        cx: &mut App,
    ) -> anyhow::Result<WindowHandle<Self>> {
        let title = page.window_title();
        let mut options = chrome::window_options(&title, WORKSPACE_SIZE, cx);
        #[cfg(any(target_os = "linux", target_os = "freebsd"))]
        if cx.compositor_name() == "Wayland" {
            options.app_id = Some("p2p-tunnel".into());
        }
        #[cfg(windows)]
        {
            options.show = matches!(kind, WorkspaceWindow::Initial);
        }
        let tab_indent = match kind {
            WorkspaceWindow::Initial => px(0.),
            WorkspaceWindow::Detached {
                placement,
                tab_indent,
            } => {
                if let Some(placement) = placement {
                    options.display_id = Some(placement.display);
                    options.window_bounds = Some(placement.bounds);
                    options.focus = placement.focus;
                }
                tab_indent
            }
        };
        cx.open_window(options, move |w, cx| {
            w.set_window_title(&title);
            #[cfg(target_os = "macos")]
            chrome::hide_system_buttons(w);
            cx.new(|cx| {
                cx.observe(&dashboard, |_, _, cx| cx.notify()).detach();
                let menu_focus = cx.focus_handle().tab_stop(true);
                menu_focus.focus(w);
                cx.observe_window_activation(w, |v: &mut Self, w, cx| {
                    if !w.is_window_active() {
                        v.dismiss_menu(cx);
                        // Window managers briefly change focus while closing the floating window.
                        if v.drag_watch.is_none() {
                            v.cancel_tab_drag(w, cx);
                        }
                    }
                })
                .detach();
                Self {
                    dashboard,
                    tabs: vec![page.clone()],
                    active: page,
                    sidebar,
                    menu_index: None,
                    menu_focus,
                    tab_focus: cx.focus_handle().tab_stop(true),
                    dragged_tab: None,
                    tab_press: None,
                    tab_indent,
                    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
                    wayland_move_left_surface: false,
                    drag_watch: None,
                    tab_layout: TabLayout::default(),
                }
            })
        })
    }

    fn other_window_with(
        &self,
        w: &Window,
        cx: &App,
        matches: impl Fn(&Page) -> bool,
    ) -> Option<WindowHandle<Self>> {
        cx.windows()
            .into_iter()
            .filter(|handle| *handle != w.window_handle())
            .filter_map(|handle| handle.downcast::<Self>())
            .find(|handle| {
                handle
                    .read(cx)
                    .is_ok_and(|v| v.dashboard == self.dashboard && v.tabs.iter().any(&matches))
            })
    }

    fn window_title(&self) -> SharedString {
        if self.tabs.contains(&Page::Home) {
            Page::Home.window_title()
        } else if self.tabs.contains(&Page::Settings) {
            Page::Settings.window_title()
        } else {
            self.tabs.first().unwrap_or(&self.active).window_title()
        }
    }

    fn refresh_title(&self, w: &mut Window) {
        w.set_window_title(&self.window_title());
    }

    fn select(&mut self, page: Page, cx: &mut Context<Self>) {
        self.active = page;
        self.menu_index = None;
        cx.notify();
    }

    fn focus_content(&self, w: &mut Window, cx: &App) {
        match &self.active {
            Page::Terminal { view, .. } => view.read(cx).focus_handle(cx).focus(w),
            _ => self.tab_focus.focus(w),
        }
    }

    fn focus_tab(&mut self, page: Page, w: &mut Window, cx: &mut Context<Self>) {
        self.select(page, cx);
        self.focus_content(w, cx);
        if !w.is_window_active() {
            w.activate_window();
        }
    }

    fn show_page(&mut self, page: Page, w: &mut Window, cx: &mut Context<Self>) {
        if !self.tabs.contains(&page) {
            if let Some(other) = self.other_window_with(w, cx, |p| *p == page)
                && other
                    .update(cx, |v, w, cx| v.focus_tab(page.clone(), w, cx))
                    .is_ok()
            {
                return;
            }
            if page == Page::Settings {
                self.dashboard.update(cx, |d, _| d.name_field = None);
            }
            self.tabs.push(page.clone());
            self.refresh_title(w);
        }
        self.focus_tab(page, w, cx);
    }

    pub(super) fn open_terminal(&mut self, peer: &str, w: &mut Window, cx: &mut Context<Self>) {
        let matches =
            |p: &Page| matches!(p, Page::Terminal { peer: name, .. } if name.as_ref() == peer);
        let existing = self.tabs.iter().position(matches);
        if existing.is_none()
            && let Some(other) = self.other_window_with(w, cx, matches)
            && other
                .update(cx, |v, w, cx| v.open_terminal(peer, w, cx))
                .is_ok()
        {
            return;
        }
        if let Some(index) = existing
            && let Page::Terminal { view, .. } = &self.tabs[index]
            && view.read(cx).is_running()
        {
            self.focus_tab(self.tabs[index].clone(), w, cx);
            return;
        }
        let ipc = self.dashboard.read(cx).address.clone();
        match terminal::Terminal::open(peer, ipc.as_deref(), w.viewport_size(), cx) {
            Ok(view) => {
                let page = Page::Terminal {
                    peer: peer.to_owned().into(),
                    view,
                };
                if let Some(index) = existing {
                    self.tabs[index] = page.clone();
                } else {
                    self.tabs.push(page.clone());
                }
                self.focus_tab(page, w, cx);
                self.refresh_title(w);
            }
            Err(e) => self.dashboard.update(cx, |d, cx| {
                d.action_error = Some(format!("Open terminal for {peer}: {e:#}"));
                cx.notify();
            }),
        }
    }

    fn take_tab(&mut self, page: &Page, w: &mut Window, cx: &mut Context<Self>) {
        self.dragged_tab = None;
        self.tab_indent = px(0.);
        self.tabs.retain(|p| p != page);
        self.tab_layout.tabs.retain(|(p, _)| p != page);
        self.menu_index = None;
        if let Some(remaining) = self.tabs.first() {
            if self.active == *page {
                self.active = remaining.clone();
                self.focus_content(w, cx);
            }
            self.refresh_title(w);
        } else {
            w.remove_window();
        }
        cx.notify();
    }

    fn close_tab(&mut self, page: &Page, w: &mut Window, cx: &mut Context<Self>) {
        if *page == Page::Settings {
            self.dashboard.update(cx, |d, cx| {
                d.name_field = None;
                cx.notify()
            });
        }
        // If the user already closed the separate Home window, keep Home reachable.
        if self.tabs.len() == 1
            && self
                .other_window_with(w, cx, |p| *p == Page::Home)
                .is_none()
        {
            self.tabs.push(Page::Home);
        }
        self.take_tab(page, w, cx);
    }

    #[cfg(not(windows))]
    fn detach(
        &mut self,
        page: Page,
        placement: Option<DropPlacement>,
        tab_indent: Pixels,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<WindowHandle<Self>> {
        if self.tabs.len() < 2 || !self.tabs.contains(&page) {
            return None;
        }
        let handle = self.open_tab_window(page.clone(), placement, tab_indent, cx)?;
        self.take_tab(&page, w, cx);
        Some(handle)
    }

    fn open_tab_window(
        &self,
        page: Page,
        placement: Option<DropPlacement>,
        tab_indent: Pixels,
        cx: &mut Context<Self>,
    ) -> Option<WindowHandle<Self>> {
        match Self::open(
            self.dashboard.clone(),
            page.clone(),
            self.sidebar,
            WorkspaceWindow::Detached {
                placement,
                tab_indent,
            },
            cx,
        ) {
            Ok(handle) => {
                let _ = handle.update(cx, |v, w, cx| {
                    #[cfg(windows)]
                    drag::prepare_detached_window(w);
                    v.focus_tab(page, w, cx);
                });
                Some(handle)
            }
            Err(e) => {
                self.dashboard.update(cx, |d, cx| {
                    d.action_error = Some(format!("Open window: {e}"));
                    cx.notify();
                });
                None
            }
        }
    }

    fn close_glyph() -> Div {
        div()
            .size(px(20.))
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .rounded_sm()
            .child("×")
    }

    fn tab_face(page: &Page, selected: bool, close: impl IntoElement) -> Div {
        div()
            .flex_none()
            .flex()
            .items_center()
            .justify_between()
            .gap_2()
            .rounded_md()
            .px_3()
            .h(px(31.))
            .min_w(px(96.))
            .text_sm()
            .font_weight(FontWeight::MEDIUM)
            .border_1()
            .border_color(gpui::transparent_black())
            .text_color(rgb(MUTED))
            .when(selected, |tab| {
                tab.bg(rgb(0xffffff))
                    .border_color(rgb(BORDER))
                    .text_color(rgb(0x12584e))
            })
            .child(page.icon().label(page.label()))
            .when(*page != Page::Home, |tab| tab.child(close))
    }

    fn tab_axis(&self) -> Axis {
        if self.sidebar {
            Axis::Vertical
        } else {
            Axis::Horizontal
        }
    }

    fn settle_tab(&mut self, cx: &mut Context<Self>) {
        if self.tab_indent != px(0.) {
            self.tab_indent = px(0.);
            cx.notify();
        }
    }

    fn tab_bar(&self, w: &Window, cx: &mut Context<Self>) -> Stateful<Div> {
        let source = w
            .window_handle()
            .downcast::<Self>()
            .expect("workspace root");
        let view = cx.weak_entity();
        let pages = self.dragged_tab.as_ref().map_or_else(
            || self.tabs.clone(),
            |drag| drag.order(&self.tabs, &self.tab_layout, self.tab_axis()),
        );
        let mut strip = div();
        for page in &pages {
            let closing = page.clone();
            let pressed = page.clone();
            let workspace = cx.weak_entity();
            let close = Self::close_glyph()
                .id("close-tab")
                .cursor_pointer()
                .hover(|s| s.bg(rgb(0xd5e4df)))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(cx.listener(move |v, _, w, cx| {
                    cx.stop_propagation();
                    v.close_tab(&closing, w, cx);
                }));
            let tab = Self::tab_face(page, self.active == *page, close)
                .id(page.element_id())
                .when(self.tab_indent > px(0.), |tab| {
                    if self.sidebar {
                        tab.mt(self.tab_indent)
                    } else {
                        tab.ml(self.tab_indent)
                    }
                })
                .cursor_pointer()
                .when(self.active == *page, |tab| {
                    tab.track_focus(&self.tab_focus).on_key_down(|e, w, cx| {
                        let key = &e.keystroke;
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
                    })
                })
                .when(
                    self.dragged_tab
                        .as_ref()
                        .is_some_and(|drag| drag.page == *page),
                    |tab| tab.opacity(0.),
                )
                .hover(|s| s.bg(rgb(0xf8fbfa)))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |v, e: &MouseDownEvent, w, cx| {
                        v.drag_watch = None;
                        v.tab_press = Some(TabPress {
                            position: e.position,
                            previous_active: v.active.clone(),
                        });
                        v.focus_tab(pressed.clone(), w, cx);
                    }),
                )
                .on_drag(
                    DraggedTab {
                        page: page.clone(),
                        source,
                    },
                    move |drag, offset, w, cx| {
                        let cursor = w.mouse_position();
                        let _ = workspace.update(cx, |v, cx| {
                            let bounds = v.tab_layout.bounds(&drag.page)?;
                            let (grab, previous_active) = v.tab_press.take().map_or_else(
                                || (offset, v.active.clone()),
                                |press| (press.position - bounds.origin, press.previous_active),
                            );
                            v.dragged_tab = Some(TabDrag {
                                page: drag.page.clone(),
                                previous_active,
                                offset: grab,
                                cursor,
                            });
                            cx.notify();
                            Some(())
                        });
                        // GPUI installs its drag after this callback. Process the first
                        // movement as well, even if the next input is already mouse-up.
                        let source = workspace.clone();
                        let drag = drag.clone();
                        w.defer(cx, move |w, cx| {
                            let _ =
                                source.update(cx, |v, cx| v.update_tab_drag(drag, cursor, w, cx));
                        });
                        cx.new(|_| Empty)
                    },
                );
            strip = strip.child(tab);
        }
        strip
            // Freeze the resting slots while reordering. On first entry measure
            // the incoming page together with its neighbours in their actual order.
            .when(
                self.dragged_tab
                    .as_ref()
                    .is_none_or(|tab| self.tab_layout.bounds(&tab.page).is_none()),
                |d| {
                    d.on_children_prepainted(move |bounds, _, cx| {
                        let _ = view.update(cx, |v, _| {
                            v.tab_layout.tabs.clear();
                            v.tab_layout.tabs.extend(pages.iter().cloned().zip(bounds));
                        });
                    })
                },
            )
            .id("workspace-tab-list")
            .flex()
            .min_w_0()
            .when(!self.sidebar, |d| {
                d.overflow_x_scroll().items_center().h_full()
            })
            .gap_1()
            .bg(rgb(0xe3ebeb))
            .p_1()
            .when(self.sidebar, |d| {
                d.flex_col().w(px(SIDEBAR_WIDTH)).flex_none()
            })
            .on_drag_move::<DraggedTab>(cx.listener(Self::move_tab_drag))
            .capture_any_mouse_up(cx.listener(Self::release_tab))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::release_tab))
    }

    fn tab_preview(&self, cx: &Context<Self>) -> Canvas<Option<AnyElement>> {
        let view = cx.weak_entity();
        canvas(
            move |_, w, cx| {
                // The strip and its available caption space have been measured
                // earlier in this prepaint, even on the first frame after docking.
                let workspace = view.upgrade()?;
                let v = workspace.read(cx);
                let drag = v.dragged_tab.as_ref()?;
                let zone = v.tab_layout.zone;
                let bounds = drag.projected(
                    v.tab_layout.bounds(&drag.page)?,
                    &v.tab_layout,
                    v.tab_axis(),
                );
                let mut preview = div()
                    .size_full()
                    .overflow_hidden()
                    .child(
                        Self::tab_face(&drag.page, true, Self::close_glyph())
                            .absolute()
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

    fn active_panel(&self, cx: &mut Context<Self>) -> AnyElement {
        let workspace = cx.weak_entity();
        let (scroll_id, content) = match &self.active {
            Page::Home => (
                "home-scroll",
                self.dashboard.update(cx, |d, cx| d.home(workspace, cx)),
            ),
            Page::Settings => (
                "settings-scroll",
                self.dashboard.update(cx, |d, cx| d.settings(cx)),
            ),
            Page::Terminal { view, .. } => {
                return div()
                    .flex_1()
                    .min_h_0()
                    .min_w_0()
                    .overflow_hidden()
                    .child(view.clone())
                    .into_any_element();
            }
        };
        div()
            .id(scroll_id)
            .flex_1()
            .min_h_0()
            .min_w_0()
            .overflow_y_scroll()
            .p_4()
            .child(content)
            .into_any_element()
    }
}

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Wayland returns pointer focus after the compositor finishes its move.
        #[cfg(any(target_os = "linux", target_os = "freebsd"))]
        if self.tab_indent > px(0.) && cx.compositor_name() == "Wayland" {
            if !window.is_window_hovered() {
                self.wayland_move_left_surface = true;
            } else if self.wayland_move_left_surface {
                self.tab_indent = px(0.);
                self.wayland_move_left_surface = false;
            }
        }
        let data = self.dashboard.read(cx);
        let connected = data.status.is_some();
        let error = data.action_error.clone();
        let view = cx.weak_entity();
        let status = div()
            .w_full()
            .h_full()
            .pr_2()
            .flex()
            .justify_end()
            .items_center()
            .gap_2()
            .text_sm()
            .when(!self.sidebar, |d| {
                let view = view.clone();
                // This spacer is the part of the window caption available to tabs.
                d.child(
                    canvas(
                        move |space, _, cx| {
                            let _ = view.update(cx, |v, _| {
                                v.tab_layout.zone = Bounds::from_corners(
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
                )
            })
            .when(window.viewport_size().width >= px(660.), |d| {
                d.when_some(data.status.as_ref(), |d, s| {
                    d.child(div().truncate().child(s.display_name().to_owned()))
                })
            })
            .child(
                div()
                    .flex_none()
                    .size(px(7.))
                    .rounded_full()
                    .bg(rgb(if connected { 0x298765 } else { 0xb77b29 })),
            )
            .when(window.viewport_size().width >= px(540.), |d| {
                d.child(if connected {
                    "Online"
                } else if data.error.is_some() {
                    "Offline"
                } else {
                    "Connecting…"
                })
            });
        let leading = if self.sidebar {
            div()
                .pl_3()
                .font_weight(FontWeight::SEMIBOLD)
                .child("P2P Tunnel")
                .into_any_element()
        } else {
            self.tab_bar(window, cx).into_any_element()
        };
        let actions = self.menu_button(window, cx);
        let title = chrome::titlebar(leading, status, actions, window, cx);
        let content = div()
            .on_action(cx.listener(|v, _: &ReconnectTerminal, w, cx| {
                if let Page::Terminal { peer, .. } = v.active.clone() {
                    v.open_terminal(&peer, w, cx);
                }
            }))
            .capture_key_down(cx.listener(|v, e: &KeyDownEvent, w, cx| {
                if e.keystroke.key == "escape" && v.cancel_tab_drag(w, cx) {
                    w.prevent_default();
                    cx.stop_propagation();
                }
            }))
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(SURFACE))
            .text_color(rgb(INK))
            .child(title)
            .when_some(error, |d, e| {
                d.child(
                    div()
                        .px_4()
                        .py_2()
                        .bg(rgb(0xffe6dd))
                        .text_color(rgb(0x813721))
                        .text_sm()
                        .child(e),
                )
            })
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .min_w_0()
                    .flex()
                    .when(self.sidebar, |d| {
                        d.child(self.tab_bar(window, cx)).on_children_prepainted(
                            move |children, _, cx| {
                                if let Some(&bounds) = children.first() {
                                    let _ = view.update(cx, |v, _| v.tab_layout.zone = bounds);
                                }
                            },
                        )
                    })
                    .child(self.active_panel(cx)),
            );
        let content = content
            .relative()
            .when(self.dragged_tab.is_some() && self.tabs.len() > 1, |d| {
                d.child(self.tab_preview(cx))
            });
        chrome::frame(content, window)
    }
}
