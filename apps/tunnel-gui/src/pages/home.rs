use super::{peer_detail, short_id};
use crate::{
    Dashboard, Workspace,
    components::{
        ButtonVariant::{Ghost, Primary, Secondary},
        Icon, ListItem, Ui, actions, page, row,
    },
};
use gpui::{prelude::*, *};
use tunnel_control::Command;

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
        let page = page("Home");
        let Some(status) = &self.status else {
            return page.child(self.gateway_notice(ui, cx));
        };
        let pairing = &status.pairing;
        let mut card = ui.section("Pair devices");
        let mut code_actions = actions();
        if pairing.code.is_some() {
            let (icon, label) = if self.copied_feedback.is_some() {
                (Icon::Check, "Copied")
            } else {
                (Icon::Copy, "Copy code")
            };
            code_actions =
                code_actions.child(ui.icon_button(Ghost, "copy-code", icon, label).on_click(
                    cx.listener(|view, _, window, cx| view.copy_pairing_code(window, cx)),
                ));
        }
        code_actions = code_actions.child(
            ui.icon_button(Ghost, "refresh-code", Icon::Refresh, "Refresh code")
                .on_click(cx.listener(|v, _, _, cx| {
                    if !v.saving {
                        v.send(Command::RefreshPairingCode, cx);
                    }
                })),
        );
        card = card
            .child(row().child("Your pairing code").child(code_actions))
            .when(pairing.code.is_some(), |card| {
                card.child(self.local_code.clone())
            });
        for peer in &pairing.incoming {
            let approve_request = peer.request_id.clone();
            let decline_request = peer.request_id.clone();
            let approve_id = peer.device_id.clone();
            let decline_id = peer.device_id.clone();
            card = card.child(ui.separator()).child(
                ListItem::new(format!("Pair with {}?", peer.name))
                    .leading(ui.device_icon(peer.platform))
                    .detail(ui.muted(peer_detail(peer.platform, &peer.device_id)))
                    .trailing(
                        actions()
                            .child(
                                ui.button(
                                    Secondary,
                                    SharedString::from(format!("decline-{}", peer.device_id)),
                                    "Decline",
                                )
                                .on_click(cx.listener(
                                    move |v, _, _, cx| {
                                        if !v.saving {
                                            v.send(
                                                Command::AnswerPairing {
                                                    device_id: decline_id.clone(),
                                                    request_id: Some(decline_request.clone()),
                                                    accept: false,
                                                },
                                                cx,
                                            );
                                        }
                                    },
                                )),
                            )
                            .child(
                                ui.button(
                                    Primary,
                                    SharedString::from(format!("accept-{}", peer.device_id)),
                                    "Accept",
                                )
                                .on_click(cx.listener(
                                    move |v, _, _, cx| {
                                        if !v.saving {
                                            v.send(
                                                Command::AnswerPairing {
                                                    device_id: approve_id.clone(),
                                                    request_id: Some(approve_request.clone()),
                                                    accept: true,
                                                },
                                                cx,
                                            );
                                        }
                                    },
                                )),
                            ),
                    ),
            );
        }
        card = card.child(ui.separator());
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
            card = card.child("Pair with another device").child(
                row()
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(165.))
                            .child(self.pairing_input.clone()),
                    )
                    .child(
                        ui.button(Primary, "pair-device", "Pair device")
                            .on_click(cx.listener(|v, _, _, cx| {
                                if !v.saving {
                                    let code = v.pairing_input.read(cx).text().trim().to_owned();
                                    v.send(Command::JoinPairing { code }, cx);
                                }
                            })),
                    ),
            );
        }
        if let Some(message) = &pairing.message {
            card = card.child(ui.muted(message.clone()));
        }

        let mut peers = ui.section("Paired devices");
        if status.peers.is_empty() {
            peers = peers.child(ui.muted("No paired devices yet."));
        }
        for (name, id) in &status.peers {
            let platform = status.peer_platform(name);
            let peer = name.clone();
            let workspace = workspace.clone();
            peers = peers.child(ui.separator()).child(
                ListItem::new(name.clone())
                    .leading(ui.device_icon(platform))
                    .detail(ui.muted(peer_detail(platform, id)))
                    .trailing(
                        ui.button(
                            Secondary,
                            SharedString::from(format!("device-{name}")),
                            "Device settings",
                        )
                        .icon(Icon::Settings)
                        .on_click(move |_, w, cx| {
                            let _ = workspace.update(cx, |v, cx| v.open_device(&peer, w, cx));
                        }),
                    ),
            );
        }
        page.child(card).child(peers)
    }
}
