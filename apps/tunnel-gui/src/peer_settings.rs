use super::*;
use crate::components::{ButtonVariant::Secondary, ListItem, Textarea, Ui, form_row};

pub(super) struct SshForm {
    pub peer: String,
    user: Entity<Input>,
    service: Entity<Input>,
    identity: Entity<Input>,
}

pub(super) struct PortsForm {
    peer: String,
    connect: Entity<Textarea>,
    listen: Entity<Textarea>,
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
        let user = Input::new(p.user.as_deref().unwrap_or(""), "OpenSSH default", cx);
        user.read(cx).focus.focus(window);
        self.ssh_form = Some(SshForm {
            peer: peer.into(),
            user,
            service: Input::new(&p.service, "ssh", cx),
            identity: Input::new(
                p.identity_file.as_deref().unwrap_or(""),
                "OpenSSH default",
                cx,
            ),
        });
        self.action_error = None;
        cx.notify();
    }

    pub(super) fn ssh_settings(&self, f: &SshForm, ui: Ui, cx: &mut Context<Self>) -> Div {
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                form_row()
                    .child(ui.labeled("Login name", f.user.clone()))
                    .child(ui.labeled("SSH service", f.service.clone()))
                    .child(ui.labeled("Identity file", f.identity.clone())),
            )
            .child(ui.form_actions(
                "ssh",
                "Save SSH settings",
                cx.listener(|v, _, _, cx| {
                    if v.saving {
                        return;
                    }
                    let Some(f) = &v.ssh_form else { return };
                    let optional = |field: &Entity<Input>| {
                        let text = field.read(cx).text().trim();
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
                }),
                cx.listener(|v, _, _, cx| {
                    if !v.saving {
                        v.ssh_form = None;
                        v.action_error = None;
                        cx.notify();
                    }
                }),
            ))
    }

    pub(super) fn port_access(&self, status: &Status, ui: Ui, cx: &mut Context<Self>) -> Div {
        let mut card = ui.section("Port access");
        if status.peers.is_empty() {
            return card.child(ui.muted("No paired devices configured."));
        }
        for peer in status.peers.keys() {
            let ports = status.port_access.get(peer).cloned().unwrap_or_default();
            let connect = ports.connect;
            let listen = ports
                .listen
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>();
            let name = peer.clone();
            let mut summary = ListItem::new(peer.clone());
            if !connect.is_empty() {
                summary =
                    summary.detail(ui.muted(format!("May connect to {}", connect.join(", "))));
            }
            if !listen.is_empty() {
                summary = summary.detail(ui.muted(format!("May listen on {}", listen.join(", "))));
            }
            if connect.is_empty() && listen.is_empty() {
                summary = summary.detail(ui.muted("Published services only"));
            }
            card = card.child(ui.separator()).child(
                summary.trailing(
                    ui.button(
                        Secondary,
                        SharedString::from(format!("port-access-{peer}")),
                        "Edit access",
                    )
                    .on_click(cx.listener(move |v, _, w, cx| {
                        if v.saving {
                            return;
                        }
                        let field = Textarea::new(&connect.join("\n"), "127.0.0.1:3000", cx);
                        field.read(cx).focus.focus(w);
                        v.ports_form = Some(PortsForm {
                            peer: name.clone(),
                            connect: field,
                            listen: Textarea::new(&listen.join("\n"), "127.0.0.1:8080", cx),
                        });
                        v.action_error = None;
                        cx.notify();
                    })),
                ),
            );
            if let Some(f) = self.ports_form.as_ref().filter(|f| f.peer == *peer) {
                card = card
                    .child(
                        form_row()
                            .child(
                                ui.labeled("May connect to these local targets", f.connect.clone()),
                            )
                            .child(
                                ui.labeled("May listen on these local addresses", f.listen.clone()),
                            ),
                    )
                    .child(ui.muted("Enter one address per line or separate addresses with commas. A port alone uses 127.0.0.1."))
                    .child(ui.form_actions(
                        "ports",
                        "Save access",
                        cx.listener(|v, _, _, cx| v.save_ports(cx)),
                        cx.listener(|v, _, _, cx| {
                            if !v.saving {
                                v.ports_form = None;
                                v.action_error = None;
                                cx.notify();
                            }
                        }),
                    ));
            }
        }
        card
    }

    fn save_ports(&mut self, cx: &mut Context<Self>) {
        if self.saving {
            return;
        }
        let Some(f) = &self.ports_form else { return };
        match parse_port_access(f.connect.read(cx).text(), f.listen.read(cx).text()) {
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

fn parse_port_access(connect: &str, listen: &str) -> Result<PortAccess, String> {
    fn entries(text: &str) -> impl Iterator<Item = &str> {
        text.split([',', '\n'])
            .map(str::trim)
            .filter(|s| !s.is_empty())
    }
    Ok(PortAccess {
        connect: entries(connect)
            .map(tunnel_control::tcp_target)
            .collect::<Result<_, _>>()?,
        listen: entries(listen)
            .map(tunnel_control::listen_address)
            .collect::<Result<_, _>>()?,
    })
}

#[cfg(test)]
mod form_controls {
    use super::parse_port_access;

    #[test]
    fn port_textareas_accept_mixed_separators_and_preserve_order() {
        let ports =
            parse_port_access("22, localhost:3000\r\n\n[::1]:8443,", "8080\n[::1]:443").unwrap();
        assert_eq!(
            ports.connect,
            ["127.0.0.1:22", "localhost:3000", "[::1]:8443"]
        );
        assert_eq!(
            ports
                .listen
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>(),
            ["127.0.0.1:8080", "[::1]:443"]
        );
    }

    #[test]
    fn port_textareas_clear_empty_lists_and_reject_invalid_entries() {
        let ports = parse_port_access(" ,\n", "").unwrap();
        assert!(ports.connect.is_empty() && ports.listen.is_empty());
        assert!(parse_port_access("22\nno-port", "").is_err());
        assert!(parse_port_access("", "22,invalid").is_err());
    }
}
