use super::{pages::labeled, *};

pub(super) struct SshForm {
    pub peer: String,
    user: Entity<Field>,
    service: Entity<Field>,
    identity: Entity<Field>,
}

pub(super) struct PortsForm {
    peer: String,
    connect: Entity<Field>,
    listen: Entity<Field>,
}

impl Dashboard {
    pub(super) fn edit_ssh(&mut self, peer: &str, window: &mut Window, cx: &mut Context<Self>) {
        if self.saving {
            return;
        }
        let p = self
            .status
            .as_ref()
            .and_then(|s| s.ssh_profiles.get(peer))
            .cloned()
            .unwrap_or_default();
        let user = Field::new(p.user.as_deref().unwrap_or(""), "OpenSSH default", cx);
        user.read(cx).focus.focus(window);
        self.ssh_form = Some(SshForm {
            peer: peer.into(),
            user,
            service: Field::new(&p.service, "ssh", cx),
            identity: Field::new(
                p.identity_file.as_deref().unwrap_or(""),
                "OpenSSH default",
                cx,
            ),
        });
        self.action_error = None;
        cx.notify();
    }

    pub(super) fn ssh_settings(&self, f: &SshForm, cx: &mut Context<Self>) -> Div {
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_3()
                    .child(labeled("Login name", f.user.clone()))
                    .child(labeled("SSH service", f.service.clone()))
                    .child(labeled("Identity file", f.identity.clone())),
            )
            .child(
                div()
                    .flex()
                    .gap_2()
                    .child(
                        primary("save-ssh", "Save SSH settings").on_click(cx.listener(
                            |v, _, _, cx| {
                                if v.saving {
                                    return;
                                }
                                let Some(f) = &v.ssh_form else { return };
                                let optional = |field: &Entity<Field>| {
                                    let text = field.read(cx).text.trim();
                                    (!text.is_empty()).then(|| text.to_owned())
                                };
                                let profile = SshProfile {
                                    user: optional(&f.user),
                                    service: optional(&f.service).unwrap_or_else(|| "ssh".into()),
                                    identity_file: optional(&f.identity),
                                };
                                let use_default = profile.user.is_none()
                                    && profile.service == "ssh"
                                    && profile.identity_file.is_none();
                                v.send(
                                    Command::SetSshProfile {
                                        peer: f.peer.clone(),
                                        profile: (!use_default).then_some(profile),
                                    },
                                    cx,
                                );
                            },
                        )),
                    )
                    .child(
                        button("cancel-ssh", "Cancel").on_click(cx.listener(|v, _, _, cx| {
                            if !v.saving {
                                v.ssh_form = None;
                                v.action_error = None;
                                cx.notify();
                            }
                        })),
                    ),
            )
    }

    pub(super) fn port_access(&self, status: &Status, cx: &mut Context<Self>) -> Div {
        let mut card = card().child(heading("Port access"));
        if status.peers.is_empty() {
            return card.child(details("No paired devices configured."));
        }
        for peer in status.peers.keys() {
            let ports = status.port_access.get(peer).cloned().unwrap_or_default();
            let connect = ports.connect.join(", ");
            let listen = ports
                .listen
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(", ");
            let name = peer.clone();
            let mut summary = div().flex().flex_col().child(peer.clone());
            if !connect.is_empty() {
                summary = summary.child(details(format!("May connect to {connect}")));
            }
            if !listen.is_empty() {
                summary = summary.child(details(format!("May listen on {listen}")));
            }
            if connect.is_empty() && listen.is_empty() {
                summary = summary.child(details("Published services only"));
            }
            card = card.child(line()).child(
                bar().child(summary).child(
                    button(
                        SharedString::from(format!("port-access-{peer}")),
                        "Edit access",
                    )
                    .on_click(cx.listener(move |v, _, w, cx| {
                        if v.saving {
                            return;
                        }
                        let field = Field::new(&connect, "127.0.0.1:3000", cx);
                        field.read(cx).focus.focus(w);
                        v.ports_form = Some(PortsForm {
                            peer: name.clone(),
                            connect: field,
                            listen: Field::new(&listen, "127.0.0.1:8080", cx),
                        });
                        v.action_error = None;
                        cx.notify();
                    })),
                ),
            );
            if let Some(f) = self.ports_form.as_ref().filter(|f| f.peer == *peer) {
                card = card
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .gap_3()
                            .child(labeled(
                                "May connect to these local targets",
                                f.connect.clone(),
                            ))
                            .child(labeled(
                                "May listen on these local addresses",
                                f.listen.clone(),
                            )),
                    )
                    .child(details(
                        "Separate addresses with commas. A port alone uses 127.0.0.1.",
                    ))
                    .child(
                        div()
                            .flex()
                            .gap_2()
                            .child(
                                primary("save-ports", "Save access")
                                    .on_click(cx.listener(|v, _, _, cx| v.save_ports(cx))),
                            )
                            .child(button("cancel-ports", "Cancel").on_click(cx.listener(
                                |v, _, _, cx| {
                                    if !v.saving {
                                        v.ports_form = None;
                                        v.action_error = None;
                                        cx.notify();
                                    }
                                },
                            ))),
                    );
            }
        }
        card
    }

    fn save_ports(&mut self, cx: &mut Context<Self>) {
        if self.saving {
            return;
        }
        let Some(f) = &self.ports_form else { return };
        let entries = |field: &Entity<Field>| {
            field
                .read(cx)
                .text
                .split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
        };
        let parse = || {
            Ok::<_, String>(PortAccess {
                connect: entries(&f.connect)
                    .map(tunnel_control::tcp_target)
                    .collect::<Result<_, _>>()?,
                listen: entries(&f.listen)
                    .map(tunnel_control::listen_address)
                    .collect::<Result<_, _>>()?,
            })
        };
        match parse() {
            Ok(ports) => self.send(
                Command::SetPortAccess {
                    peer: f.peer.clone(),
                    ports,
                },
                cx,
            ),
            Err(e) => {
                self.action_error = Some(e);
                cx.notify();
            }
        }
    }
}
