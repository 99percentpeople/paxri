//! A workspace window owns its tabs; detached windows share the same gateway controller.
use super::*;
use crate::components::{
    DropdownState, Icon, Ui,
    dock::{
        DockDrag, DockHeader, DockLayout, DockPanel, DockPlacement, DockSettings, DockState,
        DockTabs,
    },
    tooltip,
};

mod dock;
mod drag;
mod menu;

gpui::actions!(workspace, [ReconnectTerminal]);

use dock::DockTarget;
#[cfg(target_os = "macos")]
pub(super) use drag::{DragEvent, DragSignal};
use drag::{DragWatch, DropPlacement};

type TabDrag = DockDrag<Page>;

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

    fn tab(&self, ui: Ui, settings: DockSettings, selected: bool, close: impl IntoElement) -> Div {
        ui.tab(
            settings,
            self.icon(),
            self.label(),
            selected,
            (*self != Self::Home).then_some(close),
        )
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
    dock: Entity<DockState<Page>>,
    menu: Entity<DropdownState>,
    tab_focus: FocusHandle,
    dragged_tab: Option<TabDrag>,
    tab_press: Option<TabPress>,
    tab_indent: Pixels,
    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    wayland_move_left_surface: bool,
    drag_watch: Option<DragWatch>,
}

impl Workspace {
    pub(super) fn open_home(
        dashboard: Entity<Dashboard>,
        cx: &mut App,
    ) -> anyhow::Result<WindowHandle<Self>> {
        Self::open(
            dashboard,
            Page::Home,
            DockSettings::default(),
            WorkspaceWindow::Initial,
            cx,
        )
    }

    fn open(
        dashboard: Entity<Dashboard>,
        page: Page,
        settings: DockSettings,
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
            theme::observe(w);
            #[cfg(target_os = "macos")]
            chrome::hide_system_buttons(w);
            cx.new(|cx| {
                cx.observe(&dashboard, |_, _, cx| cx.notify()).detach();
                let menu = DropdownState::new(w, cx);
                menu.read(cx).focus_handle(cx).focus(w);
                cx.observe_window_activation(w, |v: &mut Self, w, cx| {
                    if !w.is_window_active() {
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
                    dock: DockState::new(settings, w, cx),
                    menu,
                    tab_focus: cx.focus_handle().tab_stop(true),
                    dragged_tab: None,
                    tab_press: None,
                    tab_indent,
                    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
                    wayland_move_left_surface: false,
                    drag_watch: None,
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
        self.menu.update(cx, DropdownState::dismiss);
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
        self.dock.update(cx, |dock, _| dock.forget(page));
        self.menu.update(cx, DropdownState::dismiss);
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
            self.dock.read(cx).settings(),
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

    fn settle_tab(&mut self, cx: &mut Context<Self>) {
        if self.tab_indent != px(0.) {
            self.tab_indent = px(0.);
            cx.notify();
        }
    }

    fn tab_bar(&self, ui: Ui, w: &Window, cx: &Context<Self>) -> DockTabs<Page> {
        let source = w
            .window_handle()
            .downcast::<Self>()
            .expect("workspace root");
        let mut strip = DockTabs::new("workspace-tab-list", &self.dock).indent(self.tab_indent);
        let settings = self.dock.read(cx).settings();
        for page in &self.tabs {
            let middle_close = page.clone();
            let closing = page.clone();
            let pressed = page.clone();
            let workspace = cx.weak_entity();
            let close =
                ui.tab_close_button(cx.listener(move |v, _, w, cx| v.close_tab(&closing, w, cx)));
            let tab = page
                .tab(ui, settings, self.active == *page, close)
                .id(page.element_id())
                .tooltip(tooltip(page.label()))
                .when(*page != Page::Home, |tab| {
                    tab.on_mouse_down(
                        MouseButton::Middle,
                        cx.listener(move |v, _, w, cx| {
                            cx.stop_propagation();
                            v.close_tab(&middle_close, w, cx);
                        }),
                    )
                })
                .when(self.active == *page, |tab| tab.track_focus(&self.tab_focus))
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
                        let _ = workspace.update(cx, |v, cx| v.begin_tab_drag(drag, offset, w, cx));
                        cx.new(|_| Empty)
                    },
                );
            strip = strip.item(page.clone(), tab);
        }
        strip
            .on_drag_move::<DraggedTab>(cx.listener(Self::move_tab_drag))
            .capture_any_mouse_up(cx.listener(Self::release_tab))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::release_tab))
    }

    fn active_panel(&self, ui: Ui, cx: &mut Context<Self>) -> DockPanel {
        let workspace = cx.weak_entity();
        match &self.active {
            Page::Home => DockPanel::scroll(
                "home-scroll",
                self.dashboard.update(cx, |d, cx| d.home(ui, workspace, cx)),
            ),
            Page::Settings => DockPanel::scroll(
                "settings-scroll",
                self.dashboard.update(cx, |d, cx| d.settings(ui, cx)),
            ),
            Page::Terminal { view, .. } => DockPanel::fill(view.clone()),
        }
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
        let ui = Ui::new(window, cx);
        let data = self.dashboard.read(cx);
        let connected = data.status.is_some();
        let error = data.action_error.clone();
        let status = div()
            .min_w_0()
            .flex()
            .items_center()
            .gap_2()
            .when(window.viewport_size().width >= px(660.), |d| {
                d.when_some(data.status.as_ref(), |d, s| {
                    d.child(div().truncate().child(s.display_name().to_owned()))
                })
            })
            .child(ui.status_dot(connected))
            .when(window.viewport_size().width >= px(540.), |d| {
                d.child(if connected {
                    "Online"
                } else if data.error.is_some() {
                    "Offline"
                } else {
                    "Connecting…"
                })
            });
        let header = DockHeader::new("P2P Tunnel", status, self.workspace_menu(ui, cx));
        let tabs = self.tab_bar(ui, window, cx);
        let panel = self.active_panel(ui, cx);
        DockLayout::new(header, tabs, panel)
            .when_some(error, |dock, message| dock.banner(ui.alert(message)))
            .when_some(self.dragged_tab.as_ref(), |dock, drag| {
                dock.dragging(
                    drag.clone(),
                    drag.page
                        .tab(ui, self.dock.read(cx).settings(), true, ui.tab_close()),
                )
            })
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
    }
}
