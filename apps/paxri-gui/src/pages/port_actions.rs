use crate::{
    Dashboard,
    components::{ButtonVariant::Destructive, Dialog, DialogContent, DialogMode, DialogSuccess},
};
use gpui::{prelude::*, *};
use paxri_control::{Command, ForwardStatus, PeerForward};
use paxri_ui::components::forwarding::{RuleAction, RuleMenu};

impl Dashboard {
    pub(super) fn port_actions(
        &self,
        name: &str,
        owner: Option<&str>,
        rule: &ForwardStatus,
        display_name: &str,
        cx: &Context<Self>,
    ) -> RuleMenu {
        let dashboard = cx.entity();
        let key = name.to_owned();
        let peer = rule.peer.clone();
        let remote = owner.map(str::to_owned);
        let prompt = format!(
            "Remove the forwarding rule for {}?",
            rule.route_for(display_name)
        );
        RuleMenu::new(
            format!("rule-actions-{owner:?}/{name}"),
            name,
            rule.enabled(),
            move |action, w, cx| match action {
                RuleAction::Edit => dashboard.update(cx, |v, cx| {
                    v.open_rule(remote.as_deref(), Some(&key), w, cx)
                }),
                RuleAction::SetEnabled(enabled) => dashboard.update(cx, |v, cx| {
                    if v.saving {
                        return;
                    }
                    let command = match &remote {
                        Some(owner) => Command::ManagePeerForward {
                            owner: owner.clone(),
                            action: PeerForward::SetEnabled {
                                name: key.clone(),
                                enabled,
                            },
                        },
                        None => Command::SetForward {
                            name: key.clone(),
                            expected_peer: Some(peer.clone()),
                            enabled,
                        },
                    };
                    v.send(command, cx);
                }),
                RuleAction::Remove => {
                    let name = key.clone();
                    let peer = peer.clone();
                    let remote = remote.clone();
                    let prompt = prompt.clone();
                    Dialog::open(
                        w,
                        cx,
                        DialogMode::Modal,
                        "Remove forwarding rule?",
                        size(px(400.), px(180.)),
                        dashboard.clone(),
                        None,
                        move |_, ui, cx| {
                            let name = name.clone();
                            let peer = peer.clone();
                            let remote = remote.clone();
                            DialogContent::new(prompt.clone(), "Cancel").action(
                                ui.button(Destructive, "confirm-remove", "Remove rule")
                                    .on_click(cx.listener(move |v, _, w, cx| {
                                        let command = match &remote {
                                            Some(owner) => Command::ManagePeerForward {
                                                owner: owner.clone(),
                                                action: PeerForward::Remove { name: name.clone() },
                                            },
                                            None => Command::RemoveForward {
                                                name: name.clone(),
                                                expected_peer: Some(peer.clone()),
                                            },
                                        };
                                        v.send_from_dialog(command, w, DialogSuccess::Close, cx);
                                    })),
                            )
                        },
                    );
                }
            },
        )
    }
}
