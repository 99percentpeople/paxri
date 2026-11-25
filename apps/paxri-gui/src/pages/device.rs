use super::{home::DevicesPane, overview};
use crate::{
    Dashboard, Workspace,
    components::{
        Button,
        ButtonVariant::Destructive,
        Dialog, DialogContent, DialogMode, DialogSuccess, Ui,
    },
};
use gpui::{prelude::*, *};
use paxri_control::{Command, DevicePlatform, Status};
use paxri_ui::components::DeviceCounts;

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
        let view = cx.weak_entity();
        let selected = peer.to_owned();
        let expected = id.clone();
        let menu = ui.device_actions(
            SharedString::from(format!("device-actions-{id}")),
            name,
            move |w, cx| {
                let _ = view.update(cx, |v, cx| v.confirm_unpair(&selected, &expected, w, cx));
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
                self.connection_history.connection_section(
                    id,
                    status.connections_for(peer),
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
