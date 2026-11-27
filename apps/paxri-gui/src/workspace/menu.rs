use super::*;
use crate::{
    components::ButtonVariant::Ghost,
    theme::{TextRole, TypographyExt},
};
use paxri_control::DevicePlatform;
use paxri_ui::components::{
    Dropdown, DropdownContent, DropdownState, DropdownTrigger, directory::SessionLocation,
};

struct RestorableSession {
    key: SessionKey,
    title: SharedString,
}

struct DeviceLauncherGroup {
    peer: SharedString,
    name: String,
    platform: DevicePlatform,
    restorable: Vec<RestorableSession>,
}

#[derive(IntoElement)]
pub(super) struct TabSessionLauncher {
    workspace: WeakEntity<Workspace>,
    dashboard: WeakEntity<Dashboard>,
    placement: DockPlacement,
    groups: Vec<DeviceLauncherGroup>,
    has_unlisted_peer: bool,
}

impl RenderOnce for TabSessionLauncher {
    fn render(mut self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let ui = Ui::new(window, cx);
        let has_restorable = self.groups.iter().any(|group| !group.restorable.is_empty());
        let hint = if self.groups.is_empty() {
            "No devices available for a terminal session"
        } else if has_restorable {
            "New or restore terminal session"
        } else {
            "New terminal session"
        };
        let button = match self.placement {
            DockPlacement::Top => ui.icon_button(Ghost, "new-session-tab", Icon::Plus, hint),
            DockPlacement::Left => ui
                .button(Ghost, "new-session-tab", "New session")
                .icon(Icon::Plus)
                .row()
                .tooltip(tooltip(hint.into())),
        };
        let state = DropdownState::local("tab-session-menu".into(), window, cx);
        if self.groups.len() == 1 && !has_restorable && !self.has_unlisted_peer {
            let group = self.groups.remove(0);
            if state.read(cx).is_open() {
                state.update(cx, DropdownState::dismiss);
            }
            let workspace = self.workspace;
            return button
                .on_click(move |_, w, cx| {
                    let _ = workspace.update(cx, |v, cx| v.new_terminal(group.peer.clone(), w, cx));
                })
                .into_any_element();
        }
        let mut menu = DropdownContent::new();
        let mut prev_had_sessions = false;
        for (index, group) in self.groups.into_iter().enumerate() {
            let group_has_sessions = !group.restorable.is_empty();
            if index > 0 && (prev_had_sessions || group_has_sessions) {
                menu = menu.separator();
            }
            prev_had_sessions = group_has_sessions;
            let device_row = div()
                .w_full()
                .min_w_0()
                .flex()
                .items_center()
                .justify_between()
                .gap_2()
                .child(
                    Icon::device(group.platform)
                        .label(group.name)
                        .flex_auto()
                        .text_role(TextRole::Label),
                )
                .child(
                    div()
                        .flex_none()
                        .flex()
                        .items_center()
                        .gap(px(3.))
                        .text_role(TextRole::Caption)
                        .text_color(rgb(ui.colors.muted_foreground))
                        .child(Icon::Plus.draw(12.))
                        .child("New"),
                );
            let workspace = self.workspace.clone();
            let new_peer = group.peer.clone();
            menu = menu.item(
                SharedString::from(format!("new-{}", group.peer)),
                device_row,
                move |w, cx| {
                    let _ = workspace.update(cx, |v, cx| v.new_terminal(new_peer.clone(), w, cx));
                },
            );
            for session in group.restorable {
                let workspace = self.workspace.clone();
                let peer = group.peer.clone();
                let key = session.key;
                let label = div()
                    .w_full()
                    .min_w_0()
                    .pl(crate::theme::SPACE_3)
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .child(Icon::Terminal.label(session.title).flex_auto())
                    .child(ui.caption(format!("#{}", key.number)).flex_none());
                menu = menu.item(
                    SharedString::from(format!("restore-{peer}-{}-{}", key.number, key.nonce)),
                    label,
                    move |w, cx| {
                        let _ = workspace
                            .update(cx, |v, cx| v.show_terminal(peer.clone(), key, w, cx));
                    },
                );
            }
        }
        let button = button.pressed(state.read(cx).is_open());
        let dashboard = self.dashboard;
        Dropdown::new(&state, DropdownTrigger::new(button), menu)
            .align_start()
            .on_open(move |_, cx| {
                let _ = dashboard.update(cx, |d, cx| d.refresh_hosted_sessions(true, cx));
            })
            .into_any_element()
    }
}

impl Workspace {
    pub(super) fn tab_session_launcher(&self, _ui: Ui, cx: &Context<Self>) -> TabSessionLauncher {
        let dashboard = self.dashboard.read(cx);
        let mut groups = Vec::new();
        let mut has_unlisted_peer = false;
        if let Some(status) = dashboard.status.as_ref() {
            for (peer, _) in status.peers_by_last_change() {
                if !status.can_open_shell(peer) {
                    continue;
                }
                let name = status.peer_display_name(peer).to_owned();
                let platform = status.peer_platform(peer);
                let peer_key: SharedString = peer.clone().into();
                if status
                    .peer_capabilities
                    .get(peer)
                    .is_some_and(|c| c.managed_terminals)
                {
                    let listed = dashboard
                        .terminal_sessions
                        .get(&peer_key)
                        .is_some_and(|s| s.listed);
                    has_unlisted_peer |= !listed;
                }
                let restorable = dashboard
                    .open_sessions(&peer_key, cx)
                    .into_iter()
                    .filter_map(|(key, title, location)| {
                        (location == SessionLocation::Gateway)
                            .then_some(RestorableSession { key, title })
                    })
                    .collect();
                groups.push(DeviceLauncherGroup {
                    peer: peer_key,
                    name,
                    platform,
                    restorable,
                });
            }
        }
        TabSessionLauncher {
            workspace: cx.weak_entity(),
            dashboard: self.dashboard.downgrade(),
            placement: self.dock.read(cx).placement(),
            groups,
            has_unlisted_peer,
        }
    }

    pub(super) fn titlebar_actions(&self, ui: Ui, cx: &Context<Self>) -> Div {
        div()
            .flex()
            .flex_none()
            .px_1()
            .when_some(
                match &self.active {
                    Page::Terminal { view, .. } if view.read(cx).is_running() => {
                        Some(view.read(cx).action())
                    }
                    _ => None,
                },
                |row, action| {
                    let button = match action {
                        paxri_ui::terminal::lifecycle::SessionAction::End => {
                            ui.end_session_button()
                        }
                        paxri_ui::terminal::lifecycle::SessionAction::Cancel => {
                            ui.icon_button(Ghost, "cancel-session", Icon::Close, "Cancel session")
                        }
                        paxri_ui::terminal::lifecycle::SessionAction::Close => {
                            ui.icon_button(Ghost, "close-session", Icon::Close, "Close session")
                        }
                    };
                    row.child(button.on_click(cx.listener(|this, _, _, cx| {
                        if let Page::Terminal { view, .. } = &this.active {
                            view.update(cx, |terminal, cx| terminal.end(cx));
                        }
                    })))
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
