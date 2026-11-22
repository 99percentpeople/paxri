//! A workspace window owns its tabs; detached windows share the same gateway controller.
use super::*;
use crate::components::{
    DropdownState, Icon, Ui,
    dock::{DockDrag, DockLayout, DockPanel, DockPlacement, DockSettings, DockState, DockTabs},
    observe_window_focus, tooltip,
};

mod dock;
mod drag;
mod menu;
mod rename;
mod terminals;
use rename::{EndRename, TabRename};
pub(super) use terminals::PeerSessions;

gpui::actions!(workspace, [ReconnectTerminal]);

#[cfg(not(target_os = "macos"))]
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
        number: usize,
        view: Entity<terminal::Terminal>,
        title: Entity<terminal::TabTitle>,
    },
}

impl Page {
    fn terminal(
        peer: SharedString,
        number: usize,
        view: Entity<terminal::Terminal>,
        cx: &App,
    ) -> Self {
        // The tab strip observes title changes independently of terminal screen updates.
        let title = view.read(cx).title.clone();
        Self::Terminal {
            peer,
            number,
            view,
            title,
        }
    }

    fn shell_exited(&self, cx: &App) -> bool {
        matches!(self, Self::Terminal { view, .. } if view.read(cx).has_exited())
    }

    fn window_title(&self) -> SharedString {
        match self {
            Self::Home => "PeerHatch".into(),
            Self::Settings => "Settings — PeerHatch".into(),
            Self::Terminal { peer, number, .. } => format!("{peer} — Terminal {number}").into(),
        }
    }

    fn icon(&self) -> Icon {
        match self {
            Self::Home => Icon::Home,
            Self::Settings => Icon::Settings,
            Self::Terminal { .. } => Icon::Terminal,
        }
    }

    fn label(&self, cx: &App) -> SharedString {
        match self {
            Self::Home => "Devices".into(),
            Self::Settings => "Settings".into(),
            Self::Terminal { peer, title, .. } => title.read(cx).label(peer),
        }
    }

    fn tab(
        &self,
        ui: Ui,
        settings: DockSettings,
        selected: bool,
        close: impl IntoElement,
        label: impl IntoElement,
    ) -> Div {
        ui.tab(
            settings,
            self.icon(),
            label,
            selected,
            (*self != Self::Home).then_some(close),
        )
    }

    fn element_id(&self) -> ElementId {
        match self {
            Self::Terminal { view, .. } => ("terminal-tab", view.entity_id()).into(),
            Self::Home => "Devices".into(),
            Self::Settings => "Settings".into(),
        }
    }
}

#[derive(Clone)]
struct DraggedTab {
    page: Page,
    source: WindowHandle<Workspace>,
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
    session_menu: Entity<DropdownState>,
    rename: Option<TabRename>,
    tab_focus: FocusHandle,
    dragged_tab: Option<TabDrag>,
    tab_press: Option<TabDrag>,
    tab_indent: Pixels,
    #[cfg(windows)]
    placing_window: bool,
    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    wayland_move_left_surface: bool,
    drag_watch: Option<DragWatch>,
}

impl Workspace {
    pub(super) fn shows_devices(dashboard: &Entity<Dashboard>, cx: &App) -> bool {
        cx.windows().into_iter().filter_map(|window| window.downcast::<Self>()).any(|window| {
            window.read(cx).is_ok_and(|w| w.active == Page::Home && &w.dashboard == dashboard)
        })
    }

    pub(super) fn open_home(
        dashboard: Entity<Dashboard>,
        cx: &mut App,
    ) -> anyhow::Result<WindowHandle<Self>> {
        Self::open(
            dashboard,
            Page::Home,
            DockSettings::default().with_placement(theme::current(cx).tab_placement),
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
            options.app_id = Some("peerhatch".into());
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
                cx.observe_global_in::<theme::Preferences>(w, |v: &mut Self, w, cx| {
                    let placement = theme::current(cx).tab_placement;
                    if v.dock.read(cx).placement() != placement {
                        v.cancel_tab_drag(w, cx);
                        v.settle_tab(cx);
                        v.dock.update(cx, |dock, cx| {
                            dock.set_placement(placement, &v.active, cx)
                        });
                    }
                })
                .detach();
                let tab_focus = cx.focus_handle().tab_stop(true);
                tab_focus.focus(w);
                observe_window_focus(w, &tab_focus, cx);
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
                    session_menu: DropdownState::new(w, cx),
                    rename: None,
                    tab_focus,
                    dragged_tab: None,
                    tab_press: None,
                    tab_indent,
                    #[cfg(windows)]
                    placing_window: !matches!(kind, WorkspaceWindow::Initial),
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
        if !self.tabs.contains(&page) || page.shell_exited(cx) {
            return;
        }
        #[cfg(windows)]
        if page == Page::Settings {
            self.dashboard.update(cx, |d, cx| d.inspect_startup(cx));
        }
        self.active = page;
        self.session_menu.update(cx, DropdownState::dismiss);
        cx.notify();
    }

    fn focus_content(&self, w: &mut Window, cx: &App) {
        let focus = match &self.active {
            Page::Terminal { view, .. } => view.read(cx).focus_handle(cx),
            _ => self.tab_focus.clone(),
        };
        #[cfg(windows)]
        if !focus.is_focused(w) {
            crate::ime::blur(w);
        }
        focus.focus(w);
    }

    fn focus_tab(&mut self, page: Page, w: &mut Window, cx: &mut Context<Self>) {
        self.select(page, cx);
        self.focus_content(w, cx);
        if !w.is_window_active() {
            w.activate_window();
        }
    }

    fn focus_open_page(&mut self, page: &Page, w: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.tabs.contains(page) {
            self.focus_tab(page.clone(), w, cx);
            return true;
        }
        self.other_window_with(w, cx, |p| p == page)
            .is_some_and(|other| {
                other
                    .update(cx, |v, w, cx| v.focus_tab(page.clone(), w, cx))
                    .is_ok()
            })
    }

    fn show_page(&mut self, page: Page, w: &mut Window, cx: &mut Context<Self>) {
        if self.focus_open_page(&page, w, cx) {
            return;
        }
        if page == Page::Settings {
            self.dashboard.update(cx, |d, cx| {
                d.settings_editor.reset(cx);
            });
        }
        self.append_tab(page, w, cx);
    }

    fn append_tab(&mut self, page: Page, w: &mut Window, cx: &mut Context<Self>) {
        self.dock.read(cx).reveal_tab(self.tabs.len());
        self.tabs.push(page.clone());
        self.focus_tab(page, w, cx);
        self.refresh_title(w);
    }

    fn terminal_exited(view: Entity<terminal::Terminal>, _: &terminal::ShellExited, cx: &mut App) {
        // A tab can change windows while its shell is running.
        for window in cx
            .windows()
            .into_iter()
            .filter_map(|w| w.downcast::<Self>())
        {
            let page = window.read(cx).ok().and_then(|owner| {
                owner
                    .tabs
                    .iter()
                    .find(|p| matches!(p, Page::Terminal { view: tab, .. } if tab == &view))
                    .cloned()
            });
            if let Some(page) = page {
                let _ = window.update(cx, |owner, w, cx| {
                    if owner
                        .dragged_tab
                        .as_ref()
                        .is_some_and(|drag| drag.page == page)
                    {
                        cx.stop_active_drag(w);
                        owner.drag_watch = None;
                        owner.dragged_tab = None;
                        owner.tab_indent = px(0.);
                    }
                    owner.forget_terminal(&page, cx);
                    owner.remove_tab(&page, w, cx);
                });
                break;
            }
        }
    }

    fn take_tab(&mut self, page: &Page, w: &mut Window, cx: &mut Context<Self>) {
        self.dragged_tab = None;
        self.tab_indent = px(0.);
        self.session_menu.update(cx, DropdownState::dismiss);
        self.remove_tab(page, w, cx);
    }

    fn remove_tab(&mut self, page: &Page, w: &mut Window, cx: &mut Context<Self>) {
        if self.rename.as_ref().is_some_and(|edit| edit.matches(page)) {
            self.end_rename(EndRename::Blur, cx);
        }
        self.tabs.retain(|p| p != page);
        self.dock.update(cx, |dock, _| dock.forget(page));
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
        self.forget_terminal(page, cx);
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
        let mut strip = DockTabs::new("workspace-tab-list", &self.dock)
            .indent(self.tab_indent);
        if let Some(menu) = self.new_session_menu(ui, cx) {
            strip = strip.trailing(menu);
        }
        let settings = self.dock.read(cx).settings();
        for page in &self.tabs {
            let editor = self.rename.as_ref().filter(|edit| edit.matches(page));
            let label = match editor {
                Some(edit) => edit.input.clone().into_any_element(),
                None => page.label(cx).into_any_element(),
            };
            let middle_close = page.clone();
            let closing = page.clone();
            let pressed = page.clone();
            let workspace = cx.weak_entity();
            let close =
                ui.tab_close_button(cx.listener(move |v, _, w, cx| v.close_tab(&closing, w, cx)));
            let tab = page
                .tab(ui, settings, self.active == *page, close, label)
                .id(page.element_id())
                .when(*page != Page::Home, |tab| {
                    tab.on_mouse_down(
                        MouseButton::Middle,
                        cx.listener(move |v, _, w, cx| {
                            cx.stop_propagation();
                            v.close_tab(&middle_close, w, cx);
                        }),
                    )
                });
            let tab = if editor.is_some() {
                let dock = self.dock.read(cx);
                tab.when(dock.placement() == DockPlacement::Top, |tab| {
                    tab.when_some(dock.bounds(page), |tab, bounds| tab.w(bounds.size.width))
                })
                .on_key_down(cx.listener(Self::rename_key))
            } else {
                tab.tooltip(tooltip(page.label(cx)))
                    .when(self.active == *page, |tab| tab.track_focus(&self.tab_focus))
                    .when(matches!(page, Page::Terminal { .. }), |tab| {
                        let page = page.clone();
                        tab.on_click(cx.listener(move |v, event: &ClickEvent, w, cx| {
                            if event.standard_click() && event.click_count() == 2 {
                                v.begin_rename(&page, w, cx);
                            }
                        }))
                    })
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |v, e: &MouseDownEvent, w, cx| {
                            v.drag_watch = None;
                            v.tab_press = TabDrag::pressed(
                                pressed.clone(),
                                v.active.clone(),
                                e.position,
                                v.dock.read(cx),
                            );
                            v.focus_tab(pressed.clone(), w, cx);
                        }),
                    )
                    .on_drag(
                        DraggedTab {
                            page: page.clone(),
                            source,
                        },
                        move |drag, _, w, cx| {
                            let _ = workspace.update(cx, |v, cx| v.begin_tab_drag(drag, w, cx));
                            cx.new(|_| Empty)
                        },
                    )
            };
            strip = strip.item(page.clone(), tab);
        }
        strip
            .on_drag_move::<DraggedTab>(cx.listener(Self::move_tab_drag))
            .capture_any_mouse_up(cx.listener(Self::release_tab))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::release_tab))
    }

    fn active_panel(&self, ui: Ui, viewport: Pixels, cx: &mut Context<Self>) -> DockPanel {
        let width = self.dock.read(cx).settings().page_width(viewport);
        match &self.active {
            Page::Home => self.dashboard.update(cx, |d, cx| d.home(ui, width, cx)),
            Page::Settings => self.dashboard.update(cx, |d, cx| d.settings(ui, width, cx)),
            // A new Win32 window is still cloaked while its client size is
            // being set. Laying out the PTY here would resize the live session.
            #[cfg(windows)]
            Page::Terminal { .. } if self.placing_window => DockPanel::fill(gpui::Empty),
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
        let error = self.dashboard.read(cx).action_error.clone();
        let tabs = self.tab_bar(ui, window, cx);
        let panel = self.active_panel(ui, window.viewport_size().width, cx);
        let dock = DockLayout::new("peerhatch", tabs, panel, self.titlebar_actions(ui, cx));
        let dock = dock
            .when(self.active == Page::Settings, |dock| {
                let search = self.dashboard.read(cx).settings_editor.search.clone();
                dock.capture_key_down(move |event, window, cx| {
                    let key = &event.keystroke;
                    let shortcut = Modifiers::secondary_key();
                    if key.modifiers == shortcut && key.key.eq_ignore_ascii_case("f") {
                        search.read(cx).focus_handle(cx).focus(window);
                    } else if key.key == "escape" && !key.modifiers.modified()
                        && search.read(cx).focus_handle(cx).is_focused(window)
                        && !search.read(cx).is_composing() && !search.read(cx).text().is_empty()
                    {
                        search.update(cx, |field, cx| field.set_text("", cx));
                    } else {
                        return;
                    }
                    window.prevent_default();
                    cx.stop_propagation();
                })
            })
            .when_some(error, |dock, message| dock.banner(ui.alert(message)))
            .when_some(self.dragged_tab.as_ref(), |dock, drag| {
                dock.dragging(
                    drag.clone(),
                    drag.page.tab(
                        ui,
                        self.dock.read(cx).settings(),
                        true,
                        ui.tab_close(),
                        drag.page.label(cx),
                    ),
                )
            })
            .on_action(cx.listener(Self::reconnect_terminal))
            .capture_key_down(cx.listener(|v, e: &KeyDownEvent, w, cx| {
                if e.keystroke.key == "escape" && v.cancel_tab_drag(w, cx) {
                    w.prevent_default();
                    cx.stop_propagation();
                }
            }));
        crate::components::Dialog::guard_owner(dock, window, ui, cx)
    }
}
