use crate::{
    Dashboard, Input,
    components::{
        ButtonVariant::{Ghost, Primary},
        Dialog, DialogContent, DialogMode, DialogSuccess, Icon, Select, Ui, form_row, heading, row,
    },
};
use gpui::{prelude::*, *};
use peerhatch_control::{Command, Destination, Direction, Forward, Status};

struct RuleForm {
    dashboard: Entity<Dashboard>,
    original_peer: Option<String>,
    peer: String,
    name: Entity<Input>,
    listen: Entity<Input>,
    service: Entity<Input>,
    original_service: Option<String>,
    service_list: ServiceList,
    lookup: Option<Task<()>>,
    target: Entity<Input>,
    mode: RuleMode,
}

enum ServiceList {
    Loading,
    Ready(Vec<String>),
    Unavailable(String),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum RuleMode {
    Local,
    Remote,
    Service,
}

impl Dashboard {
    pub(super) fn open_rule(
        &mut self,
        name: Option<&str>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.saving {
            return;
        }
        let Some(status) = &self.status else {
            return;
        };
        let f = name.and_then(|n| status.forwards.get(n));
        if name.is_some() && f.is_none() {
            return;
        }
        let Some(peer) = f
            .map(|rule| &rule.peer)
            .or(self.forward_filter.as_ref())
            .or_else(|| status.peers.keys().next())
        else {
            return;
        };
        let form = RuleForm {
            dashboard: cx.entity(),
            original_peer: f.map(|rule| rule.peer.clone()),
            peer: peer.clone(),
            name: Input::new(name.unwrap_or_default(), "Rule name", cx),
            listen: Input::new(
                f.map_or("127.0.0.1:8080", |f| &f.listen),
                "127.0.0.1:8080",
                cx,
            ),
            service: Input::new(
                f.filter(|f| f.target.is_none()).map_or("", |f| &f.service),
                "Service name",
                cx,
            ),
            original_service: f.filter(|f| f.target.is_none()).map(|f| f.service.clone()),
            service_list: ServiceList::Loading,
            lookup: None,
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
        let title = if form.original_peer.is_some() {
            "Edit forwarding rule"
        } else {
            "New forwarding rule"
        };
        let initial = if form.original_peer.is_some() {
            &form.listen
        } else {
            &form.name
        };
        let focus = initial.read(cx).focus_handle(cx);
        let model = cx.new(|cx| {
            cx.observe(&form.dashboard, |_, _, cx| cx.notify()).detach();
            form
        });
        if model.read(cx).mode == RuleMode::Service {
            let form = model.downgrade();
            cx.defer(move |cx| {
                let _ = form.update(cx, |v, cx| v.load_services(cx));
            });
        }
        Dialog::open(
            window,
            cx,
            DialogMode::Modal,
            title,
            size(px(520.), px(370.)),
            model,
            Some(focus),
            RuleForm::render_form,
        );
    }
}

impl RuleForm {
    fn load_services(&mut self, cx: &mut Context<Self>) {
        self.service_list = ServiceList::Loading;
        let dashboard = self.dashboard.read(cx);
        let path = dashboard
            .active_address()
            .unwrap_or_else(peerhatch_control::default_address);
        let peer = self.peer.clone();
        let (mut send, receive) = tokio::sync::oneshot::channel();
        dashboard.runtime.spawn(async move {
            tokio::select! {
                _ = send.closed() => {}
                result = peerhatch_control::list_peer_services(&path, peer) => { let _ = send.send(result); }
            }
        });
        // Replacing this task or closing the form also closes the outstanding IPC query.
        self.lookup = Some(cx.spawn(async move |this, cx| {
            let Ok(result) = receive.await else { return };
            let _ = this.update(cx, |v, cx| {
                v.service_list = match result {
                    Ok(names) => ServiceList::Ready(names),
                    Err(error) => ServiceList::Unavailable(format!("{error:#}")),
                };
                cx.notify();
            });
        }));
        cx.notify();
    }

    fn is_saved_service(&self, name: &str) -> bool {
        self.original_peer.as_ref() == Some(&self.peer)
            && self.original_service.as_deref() == Some(name)
    }

    fn service_field(&self, ui: Ui, cx: &Context<Self>) -> Div {
        let value = self.service.read(cx).text();
        let mut field = div().flex().flex_col().min_w_0().gap_1();
        let mut line = div().w_full().flex().items_center().gap_1();
        match &self.service_list {
            ServiceList::Loading => {
                return ui.labeled("Remote service", ui.muted("Loading services…"));
            }
            ServiceList::Ready(names) => {
                let selected = names.iter().find(|n| n.as_str() == value).cloned();
                let placeholder = if selected.is_none() && self.is_saved_service(value) {
                    format!("{value} (saved, currently unlisted)")
                } else if names.is_empty() {
                    "No services available to this device".into()
                } else {
                    "Select a published service".into()
                };
                let id = (
                    ElementId::from("remote-service"),
                    SharedString::from(self.peer.clone()),
                );
                let mut select = Select::new(
                    id,
                    selected,
                    cx.listener(|v, name: &String, _, cx| {
                        v.service.update(cx, |input, cx| input.set_text(name, cx));
                        cx.notify();
                    }),
                )
                .placeholder(placeholder);
                for name in names {
                    select = select.option(name.clone(), name.clone());
                }
                line = line.child(div().flex_1().min_w_0().child(select));
            }
            ServiceList::Unavailable(error) => {
                line = line.child(div().flex_1().min_w_0().child(self.service.clone()));
                field = field.child(
                    ui.muted("Service list unavailable. Enter a name or retry.")
                        .id("service-lookup-error")
                        .tooltip(crate::components::tooltip(error.clone().into())),
                );
            }
        }
        line = line.child(
            ui.icon_button(
                Ghost,
                "refresh-remote-services",
                Icon::Refresh,
                "Refresh services",
            )
            .on_click(cx.listener(|v, _, _, cx| v.load_services(cx))),
        );
        ui.labeled("Remote service", field.child(line))
    }

    fn save_rule(&mut self, window: &Window, cx: &mut Context<Self>) {
        if self.dashboard.read(cx).saving {
            return;
        }
        let name = self.name.read(cx).text().to_owned();
        let name = if self.original_peer.is_some() {
            name
        } else {
            name.trim().to_owned()
        };
        let Ok(listen) = peerhatch_control::listen_address(self.listen.read(cx).text().trim())
        else {
            Dialog::show_error(
                window,
                "Enter a listen address with a port, such as 127.0.0.1:8080 or [::1]:8080.",
                cx,
            );
            return;
        };
        if name.is_empty() {
            Dialog::show_error(window, "Enter a rule name.", cx);
            return;
        }
        let destination = if self.mode == RuleMode::Service {
            let value = self.service.read(cx).text();
            let name = match &self.service_list {
                ServiceList::Loading => {
                    Dialog::show_error(window, "Wait for the service list to load.", cx);
                    return;
                }
                ServiceList::Ready(names)
                    if !self.is_saved_service(value) && !names.iter().any(|n| n == value) =>
                {
                    Dialog::show_error(
                        window,
                        "Select a published service available to this device.",
                        cx,
                    );
                    return;
                }
                ServiceList::Unavailable(_) if !self.is_saved_service(value) => value.trim(),
                _ => value,
            };
            if name.is_empty() {
                Dialog::show_error(window, "Enter a remote service name.", cx);
                return;
            }
            Destination::Service(name.to_owned())
        } else {
            match peerhatch_control::tcp_target(self.target.read(cx).text().trim()) {
                Ok(target) => Destination::Target(target),
                Err(e) => {
                    Dialog::show_error(window, e, cx);
                    return;
                }
            }
        };
        let command = Command::SaveForward {
            name,
            edit: self.original_peer.is_some(),
            expected_peer: self.original_peer.clone(),
            rule: Forward {
                listen,
                peer: self.peer.clone(),
                destination,
                direction: if self.mode == RuleMode::Remote {
                    Direction::Remote
                } else {
                    Direction::Local
                },
            },
        };
        self.dashboard.update(cx, |d, cx| {
            d.send_from_dialog(command, window, DialogSuccess::Close, cx)
        });
    }

    fn render_form(&self, ui: Ui, cx: &Context<Self>) -> DialogContent {
        let dashboard = self.dashboard.read(cx);
        let name = if self.original_peer.is_some() {
            ui.labeled("Rule name", self.name.read(cx).text().to_owned())
        } else {
            ui.labeled("Rule name", self.name.clone())
        };
        let mut peers = Select::new(
            "rule-device",
            self.peer.clone(),
            cx.listener(|v, peer: &String, _, cx| {
                if !v.dashboard.read(cx).saving && v.peer != *peer {
                    v.peer = peer.clone();
                    v.service.update(cx, |field, cx| field.set_text("", cx));
                    if v.mode == RuleMode::Service {
                        v.load_services(cx);
                    }
                    cx.notify();
                }
            }),
        );
        if let Some(status) = &dashboard.status {
            for name in status.peers.keys() {
                peers = peers.option(name.clone(), name.clone());
            }
        }
        let modes = Select::new(
            "forwarding-mode",
            self.mode,
            cx.listener(|v, mode: &RuleMode, _, cx| {
                if !v.dashboard.read(cx).saving && v.mode != *mode {
                    v.mode = *mode;
                    if v.mode == RuleMode::Service {
                        v.load_services(cx);
                    } else {
                        v.lookup = None;
                    }
                    cx.notify();
                }
            }),
        )
        .placeholder("Choose a forwarding mode")
        .option(RuleMode::Local, "Listen locally")
        .option(RuleMode::Remote, "Listen on peer")
        .option(RuleMode::Service, "Named service");
        let body = div()
            .flex()
            .flex_col()
            .gap_3()
            .child(
                form_row()
                    .child(ui.labeled("Device", peers))
                    .child(ui.labeled("Forwarding mode", modes)),
            )
            .child(form_row().child(name).child(ui.labeled(
                if self.mode == RuleMode::Remote {
                    "Peer listen address"
                } else {
                    "Local listen address"
                },
                self.listen.clone(),
            )))
            .child(match self.mode {
                RuleMode::Service => self.service_field(ui, cx),
                RuleMode::Local => ui.labeled("Remote TCP target", self.target.clone()),
                RuleMode::Remote => ui.labeled("Local TCP target", self.target.clone()),
            });
        DialogContent::new(body, "Cancel").action(
            ui.button(
                Primary,
                "save-rule",
                if dashboard.saving {
                    "Saving…"
                } else {
                    "Save rule"
                },
            )
            .on_click(cx.listener(|v, _, w, cx| v.save_rule(w, cx))),
        )
    }
}

impl Dashboard {
    pub(super) fn forwarding(&self, s: &Status, ui: Ui, width: Pixels, cx: &Context<Self>) -> Div {
        let page = super::overview();
        let mut filter = Select::new(
            "forward-device-filter",
            Some(self.forward_filter.clone()),
            cx.listener(|v, peer: &Option<String>, _, cx| {
                v.forward_filter = peer.clone();
                cx.notify();
            }),
        )
        .option(None, "All devices");
        for name in s.peers.keys() {
            filter = filter.option(Some(name.clone()), name.clone());
        }
        let narrow = width < px(650.);
        let mut table = ui
            .card()
            .child(
                row()
                    .child(heading("Port forwarding"))
                    .when(!s.peers.is_empty(), |r| {
                        r.child(
                            ui.button(Primary, "add-rule", "Add rule")
                                .icon(Icon::Plus)
                                .on_click(cx.listener(|v, _, w, cx| v.open_rule(None, w, cx))),
                        )
                    }),
            )
            .child(
                div()
                    .w(px(200.))
                    .max_w_full()
                    .child(ui.labeled("Device", filter)),
            );
        let columns = || div().w_full().min_w_0().flex().gap_2().items_center();
        let cell = || div().min_w_0().flex_1().flex().flex_col();
        if !narrow {
            let mut header = columns();
            for label in ["Rule", "Device", "Listen", "Target", "State"] {
                header = header.child(cell().child(ui.muted(label)));
            }
            table = table.child(header.child(div().w(px(36.))));
        }
        let mut rows = s
            .forwards
            .iter()
            .filter(|(_, f)| self.forward_filter.as_ref().is_none_or(|p| p == &f.peer))
            .peekable();
        if rows.peek().is_none() {
            return page.child(table.child(ui.muted(if s.peers.is_empty() {
                "Pair a device to create a forwarding rule."
            } else {
                "No forwarding rules."
            })));
        }
        for (name, f) in rows {
            let actions = div().w(px(36.)).flex_none().child(self.port_actions(
                name,
                super::port_actions::PortEntry::Forward {
                    peer: f.peer.clone(),
                    enabled: f.enabled(),
                },
                cx,
            ));
            let listen = f.bound.as_ref().unwrap_or(&f.listen);
            let target = f.target.as_ref().unwrap_or(&f.service);
            let (source, destination) = if f.direction == Direction::Local {
                ("This device", f.peer.as_str())
            } else {
                (f.peer.as_str(), "This device")
            };
            let line = |column: &'static str, value: &str| {
                crate::components::single_line(div())
                    .w_full()
                    .id((ElementId::from(column), SharedString::from(name.clone())))
                    .tooltip(crate::components::tooltip(value.to_owned().into()))
                    .child(value.to_owned())
            };
            let endpoint = |column, host: &str, address: &str| {
                cell()
                    .child(ui.muted(host.to_owned()).text_xs())
                    .child(line(column, address))
            };
            let entry = if narrow {
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        columns()
                            .child(cell().child(name.clone()))
                            .child(f.state().to_owned())
                            .child(actions),
                    )
                    .child(ui.muted(f.route()))
            } else {
                columns()
                    .child(cell().child(line("rule", name)))
                    .child(cell().child(line("peer", &f.peer)))
                    .child(endpoint("listen", source, listen))
                    .child(endpoint("target", destination, target))
                    .child(cell().child(line("state", f.state())))
                    .child(actions)
            };
            table = table.child(ui.separator()).child(entry);
        }
        page.child(table)
    }
}
