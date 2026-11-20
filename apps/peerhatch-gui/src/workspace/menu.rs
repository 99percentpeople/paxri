use super::*;
use crate::components::{
    Button, ButtonVariant::Ghost, Dropdown, DropdownContent, DropdownTrigger, single_line,
};

impl Workspace {
    pub(super) fn dismiss_menus(&self, cx: &mut Context<Self>) {
        self.menu.update(cx, DropdownState::dismiss);
        self.session_menu.update(cx, DropdownState::dismiss);
    }

    pub(super) fn new_session_menu(&self, ui: Ui, cx: &Context<Self>) -> Dropdown<Button> {
        let dock = self.dock.read(cx);
        let button = if dock.placement() == DockPlacement::Left && !dock.settings().compact() {
            ui.button(
                Ghost,
                "new-session",
                single_line(div()).child("New session"),
            )
            .icon(Icon::Plus)
            .row()
            .tooltip(tooltip("New terminal session".into()))
        } else {
            ui.icon_button(Ghost, "new-session", Icon::Plus, "New terminal session")
        };
        let trigger = DropdownTrigger::new(button.pressed(self.session_menu.read(cx).is_open()));
        let status = self.dashboard.read(cx).status.as_ref();
        let mut content = DropdownContent::new();
        if let Some(status) = status.filter(|s| !s.peers.is_empty()) {
            for (name, _) in status.peers_by_last_change() {
                let peer = SharedString::from(name.clone());
                let label = Icon::device(status.peer_platform(name)).label(peer.clone());
                let workspace = cx.weak_entity();
                content = content.item(peer.clone(), label, move |window, cx| {
                    let _ = workspace
                        .update(cx, |view, cx| view.new_terminal(peer.clone(), window, cx));
                });
            }
        } else {
            let (icon, label, page) = if status.is_some() {
                (Icon::Home, "Pair a device…", Page::Home)
            } else {
                (Icon::Settings, "Open gateway settings…", Page::Settings)
            };
            let workspace = cx.weak_entity();
            content = content.item("session-setup", icon.label(label), move |window, cx| {
                let _ = workspace.update(cx, |view, cx| view.show_page(page.clone(), window, cx));
            });
        }
        Dropdown::new(&self.session_menu, trigger, content).align_start()
    }

    pub(super) fn workspace_menu(&self, ui: Ui, cx: &Context<Self>) -> Div {
        let trigger = DropdownTrigger::new(
            ui.icon_button(Ghost, "workspace-menu", Icon::Menu, "Menu")
                .pressed(self.menu.read(cx).is_open()),
        );
        let show = |page: Page| {
            let workspace = cx.weak_entity();
            move |window: &mut Window, cx: &mut App| {
                let _ = workspace.update(cx, |view, cx| view.show_page(page.clone(), window, cx));
            }
        };
        let mut content = DropdownContent::new().item(
            "settings",
            Icon::Settings.label("Settings"),
            show(Page::Settings),
        );
        if !self.tabs.contains(&Page::Home) {
            content = content.item("home", Icon::Home.label("Open Home"), show(Page::Home));
        }
        let placement = self.dock.read(cx).placement().other();
        let label = match placement {
            DockPlacement::Top => Icon::TabsOnTop.label("Tabs on top"),
            DockPlacement::Left => Icon::TabsOnLeft.label("Tabs on left"),
        };
        let dock = self.dock.clone();
        let active = self.active.clone();
        let content = content.item("tab-placement", label, move |_, cx| {
            dock.update(cx, |dock, cx| dock.set_placement(placement, &active, cx));
        });
        div()
            .flex_none()
            .px_1()
            .child(Dropdown::new(&self.menu, trigger, content))
    }
}
