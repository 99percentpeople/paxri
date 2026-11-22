use super::{device_heading, home::DevicesPane, overview};
use crate::{
    Dashboard, Workspace,
    components::{
        Button,
        ButtonVariant::{Destructive, Ghost, Primary},
        Dialog, DialogContent, DialogMode, DialogSuccess, Icon, Ui, heading, row, single_line,
        tooltip,
    },
};
use gpui::{prelude::*, *};
use peerhatch_control::{Command, PeerConnection, Status, StreamKind};

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
    fn connection_paths(&self, peer_id: &str, paths: &[PeerConnection], ui: Ui) -> Div {
        let mut list = ui.section().child(heading("Connection"));
        for (i, path) in paths.iter().enumerate() {
            let streams = path
                .stream_labels_matching(|kind| kind != StreamKind::Terminal)
                .collect::<Vec<_>>()
                .join(" · ");
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
                    r.child(
                        div()
                            .ml_auto()
                            .min_w_0()
                            .flex()
                            .items_center()
                            .gap_2()
                            .children(self.latency.sparkline(peer_id, path, ui))
                            .child(ui.muted(latency).flex_none().whitespace_nowrap()),
                    )
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
            list = list.when(i > 0, |d| d.child(ui.separator())).child(details);
        }
        list
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

    fn sessions(&self, peer: &str, online: bool, ui: Ui, cx: &Context<Self>) -> Div {
        let peer = SharedString::from(peer.to_owned());
        let mut sessions = self.open_sessions(&peer, cx).peekable();
        let section = ui.section().child(
            row()
                .child(heading("Terminal sessions"))
                .when(online, |r| r.child(self.new_session(&peer, ui))),
        );
        if sessions.peek().is_none() {
            return section.child(ui.muted("No open sessions."));
        }
        section.child(
            div()
                .flex()
                .flex_col()
                .children(sessions.map(|(number, title)| {
                    let peer = peer.clone();
                    ui.navigation_row(
                        ("show-terminal", number),
                        Icon::Terminal.draw(18.),
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
                    .child(ui.muted("Active connections close immediately. Both gateways remove forwarding rules for this pairing. Offline peers sync when reachable."));
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
        let online = status.peer_online(peer);
        let dot = if online {
            ui.colors.success
        } else {
            ui.colors.muted_foreground
        };
        let header = device_heading(
            ui,
            ui.status_badge(status.peer_state(peer), dot),
            peer.to_owned().into(),
            status.peer_platform(peer),
            identity,
        );
        let count = status
            .forwards
            .values()
            .filter(|rule| rule.peer == peer)
            .count();
        let rule_count = match count {
            0 => "View forwarding rules".to_owned(),
            1 => "1 locally configured rule".to_owned(),
            n => format!("{n} locally configured rules"),
        };
        let selected = peer.to_owned();
        let forwards = ui
            .navigation_row(
                "device-forwards",
                Icon::Forward.draw(18.),
                "Port forwarding".into(),
                rule_count,
            )
            .on_click(cx.listener(move |v, _, _, cx| {
                v.forward_filters.reset(Some(selected.clone()), cx);
                v.show_devices_pane(DevicesPane::Forwards, cx);
            }));
        overview()
            .py_2()
            .child(header)
            .child(ui.separator())
            .when_some(
                status
                    .connections_for(peer)
                    .filter(|paths| !paths.is_empty())
                    .zip(status.peers.get(peer)),
                |page, (paths, id)| {
                    page.child(self.connection_paths(id, paths, ui))
                        .child(ui.separator())
                },
            )
            .child(self.sessions(peer, online, ui, cx))
            .child(ui.separator())
            .child(forwards)
            .when_some(status.peers.get(peer), |page, id| {
                let id = id.clone();
                let name = peer.to_owned();
                page.child(div().flex().justify_end().child(
                    ui.button(Ghost, "unpair-device", "Unpair device").on_click(
                        cx.listener(move |v, _, w, cx| v.confirm_unpair(&name, &id, w, cx)),
                    ),
                ))
            })
    }
}
