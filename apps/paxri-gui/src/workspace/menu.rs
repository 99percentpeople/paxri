use super::*;
use crate::components::ButtonVariant::Ghost;
use paxri_ui::components::directory::SessionLauncher;

impl Workspace {
    pub(super) fn tab_session_launcher(&self, ui: Ui, cx: &Context<Self>) -> SessionLauncher {
        let peers: Vec<_> =
            self.dashboard
                .read(cx)
                .status
                .as_ref()
                .map_or_else(Vec::new, |status| {
                    status
                        .peers_by_last_change()
                        .into_iter()
                        .filter(|(peer, _)| status.can_open_shell(peer))
                        .map(|(peer, _)| {
                            (
                                peer.clone(),
                                status.peer_display_name(peer).to_owned(),
                                status.peer_platform(peer),
                            )
                        })
                        .collect()
                });
        let hint = if peers.is_empty() {
            "No devices available for a terminal session"
        } else {
            "New terminal session"
        };
        let button = match self.dock.read(cx).placement() {
            DockPlacement::Top => ui.icon_button(Ghost, "new-session-tab", Icon::Plus, hint),
            DockPlacement::Left => ui
                .button(Ghost, "new-session-tab", "New session")
                .icon(Icon::Plus)
                .row()
                .tooltip(tooltip(hint.into())),
        };
        SessionLauncher::new(
            button,
            peers,
            cx.listener(|v, peer: &String, w, cx| {
                v.new_terminal(peer.clone().into(), w, cx);
            }),
        )
    }

    pub(super) fn titlebar_actions(&self, ui: Ui, cx: &Context<Self>) -> Div {
        div()
            .flex()
            .flex_none()
            .px_1()
            .when(
                matches!(&self.active, Page::Terminal { view, .. } if view.read(cx).is_running()),
                |row| {
                    row.child(
                        ui.end_session_button()
                            .on_click(cx.listener(|this, _, _, cx| {
                                if let Page::Terminal { view, .. } = &this.active {
                                    view.update(cx, |terminal, _| terminal.end());
                                }
                            })),
                    )
                },
            )
            .when(self.active != Page::Home, |row| {
                row.child(
                    ui.icon_button(Ghost, "open-sessions", Icon::Terminal, "Sessions")
                        .on_click(cx.listener(|v, _, w, cx| v.show_sessions(None, w, cx))),
                )
            })
            .when(!self.tabs.contains(&Page::Home), |row| {
                row.child(
                    ui.icon_button(Ghost, "open-devices", Icon::Monitor, "Open Devices")
                        .on_click(cx.listener(|v, _, w, cx| v.show_page(Page::Home, w, cx))),
                )
            })
            .child(
                ui.icon_button(Ghost, "open-settings", Icon::Settings, "Settings")
                    .on_click(cx.listener(|v, _, w, cx| v.show_page(Page::Settings, w, cx))),
            )
    }
}
