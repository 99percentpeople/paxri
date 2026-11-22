use super::{device_heading, home::DevicesPane, overview, overview_columns};
use crate::{
    Dashboard, Workspace,
    components::{
        Button,
        ButtonVariant::{Destructive, Primary, Secondary},
        Dialog, DialogContent, DialogMode, DialogSuccess, Icon, Ui, actions, single_line, tooltip,
    },
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

impl Dashboard {
    fn connection(&self, peer: &str, status: &Status, ui: Ui) -> Div {
        let paths = status.connections_for(peer);
        let mut card = ui.section("Connection").child(ui.separator());
        let Some(paths) = paths else {
            return card.child(ui.muted("Connection status unavailable."));
        };
        if paths.is_empty() {
            return card.child(ui.muted(status.peer_state(peer)));
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

    fn new_session(&self, peer: &str, ui: Ui) -> Button {
        let peer = SharedString::from(peer.to_owned());
        ui.button(Primary, "new-terminal-session", "New session")
            .icon(Icon::Terminal)
            .on_click(move |_, w, cx| {
                if let Some(workspace) = w.root::<Workspace>().flatten() {
                    workspace.update(cx, |v, cx| v.new_terminal(peer.clone(), w, cx));
                }
            })
    }

    fn sessions(&self, peer: &str, ui: Ui, cx: &Context<Self>) -> Div {
        let peer = SharedString::from(peer.to_owned());
        let mut sessions = self.open_sessions(&peer, cx).peekable();
        let card = ui.section("Terminal sessions").child(ui.separator());
        if sessions.peek().is_none() {
            return card.child(ui.muted("No open sessions."));
        }
        card.child(
            div()
                .flex()
                .flex_col()
                .children(sessions.map(|(number, title)| {
                    let peer = peer.clone();
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

impl Dashboard {
    fn confirm_unpair(&self, peer: &str, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let peer = peer.to_owned();
        let id = id.to_owned();
        let dashboard = cx.entity();
        Dialog::open(
            window,
            cx,
            DialogMode::Modal,
            "Unpair device?",
            size(px(440.), px(252.)),
            dashboard,
            None,
            move |_, ui, cx| {
                let name = peer.clone();
                let expected = id.clone();
                let body = div().flex().flex_col().gap_2()
                    .child(format!("Unpair this device and {peer}?"))
                    .child(ui.muted("Active connections close immediately. Both gateways remove forwarding rules and service grants for this pairing. Offline peers sync when reachable."));
                DialogContent::new(body, "Cancel").action(
                    ui.button(Destructive, "confirm-unpair", "Unpair device")
                        .on_click(cx.listener(move |v, _, w, cx| {
                            v.send_from_dialog(
                                Command::Unpair {
                                    peer: name.clone(),
                                    expected_id: Some(expected.clone()),
                                },
                                w,
                                DialogSuccess::Close,
                                cx,
                            );
                        })),
                )
            },
        );
    }

    pub(super) fn device_detail(
        &self,
        peer: &str,
        status: &Status,
        ui: Ui,
        cx: &Context<Self>,
    ) -> Div {
        let identity = status.peers.get(peer).map(|id| {
            (
                id.as_str(),
                status
                    .short_peer_ids()
                    .remove(peer)
                    .unwrap_or_else(|| id.clone()),
            )
        });
        let header = device_heading(
            ui,
            "Device details",
            peer.to_owned().into(),
            status.peer_platform(peer),
            identity,
        )
        .when(status.peer_online(peer), |row| {
            row.child(self.new_session(peer, ui))
        });
        let unpair = status.peers.get(peer).map(|id| {
            let id = id.clone();
            let name = peer.to_owned();
            ui.button(Secondary, "unpair-device", "Unpair device")
                .on_click(cx.listener(move |v, _, w, cx| v.confirm_unpair(&name, &id, w, cx)))
        });
        let selected = peer.to_owned();
        overview()
            .child(header)
            .child(overview_columns(
                self.connection(peer, status, ui),
                self.sessions(peer, ui, cx),
            ))
            .child(
                actions()
                    .child(
                        ui.button(Secondary, "device-forwards", "View forwarding rules")
                            .on_click(cx.listener(move |v, _, _, cx| {
                                v.forward_filter = Some(selected.clone());
                                v.show_devices_pane(DevicesPane::Forwards, cx);
                            })),
                    )
                    .children(unpair),
            )
    }
}
