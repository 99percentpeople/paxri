use super::device::DeviceSettings;
use crate::{
    Input,
    components::{
        ButtonVariant::{Destructive, Primary, Secondary},
        Icon, Select, Ui, actions, form_row, heading, row,
    },
};
use gpui::{prelude::*, *};
use tunnel_control::{Command, Destination, Direction, Forward, Status};

pub(super) struct RuleForm {
    edit: bool,
    name: Entity<Input>,
    listen: Entity<Input>,
    service: Entity<Input>,
    target: Entity<Input>,
    mode: RuleMode,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum RuleMode {
    Local,
    Remote,
    Service,
}

impl DeviceSettings {
    pub(super) fn open_rule(
        &mut self,
        name: Option<&str>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.saving {
            return;
        }
        let Some(status) = &self.dashboard.read(cx).status else {
            return;
        };
        let f = name
            .and_then(|n| status.forwards.get(n))
            .filter(|f| f.peer == self.peer.as_ref())
            .cloned();
        let f = f.as_ref();
        if name.is_some() && f.is_none() {
            return;
        }
        let form = RuleForm {
            edit: f.is_some(),
            name: Input::new(name.unwrap_or_default(), "Rule name", cx),
            listen: Input::new(
                f.map_or("127.0.0.1:8080", |f| &f.listen),
                "127.0.0.1:8080",
                cx,
            ),
            service: Input::new(f.map_or("", |f| &f.service), "web", cx),
            target: Input::new(
                f.and_then(|f| f.target.as_deref()).unwrap_or(""),
                "127.0.0.1:3000",
                cx,
            ),
            mode: match f {
                Some(f) if f.direction == Direction::Remote => RuleMode::Remote,
                Some(f) if f.target.is_none() => RuleMode::Service,
                _ => RuleMode::Local,
            },
        };
        if form.edit { &form.listen } else { &form.name }
            .read(cx)
            .focus
            .focus(window);
        self.rule = Some(form);
        self.removing = None;
        self.action_error = None;
        cx.notify();
    }

    fn save_rule(&mut self, cx: &mut Context<Self>) {
        if self.saving {
            return;
        }
        let Some(f) = &self.rule else { return };
        let name = f.name.read(cx).text().to_owned();
        let name = if f.edit { name } else { name.trim().to_owned() };
        let Ok(listen) = tunnel_control::listen_address(f.listen.read(cx).text().trim()) else {
            self.action_error = Some(
                "Enter a listen address with a port, such as 127.0.0.1:8080 or [::1]:8080.".into(),
            );
            cx.notify();
            return;
        };
        if name.is_empty() {
            self.action_error = Some("Enter a rule name.".into());
            cx.notify();
            return;
        }
        let destination = if f.mode == RuleMode::Service {
            Destination::Service(f.service.read(cx).text().trim().to_owned())
        } else {
            match tunnel_control::tcp_target(f.target.read(cx).text().trim()) {
                Ok(target) => Destination::Target(target),
                Err(e) => {
                    self.action_error = Some(e);
                    cx.notify();
                    return;
                }
            }
        };
        self.send(
            Command::SaveForward {
                name,
                edit: f.edit,
                expected_peer: f.edit.then(|| self.peer.to_string()),
                rule: Forward {
                    listen,
                    peer: self.peer.to_string(),
                    destination,
                    direction: if f.mode == RuleMode::Remote {
                        Direction::Remote
                    } else {
                        Direction::Local
                    },
                },
            },
            cx,
        );
    }

    pub(super) fn rule_form(&self, f: &RuleForm, ui: Ui, cx: &Context<Self>) -> Div {
        let name = if f.edit {
            ui.labeled("Rule name", f.name.read(cx).text().to_owned())
        } else {
            ui.labeled("Rule name", f.name.clone())
        };
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
        .placeholder("Choose a forwarding mode")
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
            .child(match f.mode {
                RuleMode::Service => ui.labeled("Remote service", f.service.clone()),
                RuleMode::Local => ui.labeled("Remote TCP target", f.target.clone()),
                RuleMode::Remote => ui.labeled("Local TCP target", f.target.clone()),
            })
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

impl DeviceSettings {
    pub(super) fn forwarding(&self, s: &Status, ui: Ui, cx: &Context<Self>) -> Div {
        let mut rules = ui
            .card()
            .child(
                row()
                    .child(heading("Port forwarding"))
                    .when(self.rule.is_none(), |d| {
                        d.child(
                            ui.button(Primary, "add-rule", "Add rule")
                                .icon(Icon::Plus)
                                .on_click(cx.listener(|v, _, w, cx| v.open_rule(None, w, cx))),
                        )
                    }),
            );
        if let Some(f) = &self.rule {
            rules = rules.child(ui.separator()).child(self.rule_form(f, ui, cx));
        }
        if !s.forwards.values().any(|f| f.peer == self.peer.as_ref()) && self.rule.is_none() {
            rules = rules.child(ui.muted("No forwarding rules for this device."));
        }
        for (name, f) in s
            .forwards
            .iter()
            .filter(|(_, f)| f.peer == self.peer.as_ref())
        {
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
                                expected_peer: Some(v.peer.to_string()),
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
                                                    Command::RemoveForward {
                                                        name: name.clone(),
                                                        expected_peer: Some(v.peer.to_string()),
                                                    },
                                                    cx,
                                                );
                                            }
                                        })),
                                ),
                        ),
                );
            }
        }
        rules
    }
}
