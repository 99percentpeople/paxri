use super::device::DeviceSettings;
use crate::components::{ButtonVariant::Secondary, Textarea, Ui, form_row, heading, row};
use gpui::{prelude::*, *};
use peerhatch_control::{Command, PortAccess, Status};

pub(super) struct PortsForm {
    connect: Entity<Textarea>,
    listen: Entity<Textarea>,
}

impl DeviceSettings {
    pub(super) fn port_access(&self, status: &Status, ui: Ui, cx: &Context<Self>) -> Div {
        let peer = self.peer.as_ref();
        let ports = status.port_access.get(peer).cloned().unwrap_or_default();
        let connect = ports.connect;
        let listen = ports
            .listen
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>();
        let mut header = row().child(heading("Port access"));
        let mut summary = div().min_w_0().flex().flex_col().gap_1();
        if connect.is_empty() && listen.is_empty() {
            summary = summary
                .child(ui.muted("This peer can connect only to services you publish to it."));
        } else {
            if !connect.is_empty() {
                summary = summary.child(ui.muted(format!("May connect to {}", connect.join(", "))));
            }
            if !listen.is_empty() {
                summary = summary.child(ui.muted(format!("May listen on {}", listen.join(", "))));
            }
        }
        if self.ports_form.is_none() {
            header = header.child(
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
                    cx.focus_view(&field, w);
                    v.ports_form = Some(PortsForm {
                        connect: field,
                        listen: Textarea::new(&listen.join("\n"), "127.0.0.1:8080", cx),
                    });
                    v.action_error = None;
                    cx.notify();
                })),
            );
        }
        let mut card = ui.card().child(header).child(ui.separator());
        if let Some(f) = &self.ports_form {
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
        } else {
            card = card.child(summary);
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
                    peer: self.peer.to_string(),
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
            .map(peerhatch_control::tcp_target)
            .collect::<Result<_, _>>()?,
        listen: entries(listen)
            .map(peerhatch_control::listen_address)
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
