use super::short_id;
use crate::{
    Dashboard, Workspace,
    components::{
        ButtonVariant::{Destructive, Primary, Secondary},
        Icon, ListItem, Ui, actions, heading, page, row,
    },
};
use gpui::{prelude::*, *};
use tunnel_control::Command;

impl Dashboard {
    pub(crate) fn home(
        &self,
        ui: Ui,
        workspace: WeakEntity<Workspace>,
        cx: &mut Context<Self>,
    ) -> Div {
        let page = page("Home");
        let Some(s) = &self.status else {
            return page.child(self.gateway_notice(ui, cx));
        };
        let mut peers = ui.section("Paired devices");
        if s.peers.is_empty() {
            peers = peers.child(ui.muted("No paired devices configured."));
        }
        for (name, id) in &s.peers {
            let peer = name.clone();
            let settings_peer = name.clone();
            let mut info = short_id(id);
            if let Some(profile) = s.ssh_profiles.get(name) {
                let login = profile.user.as_deref().unwrap_or("OpenSSH default");
                info = format!("{login} · {} · {info}", profile.service);
            }
            let workspace = workspace.clone();
            peers = peers.child(ui.separator()).child(
                ListItem::new(name.clone())
                    .leading(ui.avatar(name))
                    .detail(ui.muted(info))
                    .trailing(
                        actions()
                            .child(
                                ui.button(
                                    Secondary,
                                    SharedString::from(format!("ssh-settings-{name}")),
                                    "SSH settings",
                                )
                                .on_click(
                                    cx.listener(move |v, _, w, cx| {
                                        v.edit_ssh(&settings_peer, w, cx)
                                    }),
                                ),
                            )
                            .child(
                                ui.button(
                                    Secondary,
                                    SharedString::from(format!("ssh-{name}")),
                                    "Open SSH",
                                )
                                .icon(Icon::Terminal)
                                .on_click(move |_, w, cx| {
                                    let _ =
                                        workspace.update(cx, |v, cx| v.open_terminal(&peer, w, cx));
                                }),
                            ),
                    ),
            );
            if let Some(form) = self.ssh_form.as_ref().filter(|f| f.peer == *name) {
                peers = peers.child(self.ssh_settings(form, ui, cx));
            }
        }
        let mut rules = ui
            .card()
            .child(row().child(heading("Port forwarding")).when(
                self.rule.is_none() && !s.peers.is_empty(),
                |d| {
                    d.child(
                        ui.button(Primary, "add-rule", "Add rule")
                            .icon(Icon::Plus)
                            .on_click(cx.listener(|v, _, w, cx| v.open_rule(None, w, cx))),
                    )
                },
            ));
        if let Some(f) = &self.rule {
            rules = rules
                .child(ui.separator())
                .child(self.rule_form(f, s, ui, cx));
        }
        if s.forwards.is_empty() && self.rule.is_none() {
            rules = rules.child(ui.muted(if s.peers.is_empty() {
                "Configure a paired device to add a forwarding rule."
            } else {
                "No forwarding rules yet."
            }));
        }
        for (name, f) in &s.forwards {
            let edit = name.clone();
            let toggle = name.clone();
            let remove = name.clone();
            let enabled = f.enabled();
            let rule_actions = div()
                .flex()
                .gap_1()
                .items_center()
                .child(
                    ui.button(
                        Secondary,
                        SharedString::from(format!("edit-{name}")),
                        "Edit",
                    )
                    .on_click(cx.listener(move |v, _, w, cx| {
                        if !v.saving {
                            v.open_rule(Some(&edit), w, cx);
                        }
                    })),
                )
                .child(
                    ui.button(
                        Secondary,
                        SharedString::from(format!("toggle-{name}")),
                        if enabled { "Pause" } else { "Resume" },
                    )
                    .on_click(cx.listener(move |v, _, _, cx| {
                        v.send(
                            Command::SetForward {
                                name: toggle.clone(),
                                enabled: !enabled,
                            },
                            cx,
                        )
                    })),
                )
                .child(
                    ui.button(
                        Secondary,
                        SharedString::from(format!("remove-{name}")),
                        "Remove",
                    )
                    .on_click(cx.listener(move |v, _, _, cx| {
                        if !v.saving {
                            v.removing = Some(remove.clone());
                            v.rule = None;
                            v.action_error = None;
                            cx.notify();
                        }
                    })),
                );
            rules = rules.child(ui.separator()).child(
                row()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(
                                actions()
                                    .child(name.clone())
                                    .child(ui.muted(f.state().to_owned())),
                            )
                            .child(ui.muted(f.route())),
                    )
                    .child(rule_actions),
            );
            if self.removing.as_ref() == Some(name) {
                let name = name.clone();
                rules = rules.child(
                    row()
                        .child(ui.muted(format!("Remove the forwarding rule “{name}”?")))
                        .child(
                            actions()
                                .child(ui.button(Secondary, "cancel-remove", "Cancel").on_click(
                                    cx.listener(|v, _, _, cx| {
                                        if !v.saving {
                                            v.removing = None;
                                            cx.notify();
                                        }
                                    }),
                                ))
                                .child(
                                    ui.button(Destructive, "confirm-remove", "Remove rule")
                                        .on_click(cx.listener(move |v, _, _, cx| {
                                            if !v.saving {
                                                v.send(
                                                    Command::RemoveForward { name: name.clone() },
                                                    cx,
                                                );
                                            }
                                        })),
                                ),
                        ),
                );
            }
        }
        page.child(peers).child(rules)
    }
}
