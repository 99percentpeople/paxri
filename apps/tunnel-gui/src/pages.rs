use super::*;

pub(super) fn labeled(label: &'static str, input: Entity<Field>) -> Div {
    div()
        .flex()
        .flex_col()
        .gap_1()
        .flex_1()
        .min_w(px(165.))
        .child(details(label))
        .child(input)
}
fn page_title(label: &'static str) -> Div {
    div()
        .text_3xl()
        .font_weight(FontWeight::SEMIBOLD)
        .child(label)
}
fn short_id(id: &str) -> String {
    if id.len() > 20 && id.is_ascii() {
        format!("{}…{}", &id[..10], &id[id.len() - 8..])
    } else {
        id.into()
    }
}
fn copy_button(id: &'static str, label: &'static str, text: String) -> Stateful<Div> {
    button(id, label)
        .on_click(move |_, _, cx| cx.write_to_clipboard(ClipboardItem::new_string(text.clone())))
}

impl Dashboard {
    fn gateway_notice(&self, cx: &mut Context<Self>) -> Div {
        card()
            .child(
                bar()
                    .child(heading(if self.error.is_some() {
                        "Gateway offline"
                    } else {
                        "Connecting to gateway…"
                    }))
                    .child(
                        button("retry", "Refresh")
                            .on_click(cx.listener(|v, _, _, cx| v.send(Command::Status, cx))),
                    ),
            )
            .child(details(self.error.clone().unwrap_or_else(|| {
                self.address
                    .clone()
                    .unwrap_or_else(tunnel_control::default_address)
                    .display()
                    .to_string()
            })))
    }

    pub(super) fn home(&self, workspace: WeakEntity<Workspace>, cx: &mut Context<Self>) -> Div {
        let page = div().flex().flex_col().gap_4().child(page_title("Home"));
        let Some(s) = &self.status else {
            return page.child(self.gateway_notice(cx));
        };
        let mut peers = card().child(heading("Paired devices"));
        if s.peers.is_empty() {
            peers = peers.child(details("No paired devices configured."));
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
            peers = peers.child(line()).child(
                bar()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_3()
                            .child(
                                div()
                                    .size(px(34.))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .rounded_md()
                                    .bg(rgb(0xdceee8))
                                    .text_color(rgb(0x246953))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(
                                        name.chars()
                                            .next()
                                            .unwrap_or('?')
                                            .to_uppercase()
                                            .to_string(),
                                    ),
                            )
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .child(name.clone())
                                    .child(details(info)),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                button(
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
                                button(
                                    SharedString::from(format!("ssh-{name}")),
                                    Icon::Terminal.label("Open SSH"),
                                )
                                .on_click(move |_, w, cx| {
                                    let _ =
                                        workspace.update(cx, |v, cx| v.open_terminal(&peer, w, cx));
                                }),
                            ),
                    ),
            );
            if let Some(form) = self.ssh_form.as_ref().filter(|f| f.peer == *name) {
                peers = peers.child(self.ssh_settings(form, cx));
            }
        }
        let mut rules = card().child(bar().child(heading("Port forwarding")).when(
            self.rule.is_none() && !s.peers.is_empty(),
            |d| {
                d.child(
                    primary("add-rule", Icon::Plus.label("Add rule"))
                        .on_click(cx.listener(|v, _, w, cx| v.open_rule(None, w, cx))),
                )
            },
        ));
        if let Some(f) = &self.rule {
            rules = rules.child(line()).child(self.rule_form(f, s, cx));
        }
        if s.forwards.is_empty() && self.rule.is_none() {
            rules = rules.child(details(if s.peers.is_empty() {
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
            let actions = div()
                .flex()
                .gap_1()
                .items_center()
                .child(
                    button(SharedString::from(format!("edit-{name}")), "Edit").on_click(
                        cx.listener(move |v, _, w, cx| {
                            if !v.saving {
                                v.open_rule(Some(&edit), w, cx);
                            }
                        }),
                    ),
                )
                .child(
                    button(
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
                    button(SharedString::from(format!("remove-{name}")), "Remove").on_click(
                        cx.listener(move |v, _, _, cx| {
                            if !v.saving {
                                v.removing = Some(remove.clone());
                                v.rule = None;
                                v.action_error = None;
                                cx.notify();
                            }
                        }),
                    ),
                );
            rules = rules.child(line()).child(
                bar()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .child(name.clone())
                                    .child(details(f.state().to_owned())),
                            )
                            .child(details(f.route())),
                    )
                    .child(actions),
            );
            if self.removing.as_ref() == Some(name) {
                let name = name.clone();
                rules = rules.child(
                    bar()
                        .child(details(format!("Remove the forwarding rule “{name}”?")))
                        .child(
                            div()
                                .flex()
                                .gap_2()
                                .child(button("cancel-remove", "Cancel").on_click(cx.listener(
                                    |v, _, _, cx| {
                                        if !v.saving {
                                            v.removing = None;
                                            cx.notify();
                                        }
                                    },
                                )))
                                .child(primary("confirm-remove", "Remove rule").on_click(
                                    cx.listener(move |v, _, _, cx| {
                                        if !v.saving {
                                            v.send(
                                                Command::RemoveForward { name: name.clone() },
                                                cx,
                                            );
                                        }
                                    }),
                                )),
                        ),
                );
            }
        }
        page.child(peers).child(rules)
    }

    fn rule_form(&self, f: &RuleForm, s: &Status, cx: &mut Context<Self>) -> Div {
        let name = if f.edit {
            div()
                .flex()
                .flex_col()
                .gap_1()
                .flex_1()
                .min_w(px(165.))
                .child(details("Rule name"))
                .child(f.name.read(cx).text.clone())
        } else {
            labeled("Rule name", f.name.clone())
        };
        let mut peers = div().flex().flex_wrap().items_center().gap_1();
        for peer in s.peers.keys() {
            let name = peer.clone();
            peers = peers.child(
                button(
                    SharedString::from(format!("destination-{peer}")),
                    peer.clone(),
                )
                .when(f.peer == *peer, |d| {
                    d.bg(rgb(0x234d46)).text_color(rgb(0xffffff))
                })
                .on_click(cx.listener(move |v, _, _, cx| {
                    if let Some(f) = &mut v.rule {
                        f.peer = name.clone();
                        cx.notify();
                    }
                })),
            );
        }
        let mut modes = div().flex().flex_wrap().gap_1();
        for (mode, id, label) in [
            (RuleMode::Local, "local-port", "Listen locally"),
            (RuleMode::Remote, "remote-port", "Listen on peer"),
            (RuleMode::Service, "named-service", "Named service"),
        ] {
            modes = modes.child(
                button(id, label)
                    .when(mode == f.mode, |d| {
                        d.bg(rgb(0x234d46)).text_color(rgb(0xffffff))
                    })
                    .on_click(cx.listener(move |v, _, _, cx| {
                        if !v.saving
                            && let Some(f) = &mut v.rule
                        {
                            f.mode = mode;
                            cx.notify();
                        }
                    })),
            );
        }
        div()
            .flex()
            .flex_col()
            .gap_3()
            .child(heading(if f.edit { "Edit rule" } else { "New rule" }))
            .child(modes)
            .child(div().flex().flex_wrap().gap_3().child(name).child(labeled(
                if f.mode == RuleMode::Remote {
                    "Peer listen address"
                } else {
                    "Local listen address"
                },
                f.listen.clone(),
            )))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_3()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .flex_1()
                            .min_w(px(165.))
                            .child(details("Paired device"))
                            .child(peers),
                    )
                    .child(match f.mode {
                        RuleMode::Service => labeled("Remote service", f.service.clone()),
                        RuleMode::Local => labeled("Remote TCP target", f.target.clone()),
                        RuleMode::Remote => labeled("Local TCP target", f.target.clone()),
                    }),
            )
            .child(
                div()
                    .flex()
                    .gap_2()
                    .child(
                        primary(
                            "save-rule",
                            if self.saving {
                                "Saving…"
                            } else {
                                "Save rule"
                            },
                        )
                        .on_click(cx.listener(|v, _, _, cx| v.save_rule(cx))),
                    )
                    .child(
                        button("cancel-rule", "Cancel").on_click(cx.listener(|v, _, _, cx| {
                            if !v.saving {
                                v.rule = None;
                                v.action_error = None;
                                cx.notify();
                            }
                        })),
                    ),
            )
    }

    pub(super) fn settings(&self, cx: &mut Context<Self>) -> Div {
        let page = div()
            .flex()
            .flex_col()
            .gap_4()
            .child(page_title("Settings"));
        let Some(s) = &self.status else {
            return page.child(self.gateway_notice(cx));
        };
        let mut device = card().child(heading("This device")).child(line());
        if let Some(field) = &self.name_field {
            device = device
                .child(labeled("Device name", field.clone()))
                .child(details(format!(
                    "Leave blank to use the computer name ({})",
                    s.computer_name
                )))
                .child(
                    div()
                        .flex()
                        .gap_2()
                        .child(
                            primary(
                                "save-name",
                                if self.saving {
                                    "Saving…"
                                } else {
                                    "Save name"
                                },
                            )
                            .on_click(cx.listener(|v, _, _, cx| {
                                if !v.saving
                                    && let Some(field) = &v.name_field
                                {
                                    let text = field.read(cx).text.trim();
                                    v.send(
                                        Command::SetDeviceName {
                                            name: (!text.is_empty()).then(|| text.to_owned()),
                                        },
                                        cx,
                                    );
                                }
                            })),
                        )
                        .child(button("cancel-name", "Cancel").on_click(cx.listener(
                            |v, _, _, cx| {
                                if !v.saving {
                                    v.name_field = None;
                                    v.action_error = None;
                                    cx.notify();
                                }
                            },
                        ))),
                );
        } else {
            let name = s.device_name.clone().unwrap_or_default();
            let system = s.computer_name.clone();
            device = device.child(
                bar()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(details("Device name"))
                            .child(s.display_name().to_owned())
                            .child(details(if s.device_name.is_none() {
                                "Using computer name"
                            } else {
                                "Custom name"
                            })),
                    )
                    .child(button("edit-name", "Change name").on_click(cx.listener(
                        move |v, _, w, cx| {
                            let field = Field::new(&name, &system, cx);
                            field.read(cx).focus.focus(w);
                            v.name_field = Some(field);
                            v.action_error = None;
                            cx.notify();
                        },
                    ))),
            );
        }
        let identity = card()
            .child(heading("Gateway"))
            .child(line())
            .child(
                bar()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .child(details("Device ID"))
                            .child(short_id(&s.device_id)),
                    )
                    .child(copy_button("copy-id", "Copy ID", s.device_id.clone())),
            )
            .child(bar().child(details("Pairing")).child(copy_button(
                "copy-ticket",
                "Copy connection ticket",
                s.ticket.clone(),
            )))
            .child(details(format!(
                "Local IPC · {}",
                self.address
                    .clone()
                    .unwrap_or_else(tunnel_control::default_address)
                    .display()
            )));
        let mut services = card().child(heading("Published services"));
        if s.services.is_empty() {
            services = services.child(details("No services published."));
        }
        for (name, target) in &s.services {
            services = services
                .child(line())
                .child(bar().child(name.clone()).child(details(target.clone())));
        }
        page.child(device)
            .child(self.port_access(s, cx))
            .child(identity)
            .child(services)
    }
}
