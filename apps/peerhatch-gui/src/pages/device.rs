use super::{
    device_heading, overview, overview_columns, peer_settings::PortsForm, rules::RuleForm,
};
use crate::{
    Dashboard, GatewayRequest, Workspace,
    components::{Button, ButtonVariant::Primary, Icon, Ui, single_line, tooltip},
};
use gpui::{prelude::*, *};
use peerhatch_control::{Command, Status};

fn connection_host(address: &str) -> String {
    let Ok(url) = http_client::Url::parse(address) else {
        return address.to_owned();
    };
    let host = url.host_str().unwrap_or(address);
    match url.port() {
        Some(port) => format!("{host}:{port}"),
        None => host.to_owned(),
    }
}

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
    pub(crate) fn platform(&self, cx: &App) -> peerhatch_control::DevicePlatform {
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
    fn connection(&self, status: &Status, ui: Ui) -> Div {
        let paths = status.connections_for(&self.peer);
        let mut card = ui.section("Connection").child(ui.separator());
        let Some(paths) = paths else {
            return card.child(ui.muted("Connection status unavailable."));
        };
        if paths.is_empty() {
            return card.child(ui.muted(status.peer_state(&self.peer)));
        }
        for (i, path) in paths.iter().enumerate() {
            let streams = path.stream_labels().collect::<Vec<_>>().join(" · ");
            let route = div()
                .w_full()
                .min_w_0()
                .flex()
                .items_center()
                .gap_2()
                .child(
                    div()
                        .flex_none()
                        .whitespace_nowrap()
                        .font_weight(FontWeight::MEDIUM)
                        .child(path.path.label()),
                )
                .when_some(path.address.as_ref(), |r, address| {
                    r.child(
                        single_line(ui.muted(connection_host(address)))
                            .flex_1()
                            .id(("path-address", i))
                            .tooltip(tooltip(address.clone().into())),
                    )
                })
                .when_some(path.rtt_label(), |r, latency| {
                    r.child(ui.muted(latency).ml_auto().flex_none().whitespace_nowrap())
                });
            let details = div()
                .min_w_0()
                .flex()
                .flex_col()
                .gap_1()
                .child(route)
                .when(!streams.is_empty(), |d| {
                    d.child(ui.muted(streams).text_xs())
                });
            card = card.when(i > 0, |c| c.child(ui.separator())).child(details);
        }
        card
    }

    fn new_session(&self, ui: Ui) -> Button {
        let peer = self.peer.clone();
        ui.button(Primary, "new-terminal-session", "New session")
            .icon(Icon::Terminal)
            .on_click(move |_, w, cx| {
                if let Some(workspace) = w.root::<Workspace>().flatten() {
                    workspace.update(cx, |v, cx| v.new_terminal(peer.clone(), w, cx));
                }
            })
    }

    fn sessions(&self, ui: Ui, cx: &Context<Self>) -> Div {
        let mut sessions = self
            .dashboard
            .read(cx)
            .open_sessions(&self.peer, cx)
            .peekable();
        let card = ui.section("Terminal sessions").child(ui.separator());
        if sessions.peek().is_none() {
            return card.child(ui.muted("No open sessions."));
        }
        card.child(
            div()
                .flex()
                .flex_col()
                .children(sessions.map(|(number, title)| {
                    let peer = self.peer.clone();
                    ui.navigation_row(
                        ("show-terminal", number),
                        ui.icon_tile(Icon::Terminal, 32., 16.),
                        title,
                        format!("Session {number}"),
                    )
                    .on_click(move |_, w, cx| {
                        if let Some(workspace) = w.root::<Workspace>().flatten() {
                            workspace
                                .update(cx, |v, cx| v.show_terminal(peer.clone(), number, w, cx));
                        }
                    })
                })),
        )
    }
}

impl Render for DeviceSettings {
    fn render(&mut self, w: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let ui = Ui::new(w, cx);
        let status = self.dashboard.read(cx).status.as_ref();
        let identity = status.and_then(|s| {
            let name = self.peer.as_ref();
            let id = s.peers.get(name)?;
            let short = s
                .short_peer_ids()
                .remove(name)
                .unwrap_or_else(|| id.clone());
            Some((id.as_str(), short))
        });
        let paired = identity.is_some();
        let header = device_heading(
            ui,
            "Device details",
            self.peer.clone(),
            self.platform(cx),
            identity,
        )
        .when(status.is_some_and(|s| s.peer_online(&self.peer)), |r| {
            r.child(self.new_session(ui))
        });
        let page = overview()
            .child(header)
            .when_some(self.action_error.clone(), |p, e| p.child(ui.alert(e)));
        let Some(status) = status else {
            return page.child(self.dashboard.update(cx, |d, cx| d.gateway_notice(ui, cx)));
        };
        if !paired {
            return page.child(ui.muted("This device is no longer paired."));
        }
        page.child(overview_columns(
            self.connection(status, ui),
            self.sessions(ui, cx),
        ))
        .child(self.forwarding(status, ui, cx))
        .child(self.port_access(status, ui, cx))
    }
}
