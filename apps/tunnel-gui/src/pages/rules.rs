use crate::{
    Dashboard, RuleForm, RuleMode,
    components::{Select, Ui, form_row, heading},
};
use gpui::{prelude::*, *};
use tunnel_control::Status;

impl Dashboard {
    pub(super) fn rule_form(
        &self,
        f: &RuleForm,
        s: &Status,
        ui: Ui,
        cx: &mut Context<Self>,
    ) -> Div {
        let name = if f.edit {
            ui.labeled("Rule name", f.name.read(cx).text().to_owned())
        } else {
            ui.labeled("Rule name", f.name.clone())
        };
        let mut peers = Select::new(
            "destination",
            f.peer.clone(),
            cx.listener(|v, name: &String, _, cx| {
                if !v.saving
                    && let Some(f) = &mut v.rule
                {
                    f.peer = name.clone();
                    cx.notify();
                }
            }),
        )
        .placeholder(if s.peers.is_empty() {
            "No paired devices"
        } else {
            "Choose a device"
        });
        for peer in s.peers.keys() {
            peers = peers.option(peer.clone(), peer.clone());
        }
        let modes = Select::new(
            "forwarding-mode",
            f.mode,
            cx.listener(|v, mode: &RuleMode, _, cx| {
                if !v.saving
                    && let Some(f) = &mut v.rule
                {
                    f.mode = *mode;
                    cx.notify();
                }
            }),
        )
        .option(RuleMode::Local, "Listen locally")
        .option(RuleMode::Remote, "Listen on peer")
        .option(RuleMode::Service, "Named service");
        div()
            .flex()
            .flex_col()
            .gap_3()
            .child(heading(if f.edit { "Edit rule" } else { "New rule" }))
            .child(ui.labeled("Forwarding mode", modes))
            .child(form_row().child(name).child(ui.labeled(
                if f.mode == RuleMode::Remote {
                    "Peer listen address"
                } else {
                    "Local listen address"
                },
                f.listen.clone(),
            )))
            .child(
                form_row()
                    .child(ui.labeled("Paired device", peers))
                    .child(match f.mode {
                        RuleMode::Service => ui.labeled("Remote service", f.service.clone()),
                        RuleMode::Local => ui.labeled("Remote TCP target", f.target.clone()),
                        RuleMode::Remote => ui.labeled("Local TCP target", f.target.clone()),
                    }),
            )
            .child(ui.form_actions(
                "rule",
                if self.saving {
                    "Saving…"
                } else {
                    "Save rule"
                },
                cx.listener(|v, _, _, cx| v.save_rule(cx)),
                cx.listener(|v, _, _, cx| {
                    if !v.saving {
                        v.rule = None;
                        v.action_error = None;
                        cx.notify();
                    }
                }),
            ))
    }
}
