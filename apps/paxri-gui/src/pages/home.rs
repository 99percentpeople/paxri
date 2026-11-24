use super::overview;
use crate::{
    Dashboard,
    components::dock::{DockNavigation, DockPanel},
    components::{
        Button,
        ButtonVariant::{Ghost, Primary, Secondary},
        Dialog, DialogContent, DialogMode, DialogSuccess, Icon, ListItem, Ui, actions, heading,
    },
};
use gpui::{prelude::*, *};
use paxri_control::{Command, DevicePlatform, PairingPeer, PairingStatus, Status};
use paxri_ui::components::{ListState, ListView};

#[derive(PartialEq, Eq)]
pub(crate) enum DevicesPane {
    Peer(String),
    Forwards,
    Sessions,
}

impl Dashboard {
    pub(crate) fn reconcile_device_selection(&mut self, status: &paxri_control::Status) {
        let select_peer = match &self.devices_pane {
            None => true,
            Some(DevicesPane::Peer(peer)) => !status.peers.contains_key(peer),
            _ => false,
        };
        if select_peer {
            self.devices_pane = status
                .peers_by_last_change()
                .first()
                .map(|(name, _)| DevicesPane::Peer(name.to_string()));
        }
        if self.session_filters.scope.device.as_ref().is_some_and(|p| !status.peers.contains_key(p)) {
            self.session_filters.scope.device = None;
        }
        if self
            .forward_filters
            .device
            .as_ref()
            .is_some_and(|peer| !status.peers.contains_key(peer))
        {
            self.forward_filters.device = None;
        }
    }

    pub(super) fn show_devices_pane(&mut self, pane: DevicesPane, cx: &mut Context<Self>) {
        let forwards = pane == DevicesPane::Forwards;
        self.devices_pane = Some(pane);
        if forwards {
            self.refresh_forward_directory(cx);
            if self.busy { self.refresh_presence = true; }
            else { self.send(Command::Status, cx); }
        }
        self.home_scroll.set_offset(Point::default());
        cx.notify();
    }

    fn copy_pairing_code(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let code = self.local_code.read(cx).text().to_owned();
        crate::components::copy_with_feedback(code, "Pairing code copied", window, cx);
    }

    fn device_sidebar(&self, status: Option<&Status>, ui: Ui, cx: &Context<Self>) -> DockNavigation {
        let requests = status.map_or(0, |s| s.pairing.incoming.len());
        let body = div()
            .min_h_0()
            .min_w_0()
            .flex_1()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .when_some(status.filter(|s| !s.peers.is_empty()), |d, s| {
                        d.child(self.paired_devices(ui, s, cx))
                    })
                    .when_some(status.filter(|s| !s.pairing.pending_unpairs.is_empty()), |d, s| {
                        d.child(ui.muted(format!(
                            "Awaiting unpair confirmation: {}",
                            s.pairing.pending_unpairs.join(", ")
                        )).text_xs())
                    }),
            )
            .when(status.is_some() && (self.devices_pane.is_some() || requests > 0), |sidebar| {
                let label = if requests > 0 { format!("Pair a device ({requests})") } else { "Pair a device".to_owned() };
                sidebar.child(ui.button(Ghost, "add-device", label).icon(Icon::Plus).row()
                    .on_click(cx.listener(|v, _, w, cx| v.open_pairing(w, cx))))
            })
            .child(ui.button(Ghost, "all-sessions", "Sessions").icon(Icon::Terminal).row()
                .pressed(self.devices_pane == Some(DevicesPane::Sessions))
                .on_click(cx.listener(|v, _, _, cx| v.show_sessions(None, cx))))
            .child(
                ui.button(Ghost, "all-forwards", "Port forwarding")
                    .icon(Icon::Forward)
                    .row()
                    .pressed(self.devices_pane == Some(DevicesPane::Forwards))
                    .on_click(cx.listener(|v, _, _, cx| {
                        v.forward_filters.reset(None, cx);
                        v.show_devices_pane(DevicesPane::Forwards, cx);
                    })),
            )
            .child(ui.separator())
            .child(ui.local_device_footer(self.local_name.clone(), status.map(|_| "Gateway online")));
        DockNavigation::new("Devices", body)
    }

    pub(crate) fn home(&self, ui: Ui, width: Pixels, window: &mut Window, cx: &mut Context<Self>) -> DockPanel {
        let transition = self.gateway_transition();
        let status = self.status.as_ref().filter(|_| transition.is_none());
        let (content, action) = match (&self.devices_pane, status) {
            (_, None) => self.gateway_notice(transition, ui, cx),
            (Some(DevicesPane::Sessions), Some(status)) => self.sessions_page(status, ui, window, cx),
            (Some(DevicesPane::Peer(peer)), Some(status)) => {
                let action = self.can_open_shell(peer).then(|| self.new_session(peer, ui).into_any_element());
                (self.device_detail(peer, status, ui, window, cx), action)
            }
            (Some(DevicesPane::Forwards), Some(status)) => {
                let action = (!status.peers.is_empty()).then(|| ui.floating_action("add-rule", Icon::Plus, "Add forwarding rule")
                    .on_click(cx.listener(|v, _, w, cx| v.open_rule(None, None, w, cx))).into_any_element());
                (self.forwarding(status, ui, width, cx), action)
            }
            (_, Some(status)) => (overview()
                .child(ui.device_heading(
                    Some(ui.muted("This device").text_xs().flex_none()),
                    self.local_name.clone(),
                    DevicePlatform::current(),
                    Some((
                        &status.device_id,
                        paxri_config::peer_id::default_prefix(&status.device_id).into(),
                    )),
                ))
                .child(ui.separator())
                .child(
                    ListState::empty(Icon::Monitor, "No paired devices")
                        .detail("Use + to pair your first device.").render(*ui),
                ), Some(ui.floating_action("pair-first-device", Icon::Plus, "Pair a device")
                    .on_click(cx.listener(|v, _, w, cx| v.open_pairing(w, cx))).into_any_element())),
        };
        DockPanel::scroll("devices-content", self.home_scroll.clone(), content)
            .with_action(action)
            .with_navigation(self.device_sidebar(status, ui, cx))
    }

    fn open_pairing(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let focus = self
            .status
            .as_ref()
            .filter(|s| s.pairing.outgoing.is_none() && s.pairing.incoming.is_empty())
            .map(|_| self.pairing_input.read(cx).focus_handle(cx));
        let model = cx.entity();
        Dialog::open(
            window,
            cx,
            DialogMode::Modeless,
            "Pair a device",
            size(px(760.), px(568.)),
            model,
            focus,
            Self::pairing_dialog,
        );
        cx.notify();
    }

    fn pairing_dialog(&self, ui: Ui, cx: &Context<Self>) -> DialogContent {
        let Some(pairing) = self.status.as_ref().map(|s| &s.pairing) else {
            return DialogContent::new(
                ui.muted("Start the local gateway to pair a device."),
                "Close",
            );
        };
        let invitation = div()
            .flex_1()
            .flex_basis(px(274.))
            .min_w_0()
            .flex()
            .flex_col()
            .gap_2()
            .p_3()
            .rounded_lg()
            .border_1()
            .border_color(rgb(ui.colors.border))
            .bg(rgb(ui.colors.card))
            .child(
                ListItem::new(self.local_name.clone())
                    .leading(ui.device_icon(DevicePlatform::current())),
            )
            .when_some(self.pairing_qr.clone(), |column, qr| {
                column.child(div().w_full().flex().justify_center().child(pairing_qr(qr)))
            })
            .child(self.local_pairing_code(ui, pairing, cx))
            .when(
                pairing.code.is_some() && self.pairing_qr.is_none(),
                |column| {
                    column.child(
                        ui.muted("Invitation too large for a QR code. Copy the code to pair."),
                    )
                },
            );
        let form = div()
            .flex_1()
            .flex_basis(px(360.))
            .min_w_0()
            .flex()
            .flex_col()
            .gap_3()
            .child(heading("Pair with a code"))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(div().flex_1().min_w_0().child(self.pairing_input.clone()))
                    .when(pairing.outgoing.is_none(), |row| {
                        row.child(ui.button(Primary, "pair-device", "Pair device").on_click(
                            cx.listener(|v, _, w, cx| {
                                let code = v.pairing_input.read(cx).text().trim().to_owned();
                                v.send_from_dialog(
                                    Command::JoinPairing { code },
                                    w,
                                    DialogSuccess::KeepOpen,
                                    cx,
                                );
                            }),
                        ))
                    }),
            )
            .child(
                ui.muted("Pairing allows both devices to open terminals and copy files.")
                    .text_xs(),
            )
            .child(ui.separator())
            .child(heading("Pending devices"))
            .child(self.pending_devices(ui, pairing, cx));
        let body = div()
            .flex()
            .flex_wrap()
            .items_start()
            .gap_4()
            .child(invitation)
            .child(form);
        DialogContent::new(body, "Done")
    }

    fn pending_devices(&self, ui: Ui, pairing: &PairingStatus, cx: &Context<Self>) -> Div {
        if pairing.incoming.is_empty() && pairing.outgoing.is_none() {
            return ui.muted("No pending pairing requests.");
        }
        let list = ListView::new()
            .divided()
            .when_some(pairing.outgoing.as_ref(), |list, id| {
                list.child(
                    ListItem::new(paxri_config::peer_id::default_prefix(id).to_owned())
                        .leading(ui.device_icon(DevicePlatform::Unknown))
                        .detail("Waiting for confirmation")
                        .trailing(
                            ui.button(Secondary, "cancel-pairing", "Cancel request")
                                .on_click(cx.listener(|v, _, w, cx| {
                                    v.send_from_dialog(
                                        Command::CancelPairing,
                                        w,
                                        DialogSuccess::KeepOpen,
                                        cx,
                                    );
                                })),
                        ),
                )
            })
            .children(pairing.incoming.iter().map(|peer| {
                let prefix = paxri_config::peer_id::default_prefix(&peer.device_id);
                let detail = if peer.name.ends_with(prefix) {
                    peer.platform.label().to_owned()
                } else {
                    format!("{} · {prefix}", peer.platform.label())
                };
                ListItem::new(peer.name.clone())
                    .leading(ui.device_icon(peer.platform))
                    .detail(detail)
                    .trailing(
                        actions()
                            .child(self.answer_pairing(ui, peer, false, cx))
                            .child(self.answer_pairing(ui, peer, true, cx)),
                    )
            }));
        div().child(list)
    }

    fn answer_pairing(
        &self,
        ui: Ui,
        peer: &PairingPeer,
        accept: bool,
        cx: &Context<Self>,
    ) -> Button {
        let id = peer.device_id.clone();
        let request = peer.request_id.clone();
        let (variant, label) = if accept {
            (Primary, "Accept")
        } else {
            (Secondary, "Decline")
        };
        ui.button(
            variant,
            SharedString::from(format!("pair-{label}-{request}")),
            label,
        )
        .on_click(cx.listener(move |v, _, w, cx| {
            v.send_from_dialog(
                Command::AnswerPairing {
                    device_id: id.clone(),
                    request_id: Some(request.clone()),
                    accept,
                },
                w,
                DialogSuccess::KeepOpen,
                cx,
            );
        }))
    }

    fn local_pairing_code(&self, ui: Ui, pairing: &PairingStatus, cx: &Context<Self>) -> Div {
        let mut code = div().w_full().flex().items_center().gap_1();
        if pairing.code.is_some() {
            code = code
                .child(div().flex_1().min_w_0().child(self.local_code.clone()))
                .child(
                    ui.icon_button(Ghost, "copy-code", Icon::Copy, "Copy code")
                        .on_click(cx.listener(|v, _, w, cx| v.copy_pairing_code(w, cx))),
                );
        } else {
            code = code.child(ui.muted("Code unavailable").flex_1());
        }
        code = code.child(
            ui.icon_button(Ghost, "refresh-code", Icon::Refresh, "Refresh code")
                .on_click(cx.listener(|v, _, w, cx| {
                    v.send_from_dialog(Command::RefreshPairingCode, w, DialogSuccess::KeepOpen, cx);
                })),
        );
        ui.labeled("Your pairing code", code).min_w_0()
    }
}

fn pairing_qr(qr: std::rc::Rc<qrcode::QrCode>) -> impl IntoElement {
    let mut code = canvas(
        |_, _, _| (),
        move |bounds, _, window, _| {
            const QUIET_ZONE: usize = 4;
            let scale = window.scale_factor();
            let modules = qr.width();
            let side = (f32::from(bounds.size.width.min(bounds.size.height)) * scale).round();
            let pitch = side / (modules + 2 * QUIET_ZONE) as f32;
            // Snap shared edges, so leftover pixels are distributed across the grid.
            let edge = |index| ((index + QUIET_ZONE) as f32 * pitch).round();
            let left = (f32::from(bounds.origin.x) * scale).round();
            let top = (f32::from(bounds.origin.y) * scale).round();
            window.paint_quad(fill(bounds, rgb(0xffffff)));
            for y in 0..modules {
                let mut x = 0;
                while x < modules {
                    let start = x;
                    while x < modules && qr[(x, y)] == qrcode::Color::Dark {
                        x += 1;
                    }
                    if x > start {
                        let origin = point(px((left + edge(start)) / scale), px((top + edge(y)) / scale));
                        let size = size(px((edge(x) - edge(start)) / scale), px((edge(y + 1) - edge(y)) / scale));
                        window.paint_quad(fill(Bounds::new(origin, size), rgb(0)));
                    }
                    x += 1;
                }
            }
        },
    )
    .w_full()
    .max_w(px(288.))
    .flex_none();
    code.style().aspect_ratio = Some(1.);
    code
}
