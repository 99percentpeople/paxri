use super::{home::DevicesPane, overview};
use crate::{
    Dashboard, Workspace,
    components::{
        Button,
        ButtonVariant::{Destructive, Ghost},
        Dialog, DialogContent, DialogMode, DialogSuccess, Icon, Ui,
    },
};
use gpui::{prelude::*, *};
use peerhatch_control::{Command, DevicePlatform, Status};

impl Dashboard {
    fn new_session(&self, peer: &str, ui: Ui) -> Button {
        let peer = SharedString::from(peer.to_owned());
        ui.new_session_button()
            .on_click(move |_, w, cx| {
                if let Some(workspace) = w.root::<Workspace>().flatten() {
                    workspace.update(cx, |v, cx| v.new_terminal(peer.clone(), w, cx));
                }
            })
    }

    fn sessions(&self, peer: &str, ui: Ui, cx: &Context<Self>) -> Div {
        let peer = SharedString::from(peer.to_owned());
        let rows = self.open_sessions(&peer, cx).map(|(number, title)| {
            let peer = peer.clone();
            ui.session_row(number, title, format!("Session {number}"))
                .on_click(move |_, w, cx| {
                    if let Some(workspace) = w.root::<Workspace>().flatten() {
                        workspace.update(cx, |v, cx| v.show_terminal(peer.clone(), number, w, cx));
                    }
                })
        });
        ui.terminal_sessions(Some(self.new_session(&peer, ui)), rows)
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
            move |dashboard, ui, cx| {
                let display_name = dashboard.status.as_ref()
                    .map_or(peer.as_str(), |s| s.peer_display_name(&peer));
                let name = peer.clone();
                let expected = id.clone();
                let body = div().flex().flex_col().gap_2()
                    .child(format!("Unpair this device and {display_name}?"))
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
        let header = ui.device_heading(
            ui.connection_badge(status.connections_for(peer), status.peer_presence),
            status.peer_display_name(peer).to_owned().into(),
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
                    page.child(self.connection_history.connection_paths(id, paths, *ui))
                        .child(ui.separator())
                },
            )
            .when(status.can_open_shell(peer), |page| {
                page.child(self.sessions(peer, ui, cx)).child(ui.separator())
            })
            // Android uses this gateway to store rules, including its first one.
            .when(count > 0 || status.peer_platform(peer) == DevicePlatform::Android
                || status.peer_supports_forward_rules(peer), |page| page.child(forwards))
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
