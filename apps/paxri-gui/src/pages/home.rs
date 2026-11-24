use super::{overview, peer_detail, short_id};
use crate::{
    Dashboard,
    components::dock::{DockNavigation, DockPanel},
    components::{
        Button,
        ButtonVariant::{Ghost, Primary, Secondary},
        Dialog, DialogContent, DialogMode, DialogSuccess, Icon, ListItem, Select, Ui, single_line,
    },
};
use gpui::{prelude::*, *};
use paxri_control::{Command, DevicePlatform, PairingPeer, PairingStatus};

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

    fn device_sidebar(&self, ui: Ui, cx: &Context<Self>) -> DockNavigation {
        let requests = self.status.as_ref().map_or(0, |s| s.pairing.incoming.len());
        let state = if self.status.is_some() {
            "Gateway online"
        } else if self.error.is_some() {
            "Gateway offline"
        } else {
            "Connecting…"
        };
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
                    .when_some(self.status.as_ref(), |d, s| {
                        d.child(self.paired_devices(ui, s, cx))
                    })
                    .when_some(self.status.as_ref().filter(|s| !s.pairing.pending_unpairs.is_empty()), |d, s| {
                        d.child(ui.muted(format!(
                            "Awaiting unpair confirmation: {}",
                            s.pairing.pending_unpairs.join(", ")
                        )).text_xs())
                    }),
            )
            .when(self.status.as_ref().is_some_and(|s| !s.peers.is_empty() || requests > 0), |sidebar| {
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
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(Icon::Monitor.draw(18.))
                    .child(
                        div()
                            .min_w_0()
                            .flex_1()
                            .flex()
                            .flex_col()
                            .child(single_line(div()).child(self.local_name.clone()))
                            .child(ui.muted(state).text_xs()),
                    ),
            );
        DockNavigation::new("Devices", body)
    }

    pub(crate) fn home(&self, ui: Ui, width: Pixels, cx: &mut Context<Self>) -> DockPanel {
        let (content, action) = match (&self.devices_pane, &self.status) {
            (_, None) => (self.gateway_notice(ui, cx), self.gateway_start_button(ui, cx)),
            (Some(DevicesPane::Sessions), Some(status)) => self.sessions_page(status, ui, cx),
            (Some(DevicesPane::Peer(peer)), Some(status)) => {
                let action = self.can_open_shell(peer).then(|| self.new_session(peer, ui).into_any_element());
                (self.device_detail(peer, status, ui, cx), action)
            }
            (Some(DevicesPane::Forwards), Some(status)) => {
                let action = (!status.peers.is_empty()).then(|| ui.floating_action("add-rule", Icon::Plus, "Add forwarding rule")
                    .on_click(cx.listener(|v, _, w, cx| v.open_rule(None, None, w, cx))).into_any_element());
                (self.forwarding(status, ui, width, cx), action)
            }
            (_, Some(status)) => (overview()
                .child(ui.device_heading(
                    ui.muted("This device").text_xs().flex_none(),
                    self.local_name.clone(),
                    DevicePlatform::current(),
                    Some((
                        &status.device_id,
                        paxri_config::peer_id::default_prefix(&status.device_id).into(),
                    )),
                ))
                .child(ui.separator())
                .child(
                    ui.section()
                        .child(crate::components::heading("Paired devices"))
                        .child(ui.muted(
                            "Use the + button to pair your first device.",
                        )),
                ), Some(ui.floating_action("pair-first-device", Icon::Plus, "Pair a device")
                    .on_click(cx.listener(|v, _, w, cx| v.open_pairing(w, cx))).into_any_element())),
        };
        DockPanel::scroll("devices-content", self.home_scroll.clone(), content)
            .with_action(action)
            .with_navigation(self.device_sidebar(ui, cx))
    }

    fn open_pairing(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let pairing = self.status.as_ref().map(|s| &s.pairing);
        self.review_request = pairing
            .and_then(|p| p.incoming.first())
            .map(|r| r.request_id.clone());
        let focus = (self.review_request.is_none()
            && pairing.is_some_and(|p| p.outgoing.is_none()))
        .then(|| self.pairing_input.read(cx).focus_handle(cx));
        let model = cx.entity();
        Dialog::open(
            window,
            cx,
            DialogMode::Modeless,
            "Pair a device",
            size(px(680.), px(420.)),
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
        let review = self.review_request.as_ref().and_then(|id| {
            pairing
                .incoming
                .iter()
                .find(|r| &r.request_id == id)
                .or_else(|| pairing.incoming.first())
        });
        let mut body = div().flex().flex_col().gap_3();
        if !pairing.incoming.is_empty() {
            let mut select = Select::new(
                "pair-action",
                Some(review.map(|r| r.request_id.clone())),
                cx.listener(|v, request: &Option<String>, _, cx| {
                    v.review_request = request.clone();
                    cx.notify();
                }),
            )
            .option(None, "Pair with a code");
            for request in &pairing.incoming {
                select = select.option_with_icon(
                    Some(request.request_id.clone()),
                    format!("Request from {}", request.name),
                    Icon::device(request.platform),
                );
            }
            body = body.child(ui.labeled("Pairing requests", select));
        }
        if let Some(peer) = review {
            body = body
                .child(
                    ListItem::new(peer.name.clone())
                        .leading(ui.device_icon(peer.platform))
                        .detail(peer_detail(peer.platform, &peer.device_id)),
                )
                .child(ui.muted("Accept to let these devices open terminals and copy files."));
            return DialogContent::new(body, "Close")
                .action(self.answer_pairing(ui, peer, false, cx))
                .action(self.answer_pairing(ui, peer, true, cx));
        }
        let fields = self.pairing_fields(ui, pairing, cx).flex_1().min_w_0();
        let invitation = div()
            .flex()
            .flex_wrap()
            .gap_4()
            .when_some(self.pairing_qr.clone(), |row, qr| row.child(pairing_qr(qr)))
            .child(
                fields.when(pairing.code.is_some() && self.pairing_qr.is_none(), |d| {
                    d.child(ui.muted("Invitation too large for a QR code. Copy the code to pair."))
                }),
            );
        body = body.child(invitation);
        let dialog = DialogContent::new(body, "Done");
        if pairing.outgoing.is_some() {
            dialog.action(
                ui.button(Secondary, "cancel-pairing", "Cancel request")
                    .on_click(cx.listener(|v, _, w, cx| {
                        v.send_from_dialog(Command::CancelPairing, w, DialogSuccess::KeepOpen, cx);
                    })),
            )
        } else {
            dialog.action(
                ui.button(Primary, "pair-device", "Pair device")
                    .on_click(cx.listener(|v, _, w, cx| {
                        let code = v.pairing_input.read(cx).text().trim().to_owned();
                        v.send_from_dialog(
                            Command::JoinPairing { code },
                            w,
                            DialogSuccess::KeepOpen,
                            cx,
                        );
                    })),
            )
        }
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
        ui.button(variant, label, label)
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

    fn pairing_fields(&self, ui: Ui, pairing: &PairingStatus, cx: &Context<Self>) -> Div {
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
        div()
            .flex()
            .flex_col()
            .gap_3()
            .child(ui.labeled("Your pairing code", code))
            .when_some(pairing.outgoing.as_ref(), |d, id| {
                d.child(ui.muted(format!("Waiting for {} to confirm…", short_id(id))))
            })
            .when(pairing.outgoing.is_none(), |d| {
                d.child(ui.labeled("Other device’s pairing code", self.pairing_input.clone()))
            })
            .child(
                ui.muted("Pairing allows both devices to open terminals and copy files.")
                    .text_xs(),
            )
    }
}

fn pairing_qr(qr: std::rc::Rc<qrcode::QrCode>) -> impl IntoElement {
    canvas(
        |_, _, _| (),
        move |bounds, _, window, _| {
            // Use whole physical pixels for modules, including on fractional-scale displays.
            let scale = window.scale_factor();
            let modules = qr.width();
            let side = f32::from(bounds.size.width.min(bounds.size.height)) * scale;
            let step = (side / (modules + 8) as f32).floor();
            window.paint_quad(fill(bounds, rgb(0xffffff)));
            let inset = (side - modules as f32 * step) / 2.;
            let left = (f32::from(bounds.origin.x) * scale + inset).round();
            let top = (f32::from(bounds.origin.y) * scale + inset).round();
            for y in 0..modules {
                let mut x = 0;
                while x < modules {
                    let start = x;
                    while x < modules && qr[(x, y)] == qrcode::Color::Dark {
                        x += 1;
                    }
                    if x > start {
                        let origin = point(
                            px((left + start as f32 * step) / scale),
                            px((top + y as f32 * step) / scale),
                        );
                        let size = size(px((x - start) as f32 * step / scale), px(step / scale));
                        window.paint_quad(fill(Bounds::new(origin, size), rgb(0)));
                    }
                    x += 1;
                }
            }
        },
    )
    .size(px(250.))
    .flex_none()
}
