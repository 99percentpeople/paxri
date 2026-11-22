use crate::{
    Dashboard, Input,
    components::{
        ButtonVariant::{Ghost, Primary},
        Dialog, DialogContent, DialogMode, DialogSuccess, Icon, Select, Ui, form_row, heading, row,
    },
};
use gpui::{prelude::*, *};
use peerhatch_control::{Command, Direction, Forward, PeerForward, Status};

struct RuleForm {
    dashboard: Entity<Dashboard>,
    owner: Option<String>,
    name: Option<String>,
    original_peer: Option<String>,
    peer: String,
    listen: Entity<Input>,
    target: Entity<Input>,
    direction: Direction,
}

impl Dashboard {
    pub(super) fn open_rule(
        &mut self,
        owner: Option<&str>,
        name: Option<&str>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.saving {
            return;
        }
        let Some(status) = &self.status else { return };
        let rule = name.and_then(|id| match owner {
            Some(peer) => status.peer_forwards.as_ref()?.rules.get(peer)?.get(id),
            None => status.forwards.get(id),
        });
        if name.is_some() && rule.is_none() {
            return;
        }
        let Some(peer) = rule
            .map(|r| &r.peer)
            .or(self.forward_filters.device.as_ref())
            .or_else(|| status.peers.keys().next())
        else {
            return;
        };
        let form = RuleForm {
            dashboard: cx.entity(),
            owner: owner.map(str::to_owned),
            name: name.map(str::to_owned),
            original_peer: rule.map(|r| r.peer.clone()),
            peer: peer.clone(),
            listen: Input::new(rule.map_or("", |r| &r.listen), "8080", cx),
            target: Input::new(rule.map_or("", |r| &r.target), "3000 or host:port", cx),
            direction: rule
                .map(|r| r.direction)
                .or(self.forward_filters.direction)
                .unwrap_or_default(),
        };
        let focus = form.listen.read(cx).focus_handle(cx);
        let model = cx.new(|cx| {
            cx.observe(&form.dashboard, |_, _, cx| cx.notify()).detach();
            form
        });
        Dialog::open(
            window,
            cx,
            DialogMode::Modal,
            if name.is_some() {
                "Edit forwarding rule"
            } else {
                "New forwarding rule"
            },
            size(px(520.), px(290.)),
            model,
            Some(focus),
            RuleForm::render_form,
        );
    }
}

impl RuleForm {
    fn save(&self, window: &Window, cx: &mut Context<Self>) {
        if self.dashboard.read(cx).saving {
            return;
        }
        let listen = match peerhatch_control::listen_address(self.listen.read(cx).text().trim()) {
            Ok(addr) => addr,
            Err(error) => return Dialog::show_error(window, error, cx),
        };
        let target = match peerhatch_control::tcp_target(self.target.read(cx).text().trim()) {
            Ok(addr) => addr,
            Err(error) => return Dialog::show_error(window, error, cx),
        };
        let name = self.name.clone().unwrap_or_default();
        let command = match &self.owner {
            Some(owner) => Command::ManagePeerForward {
                owner: owner.clone(),
                action: PeerForward::Edit {
                    name,
                    listen,
                    target,
                    direction: self.direction,
                },
            },
            None => Command::SaveForward {
                name,
                edit: self.name.is_some(),
                expected_peer: self.original_peer.clone(),
                rule: Forward {
                    peer: self.peer.clone(),
                    listen,
                    target,
                    direction: self.direction,
                },
            },
        };
        self.dashboard.update(cx, |d, cx| {
            d.send_from_dialog(command, window, DialogSuccess::Close, cx)
        });
    }

    fn render_form(&self, ui: Ui, cx: &Context<Self>) -> DialogContent {
        let dashboard = self.dashboard.read(cx);
        let device = if let Some(owner) = &self.owner {
            let name = dashboard.status.as_ref()
                .map_or(owner.as_str(), |s| s.peer_display_name(owner));
            ui.labeled("Device", name.to_owned())
        } else {
            let mut peers = Select::new(
                "rule-device",
                self.peer.clone(),
                cx.listener(|v, peer: &String, _, cx| {
                    v.peer = peer.clone();
                    cx.notify();
                }),
            )
            .placeholder("Choose a paired device");
            if let Some(status) = &dashboard.status {
                for name in status.peers.keys() {
                    peers = peers.option(name.clone(), status.peer_display_name(name).to_owned());
                }
            }
            ui.labeled("Device", peers)
        };
        let modes = Select::new(
            "forwarding-mode",
            self.direction,
            cx.listener(|v, mode: &Direction, _, cx| {
                v.direction = *mode;
                cx.notify();
            }),
        )
        .option(Direction::Local, "Forward")
        .option(Direction::Remote, "Reverse");
        let local = self.direction == Direction::Local;
        let body = div()
            .flex()
            .flex_col()
            .gap_3()
            .child(
                form_row()
                    .child(device)
                    .child(ui.labeled("Direction", modes)),
            )
            .child(
                form_row()
                    .child(ui.labeled(
                        if local {
                            "Listen on this device"
                        } else {
                            "Listen on peer"
                        },
                        self.listen.clone(),
                    ))
                    .child(ui.labeled(
                        if local {
                            "Target on peer"
                        } else {
                            "Target on this device"
                        },
                        self.target.clone(),
                    )),
            );
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
            .on_click(cx.listener(|v, _, w, cx| v.save(w, cx))),
        )
    }
}

impl Dashboard {
    pub(super) fn forwarding(&self, s: &Status, ui: Ui, width: Pixels, cx: &Context<Self>) -> Div {
        let page = super::overview();
        let filters = &self.forward_filters;
        let query = filters.query(cx);
        let terms: Vec<_> = query.split_whitespace().collect();
        let active = filters.has_selections() || !terms.is_empty();
        let own = s.forwards.iter().map(|(name, f)| (None, name, f));
        let shared = s
            .peer_forwards
            .iter()
            .flat_map(|d| &d.rules)
            .flat_map(|(peer, rules)| rules.iter().map(move |(name, f)| (Some(peer), name, f)));
        let total = s.forwards.len()
            + s.peer_forwards
                .iter()
                .flat_map(|d| d.rules.values())
                .map(|rules| rules.len())
                .sum::<usize>();
        let rows: Vec<_> = own
            .chain(shared)
            .filter(|(_, name, f)| filters.matches(name, f, s.peer_display_name(&f.peer), &terms))
            .collect();
        let narrow = width < px(650.);
        let mut table = ui
            .section()
            .child(
                row()
                    .child(
                        div()
                            .min_w_0()
                            .flex()
                            .flex_wrap()
                            .items_center()
                            .gap_2()
                            .child(heading("Port forwarding"))
                            .when(active && total > 0, |r| {
                                r.child(ui.muted(format!("{} of {total}", rows.len())).text_xs())
                            })
                            .when(filters.has_selections(), |r| {
                                r.child(
                                    ui.icon_button(
                                        Ghost,
                                        "clear-forward-filters",
                                        Icon::FilterX,
                                        "Clear filters",
                                    )
                                    .on_click(cx.listener(
                                        |v, _, _, cx| {
                                            v.forward_filters.reset(None, cx);
                                            cx.notify();
                                        },
                                    )),
                                )
                            }),
                    )
                    .when(!s.peers.is_empty(), |r| {
                        r.child(
                            ui.button(Primary, "add-rule", "Add rule")
                                .icon(Icon::Plus)
                                .on_click(
                                    cx.listener(|v, _, w, cx| v.open_rule(None, None, w, cx)),
                                ),
                        )
                    }),
            )
            .child(filters.toolbar(s, width, cx));
        let columns = || div().w_full().min_w_0().flex().gap_2().items_center();
        let cell = || div().min_w_0().flex_1().flex().flex_col();
        if let Some(directory) = &s.peer_forwards {
            for (peer, error) in &directory.errors {
                if s.peers.contains_key(peer) && !s.peer_supports_forward_rules(peer) {
                    continue;
                }
                if filters.device.as_ref().is_none_or(|p| p == peer) || peer == "Gateway" {
                    table = table.child(
                        ui.muted(format!("{}: forwarding rules unavailable", s.peer_display_name(peer)))
                            .id(SharedString::from(format!("forward-error-{peer}")))
                            .tooltip(crate::components::tooltip(error.clone().into())),
                    );
                }
            }
        }
        if rows.is_empty() {
            return page.child(table.child(ui.muted(if s.peers.is_empty() {
                "Pair a device to create a forwarding rule."
            } else if s.peer_forwards.is_none() {
                "Loading forwarding rules…"
            } else if active && total > 0 {
                "No rules match your filters."
            } else {
                "No forwarding rules to display."
            })));
        }
        if !narrow {
            let mut header = columns();
            for label in ["Direction", "Device", "Listen", "Target", "State"] {
                header = header.child(cell().child(ui.muted(label)));
            }
            table = table.child(header.child(div().w(px(36.))));
        }
        for (owner, name, f) in rows {
            let owner = owner.map(String::as_str);
            let display_name = s.peer_display_name(&f.peer);
            let key = format!("{owner:?}/{name}");
            let actions = div()
                .w(px(36.))
                .flex_none()
                .child(self.port_actions(name, owner, f, display_name, cx));
            let local = f.direction == Direction::Local;
            let direction = if local { "Forward" } else { "Reverse" };
            let (source, destination) = if local {
                ("This device", display_name)
            } else {
                (display_name, "This device")
            };
            let line = |column: &'static str, value: &str| {
                crate::components::single_line(div())
                    .w_full()
                    .id((ElementId::from(column), SharedString::from(key.clone())))
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
                            .child(cell().child(format!("{direction} · {display_name}")))
                            .child(f.state().to_owned())
                            .child(actions),
                    )
                    .child(ui.muted(f.route_for(display_name)))
            } else {
                columns()
                    .child(cell().child(direction))
                    .child(cell().child(line("peer", display_name)))
                    .child(endpoint(
                        "listen",
                        source,
                        f.bound.as_ref().unwrap_or(&f.listen),
                    ))
                    .child(endpoint("target", destination, &f.target))
                    .child(cell().child(line("state", f.state())))
                    .child(actions)
            };
            table = table.child(ui.separator()).child(entry);
        }
        page.child(table)
    }
}
