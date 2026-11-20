use super::{peer_detail, peer_settings::PortsForm, rules::RuleForm};
use crate::{
    Dashboard, GatewayRequest, Workspace,
    components::{
        ButtonVariant::{Ghost, Primary},
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
                    command: crate::Operation::Api(command),
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
    fn connection(&self, status: &tunnel_control::Status, ui: Ui, cx: &Context<Self>) -> Div {
        let Some(paths) = status.connections_for(&self.peer) else {
            return ui
                .section("Connection")
                .child(ui.muted("Connection status unavailable."));
        };
        let mut section = ui.section("Connection");
        if paths.is_empty() {
            section = section.child(ui.muted("No active connection."));
        }
        for path in paths {
            section =
                section.child(ListItem::new(path.protocol.clone()).detail(path.description()));
        }
        if !self.saving {
            section = section.child(ui.button(Ghost, "check-peer", "Check connection").on_click(
                cx.listener(|v, _, _, cx| {
                    v.send(
                        Command::CheckPeer {
                            peer: v.peer.to_string(),
                        },
                        cx,
                    )
                }),
            ));
        }
        section
    }

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
        page.child(self.connection(status, ui, cx))
            .child(self.session_list(ui, cx))
            .child(self.forwarding(status, ui, cx))
            .child(self.port_access(status, ui, cx))
    }
}
