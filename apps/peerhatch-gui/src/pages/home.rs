use super::{device_heading, overview, peer_detail, short_id};
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
use peerhatch_control::{Command, DevicePlatform, PairingPeer, PairingStatus};

#[derive(PartialEq, Eq)]
pub(crate) enum DevicesPane {
    Peer(String),
    Forwards,
}

impl Dashboard {
    pub(crate) fn reconcile_device_selection(&mut self, status: &peerhatch_control::Status) {
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
        if self
            .forward_filter
            .as_ref()
            .is_some_and(|peer| !status.peers.contains_key(peer))
        {
            self.forward_filter = None;
        }
    }

    pub(super) fn show_devices_pane(&mut self, pane: DevicesPane, cx: &mut Context<Self>) {
        self.action_error = None;
        self.devices_pane = Some(pane);
        self.home_scroll.set_offset(Point::default());
        cx.notify();
    }

    fn copy_pairing_code(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let code = self.local_code.read(cx).text().to_owned();
        cx.write_to_clipboard(ClipboardItem::new_string(code.clone()));
        // Clipboard ownership can change asynchronously. Check the value after the platform event.
        let dashboard = cx.weak_entity();
        window.on_next_frame(move |_, cx| {
            let _ = dashboard.update(cx, |view, cx| {
                if view.local_code.read(cx).text() != code {
                    return;
                }
                let copied = cx.read_from_clipboard().and_then(|item| item.text());
                view.copied_feedback = (copied.as_deref() == Some(code.as_str())).then(|| {
                    cx.spawn(async |this, cx| {
                        cx.background_executor()
                            .timer(std::time::Duration::from_millis(1500))
                            .await;
                        let _ = this.update(cx, |view, cx| {
                            view.copied_feedback = None;
                            cx.notify();
                        });
                    })
                });
                cx.notify();
            });
        });
        cx.notify();
    }

    fn device_sidebar(&self, ui: Ui, cx: &Context<Self>) -> DockNavigation {
        let requests = self.status.as_ref().map_or(0, |s| s.pairing.incoming.len());
        let label = div()
            .min_w_0()
            .flex_1()
            .flex()
            .items_center()
            .justify_between()
            .child(single_line(div()).child("Add Device"))
            .when(requests > 0, |r| {
                r.child(ui.muted(requests.to_string()).text_xs())
            });
        let state = if self.status.is_some() {
            "Gateway online"
        } else if self.error.is_some() {
            "Gateway offline"
        } else {
            "Connecting…"
        };
        let add = ui
            .button(Ghost, "add-device", label)
            .row()
            .icon(Icon::Plus)
            .tooltip(crate::components::tooltip("Pair a device".into()))
            .on_click(cx.listener(|v, _, w, cx| v.open_pairing(w, cx)));
        let body = div()
            .min_h_0()
            .min_w_0()
            .flex_1()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .id("paired-list")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .when_some(self.status.as_ref(), |d, s| {
                        d.child(self.paired_devices(ui, s, cx))
                    })
                    .child(add)
                    .when_some(self.status.as_ref(), |d,s| {
                        if s.pairing.pending_unpairs.is_empty() {
                            d.when_some(s.pairing.message.clone(), |d,m| d.child(ui.muted(m).text_xs()))
                        } else {
                            d.child(ui.muted(format!("Awaiting unpair confirmation: {}", s.pairing.pending_unpairs.join(", "))).text_xs())
                        }
                    }),
            )
            .child(
                ui.button(Ghost, "all-forwards", "Port forwarding")
                    .icon(Icon::Forward)
                    .row()
                    .pressed(self.devices_pane == Some(DevicesPane::Forwards))
                    .on_click(cx.listener(|v, _, _, cx| {
                        v.forward_filter = None;
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
        let content = match (&self.devices_pane, &self.status) {
            (_, None) => self.gateway_notice(ui, cx),
            (Some(DevicesPane::Peer(peer)), Some(status)) => {
                self.device_detail(peer, status, ui, cx)
            }
            (Some(DevicesPane::Forwards), Some(status)) => self.forwarding(status, ui, width, cx),
            (_, Some(status)) => overview()
                .child(device_heading(
                    ui,
                    "This device",
                    self.local_name.clone(),
                    DevicePlatform::current(),
                    Some((
                        &status.device_id,
                        peerhatch_config::peer_id::default_prefix(&status.device_id).into(),
                    )),
                ))
                .child(ui.section("Paired devices").child(
                    ui.muted("Choose Add Device in the device list to pair your first device."),
                )),
        };
        DockPanel::scroll("devices-content", self.home_scroll.clone(), content)
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
            size(px(540.), px(370.)),
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
                        .detail(ui.muted(peer_detail(peer.platform, &peer.device_id))),
                )
                .child(ui.muted("Accept to let these devices open terminals and copy files."));
            return DialogContent::new(body, "Close")
                .action(self.answer_pairing(ui, peer, false, cx))
                .action(self.answer_pairing(ui, peer, true, cx));
        }
        body = body.child(self.pairing_fields(ui, pairing, cx));
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
            let (icon, label) = if self.copied_feedback.is_some() {
                (Icon::Check, "Copied")
            } else {
                (Icon::Copy, "Copy code")
            };
            code = code
                .child(div().flex_1().min_w_0().child(self.local_code.clone()))
                .child(
                    ui.icon_button(Ghost, "copy-code", icon, label)
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
            .when_some(pairing.message.clone(), |d, m| d.child(ui.muted(m)))
            .child(
                ui.muted("Pairing allows both devices to open terminals and copy files.")
                    .text_xs(),
            )
    }
}
