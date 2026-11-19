use super::{peer_detail, peer_settings::PortsForm, rules::RuleForm};
use crate::{
    Dashboard, GatewayRequest, Workspace,
    components::{
        ButtonVariant::{Ghost, Primary, Secondary},
        Icon, ListItem, Ui, page,
    },
};
use gpui::{prelude::*, *};
use tunnel_control::Command;

pub(crate) struct DeviceSettings {
    pub(crate) scroll: ScrollHandle,
    pub(super) peer: SharedString,
    pub(super) dashboard: Entity<Dashboard>,
    pub(super) rule: Option<RuleForm>,
    pub(super) ports_form: Option<PortsForm>,
    pub(super) removing: Option<String>,
    pub(super) saving: bool,
    pub(super) action_error: Option<String>,
}

impl DeviceSettings {
    pub(crate) fn platform(&self, cx: &App) -> tunnel_control::DevicePlatform {
        self.dashboard
            .read(cx)
            .status
            .as_ref()
            .map(|s| s.peer_platform(&self.peer))
            .unwrap_or_default()
    }

    pub(crate) fn new(
        peer: SharedString,
        dashboard: Entity<Dashboard>,
        cx: &mut App,
    ) -> Entity<Self> {
        cx.new(|cx| {
            cx.observe(&dashboard, |_, _, cx| cx.notify()).detach();
            Self {
                peer,
                scroll: ScrollHandle::new(),
                dashboard,
                rule: None,
                ports_form: None,
                removing: None,
                saving: false,
                action_error: None,
            }
        })
    }

    pub(super) fn send(&mut self, command: Command, cx: &mut Context<Self>) {
        if self.saving {
            return;
        }
        self.saving = true;
        self.action_error = None;
        let finish: fn(&mut Self) = match &command {
            Command::SetPortAccess { .. } => |v| v.ports_form = None,
            Command::SaveForward { .. } => |v| v.rule = None,
            Command::RemoveForward { .. } => |v| v.removing = None,
            _ => |_| {},
        };
        let view = cx.weak_entity();
        self.dashboard.update(cx, |dashboard, cx| {
            dashboard.request(
                GatewayRequest {
                    command,
                    on_complete: Some(Box::new(move |result, cx| {
                        let _ = view.update(cx, |page, cx| {
                            page.saving = false;
                            match result {
                                Ok(()) => finish(page),
                                Err(message) => page.action_error = Some(message),
                            }
                            cx.notify();
                        });
                    })),
                },
                cx,
            );
        });
        cx.notify();
    }
}

impl DeviceSettings {
    fn session_list(&self, ui: Ui, cx: &Context<Self>) -> Div {
        let peer = self.peer.clone();
        let new_session = ui
            .button(Primary, "new-terminal-session", "New session")
            .icon(Icon::Plus)
            .on_click(move |_, w, cx| {
                if let Some(workspace) = w.root::<Workspace>().flatten() {
                    workspace.update(cx, |v, cx| v.new_terminal(peer.clone(), w, cx));
                }
            });
        let mut sessions = self
            .dashboard
            .read(cx)
            .open_session_numbers(&self.peer)
            .peekable();
        ui.section("Native terminal")
            .child(ListItem::new("Sessions").trailing(new_session))
            .when(sessions.peek().is_none(), |list| {
                list.child(ui.muted("No open sessions."))
            })
            .children(sessions.map(|number| {
                let peer = self.peer.clone();
                ListItem::new(format!("Session {number}"))
                    .leading(Icon::Terminal.draw(16.))
                    .trailing(
                        ui.button(Ghost, ("show-terminal", number), "Show")
                            .on_click(move |_, w, cx| {
                                if let Some(workspace) = w.root::<Workspace>().flatten() {
                                    workspace.update(cx, |v, cx| {
                                        v.show_terminal(peer.clone(), number, w, cx)
                                    });
                                }
                            }),
                    )
            }))
    }

    fn native_terminal(&self, allowed: bool, ui: Ui, cx: &Context<Self>) -> Div {
        let action = if allowed {
            "Revoke access"
        } else {
            "Allow access"
        };
        let access = if allowed {
            "This peer can run commands as the local gateway user."
        } else {
            "Grant this peer a shell as the local gateway user."
        };
        self.session_list(ui, cx).child(ui.separator()).child(
            ListItem::new("Access to this device")
                .detail(ui.muted(access))
                .trailing(
                    ui.button(Secondary, "terminal-access", action)
                        .on_click(cx.listener(move |v, _, _, cx| {
                            v.send(
                                Command::SetTerminalAccess {
                                    peer: v.peer.to_string(),
                                    allowed: !allowed,
                                },
                                cx,
                            );
                        })),
                ),
        )
    }
}

impl Render for DeviceSettings {
    fn render(&mut self, w: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let ui = Ui::new(w, cx);
        let mut page = page(self.peer.clone())
            .when_some(self.action_error.clone(), |p, e| p.child(ui.alert(e)));
        let Some(status) = &self.dashboard.read(cx).status else {
            return page.child(self.dashboard.update(cx, |d, cx| d.gateway_notice(ui, cx)));
        };
        let Some(id) = status.peers.get(self.peer.as_ref()) else {
            return page.child(ui.muted("This device is no longer paired."));
        };
        page = page.child(ui.muted(peer_detail(status.peer_platform(&self.peer), id)));
        page = page.child(self.native_terminal(
            status.terminal_access.contains(self.peer.as_ref()),
            ui,
            cx,
        ));
        page.child(self.forwarding(status, ui, cx))
            .child(self.port_access(status, ui, cx))
    }
}
