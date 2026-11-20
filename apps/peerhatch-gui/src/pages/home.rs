use super::{device_heading, overview, overview_columns, peer_detail, short_id};
use crate::{
    Dashboard, Workspace,
    components::{
        Button,
        ButtonVariant::{Ghost, Primary, Secondary},
        Icon, ListItem, Ui, actions, row,
    },
};
use gpui::{prelude::*, *};
use peerhatch_control::{Command, DevicePlatform, PairingPeer, PairingStatus};

impl Dashboard {
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

    pub(crate) fn home(
        &self,
        ui: Ui,
        workspace: WeakEntity<Workspace>,
        cx: &mut Context<Self>,
    ) -> Div {
        let (gateway, color) = if self.status.is_some() {
            ("Gateway online", ui.colors.success)
        } else if self.error.is_some() {
            ("Gateway offline", ui.colors.warning)
        } else {
            ("Connecting to gateway…", ui.colors.muted_foreground)
        };
        let header = device_heading(
            ui,
            "This device",
            self.local_name.clone(),
            DevicePlatform::current(),
            self.status.as_ref().map(|s| {
                (
                    s.device_id.as_str(),
                    peerhatch_config::peer_id::default_prefix(&s.device_id).into(),
                )
            }),
        )
        .child(ui.status_badge(gateway, color));
        let page = overview().child(header);
        let Some(status) = &self.status else {
            return page.child(self.gateway_notice(ui, cx));
        };
        page.when(!status.pairing.incoming.is_empty(), |page| {
            page.child(self.pairing_requests(ui, &status.pairing.incoming, cx))
        })
        .child(overview_columns(
            self.paired_devices(ui, status, workspace, cx),
            self.pairing_card(ui, &status.pairing, cx),
        ))
    }

    fn pairing_card(&self, ui: Ui, pairing: &PairingStatus, cx: &Context<Self>) -> Div {
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
                .on_click(cx.listener(|v, _, _, cx| {
                    if !v.saving {
                        v.send(Command::RefreshPairingCode, cx);
                    }
                })),
        );
        let mut card = ui
            .section("Pair a device")
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(ui.muted("Your pairing code"))
                    .child(code),
            )
            .child(ui.separator());
        if let Some(id) = &pairing.outgoing {
            card = card.child(
                row()
                    .child(ui.muted(format!("Waiting for {} to confirm…", short_id(id))))
                    .child(
                        ui.button(Secondary, "cancel-pairing", "Cancel request")
                            .on_click(cx.listener(|v, _, _, cx| {
                                if !v.saving {
                                    v.send(Command::CancelPairing, cx);
                                }
                            })),
                    ),
            );
        } else {
            let join = ui
                .button(Primary, "pair-device", "Pair device")
                .on_click(cx.listener(|v, _, _, cx| {
                    if !v.saving {
                        let code = v.pairing_input.read(cx).text().trim().to_owned();
                        v.send(Command::JoinPairing { code }, cx);
                    }
                }));
            card = card.child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(ui.muted("Other device’s pairing code"))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(div().flex_1().min_w_0().child(self.pairing_input.clone()))
                            .child(join),
                    ),
            );
        }
        card.when_some(pairing.message.clone(), |card, message| {
            card.child(ui.muted(message))
        })
        .child(
            ui.muted("Pairing allows both devices to open terminals and copy files.")
                .text_xs(),
        )
    }

    fn pairing_requests(&self, ui: Ui, incoming: &[PairingPeer], cx: &Context<Self>) -> Div {
        let mut card = ui.section("Pairing requests");
        for peer in incoming {
            let answer = |accept| -> Button {
                let id = peer.device_id.clone();
                let request = peer.request_id.clone();
                let (variant, label) = if accept {
                    (Primary, "Accept")
                } else {
                    (Secondary, "Decline")
                };
                ui.button(
                    variant,
                    SharedString::from(format!("{label}-{request}")),
                    label,
                )
                .on_click(cx.listener(move |v, _, _, cx| {
                    if !v.saving {
                        v.send(
                            Command::AnswerPairing {
                                device_id: id.clone(),
                                request_id: Some(request.clone()),
                                accept,
                            },
                            cx,
                        );
                    }
                }))
            };
            card = card.child(ui.separator()).child(
                ListItem::new(peer.name.clone())
                    .leading(ui.device_icon(peer.platform))
                    .detail(ui.muted(peer_detail(peer.platform, &peer.device_id)))
                    .trailing(actions().child(answer(false)).child(answer(true))),
            );
        }
        card
    }
}
