use super::*;
use crate::components::{
    Button, ButtonVariant::Ghost, Dropdown, DropdownContent, DropdownTrigger, single_line,
};

impl Workspace {
    pub(super) fn new_session_menu(&self, ui: Ui, cx: &Context<Self>) -> Option<Dropdown<Button>> {
        let status = self.dashboard.read(cx).status.as_ref()?;
        let mut peers = status
            .peers_by_last_change()
            .into_iter()
            .filter(|(name, _)| status.can_open_shell(name))
            .peekable();
        peers.peek()?;
        let dock = self.dock.read(cx);
        let button = if dock.placement() == DockPlacement::Left {
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
        let mut content = DropdownContent::new();
        for (name, _) in peers {
            let peer = SharedString::from(name.clone());
            let label = Icon::device(status.peer_platform(name)).label(status.peer_display_name(name).to_owned());
            let workspace = cx.weak_entity();
            content = content.item(peer.clone(), label, move |window, cx| {
                let _ =
                    workspace.update(cx, |view, cx| view.new_terminal(peer.clone(), window, cx));
            });
        }
        Some(Dropdown::new(&self.session_menu, trigger, content).align_start())
    }

    pub(super) fn titlebar_actions(&self, ui: Ui, cx: &Context<Self>) -> Div {
        div()
            .flex()
            .flex_none()
            .px_1()
            .when(!self.tabs.contains(&Page::Home), |row| {
                row.child(
                    ui.icon_button(Ghost, "open-devices", Icon::Home, "Open Devices")
                        .on_click(cx.listener(|v, _, w, cx| v.show_page(Page::Home, w, cx))),
                )
            })
            .child(
                ui.icon_button(Ghost, "open-settings", Icon::Settings, "Settings")
                    .on_click(cx.listener(|v, _, w, cx| v.show_page(Page::Settings, w, cx))),
            )
    }
}
