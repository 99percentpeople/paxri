use super::{home::DevicesPane, overview};
use crate::{
    Dashboard, Workspace,
    components::{
        Button,
        ButtonVariant::Destructive,
        Dialog, DialogContent, DialogMode, DialogSuccess, Toast, Ui,
    },
};
use gpui::{prelude::*, *};
use paxri_control::{Command, DevicePlatform, Status};
use paxri_ui::{components::DeviceCounts, connections::reconnect_toast};

impl Dashboard {
    pub(super) fn device_counts(&self, peer: &str, status: &Status) -> DeviceCounts {
        let online = status.peer_online(peer);
        let sessions = if !status.peer_platform(peer).supports_shell(status.peer_capabilities.get(peer).copied()) {
            Some(0)
        } else {
            self.terminal_sessions.get(peer)
                .filter(|s| online && s.listed && !s.list_failed)
                .map(|s| s.hosted.len())
        };
        let local = status.forwards.values().filter(|f| f.peer == peer).count();
        let forwards = if !status.peer_supports_forward_rules(peer) {
            Some(local)
        } else {
            status.peer_forwards.as_ref().filter(|_| online)
                .and_then(|d| d.rules.get(peer))
                .map(|rules| local + rules.len())
        };
        DeviceCounts { sessions, forwards }
    }

    pub(super) fn new_session(&self, peer: &str, ui: Ui) -> Button {
        let peer = SharedString::from(peer.to_owned());
        ui.new_session_button()
            .on_click(move |_, w, cx| {
                if let Some(workspace) = w.root::<Workspace>().flatten() {
                    workspace.update(cx, |v, cx| v.new_terminal(peer.clone(), w, cx));
                }
            })
    }


}

impl Dashboard {
    fn reconnect_peer(
        &mut self,
        peer: String,
        display_name: String,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        if !self.reconnecting_peers.insert(peer.clone()) {
            return;
        }
        let target_window = window.window_handle();
        Toast::show(format!("Reconnecting to {display_name}…"), target_window, cx);
        let path = self
            .active_address()
            .unwrap_or_else(paxri_control::default_address);
        let request_peer = peer.clone();
        let job = self.runtime.spawn(async move {
            paxri_control::request(&path, Command::ReconnectPeer { peer: request_peer }).await
        });
        cx.spawn(async move |this, cx| {
            let result = job
                .await
                .map_err(|e| e.to_string())
                .and_then(|r| r.map_err(|e| format!("{e:#}")));
            let _ = this.update(cx, |view, cx| {
                view.reconnecting_peers.remove(&peer);
                match result {
                    Ok(mut status) => {
                        let conns = status
                            .connections_for(&peer)
                            .map(<[_]>::to_vec)
                            .unwrap_or_default();
                        let label = status.peer_display_name(&peer).to_owned();
                        if status.peer_forwards.is_none()
                            && let Some(old) = &mut view.status
                            && old.device_id == status.device_id
                            && old.ipc_path == status.ipc_path
                            && old.peers == status.peers
                        {
                            status.peer_forwards = old.peer_forwards.take();
                        }
                        let paths = status.peer_connections.iter().flatten().filter_map(
                            |(alias, paths)| {
                                Some((status.peers.get(alias)?.as_str(), paths.as_slice()))
                            },
                        );
                        view.connection_history.record(paths);
                        view.sync_peer_connections(&status, cx);
                        view.status = Some(status);
                        view.refresh_hosted_sessions(true, cx);
                        view.refresh_forward_directory(cx);
                        Toast::show(reconnect_toast(&label, &conns), target_window, cx);
                        cx.notify();
                    }
                    Err(error) => {
                        if !view.busy {
                            view.send(Command::Status, cx);
                        } else {
                            view.refresh_presence = true;
                        }
                        Toast::show(
                            format!("Could not reconnect to {display_name}: {error}"),
                            target_window,
                            cx,
                        );
                    }
                }
            });
        })
        .detach();
    }

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
        cx: &mut Context<Self>,
    ) -> Div {
        let Some(id) = status.peers.get(peer) else { return overview() };
        let short = status.short_peer_ids().remove(peer).unwrap_or_else(|| id.clone());
        let name = status.peer_display_name(peer);
        let reconnect_view = cx.weak_entity();
        let reconnect_peer = peer.to_owned();
        let reconnect_name = name.to_owned();
        let unpair_view = cx.weak_entity();
        let selected = peer.to_owned();
        let expected = id.clone();
        let menu = ui.device_actions(
            SharedString::from(format!("device-actions-{id}")),
            name,
            move |w, cx| {
                let _ = reconnect_view.update(cx, |v, cx| {
                    v.reconnect_peer(reconnect_peer.clone(), reconnect_name.clone(), w, cx)
                });
            },
            move |w, cx| {
                let _ = unpair_view.update(cx, |v, cx| v.confirm_unpair(&selected, &expected, w, cx));
            },
        );
        let header = ui.device_heading(
            None,
            name.to_owned().into(),
            status.peer_platform(peer),
            Some((id, short)),
        ).child(menu);
        let counts = self.device_counts(peer, status);
        let show_forwards = counts.forwards.is_some_and(|n| n > 0)
            || status.peer_platform(peer) == DevicePlatform::Android
            || status.peer_supports_forward_rules(peer);
        let selected = peer.to_owned();
        let session_peer = peer.to_owned();
        overview()
            .py_2()
            .child(header)
            .child(ui.device_detail_sections(
                self.connection_history.connection_section_with_phase(
                    id,
                    status.connections_for(peer),
                    status.peer_phase(peer),
                    status.peer_presence,
                    ui,
                ),
                counts,
                show_forwards,
                cx.listener(move |v, _, _, cx| v.show_sessions(Some(session_peer.clone()), cx)),
                cx.listener(move |v, _, _, cx| {
                    v.forward_filters.for_device(Some(selected.clone()), cx);
                    v.show_devices_pane(DevicesPane::Forwards, cx);
                }),
            ))
    }
}
