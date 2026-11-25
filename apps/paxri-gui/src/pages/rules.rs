use crate::{
    Dashboard, Input,
    components::{
        ButtonVariant::Primary, Dialog, DialogContent, DialogMode, DialogSuccess, Ui, form_row,
    },
};
use gpui::{prelude::*, *};
use paxri_control::{Command, Direction, Forward, PeerForward, Status};
use paxri_ui::components::{
    Icon, ListState, ListView,
    directory::count_summary,
    forwarding::{NARROW_WIDTH, device_select, direction_select, endpoint_labels, parse_endpoints},
};

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
    pub(crate) fn refresh_forward_directory(&mut self, cx: &mut Context<Self>) {
        let Some(status) = &self.status else { return };
        if self.listing_forwards { return; }
        self.listing_forwards = true;
        let gateway = status.device_id.clone();
        let ipc = status.ipc_path.clone();
        let peers = status.peers.clone();
        let path = self.active_address().unwrap_or_else(paxri_control::default_address);
        let job = self.runtime.spawn(async move {
            paxri_control::request(&path, Command::ListForwards).await
        });
        cx.spawn(async move |this, cx| {
            let result = job.await.map_err(anyhow::Error::from).and_then(|s| s);
            let _ = this.update(cx, |v, cx| {
                v.listing_forwards = false;
                let Some(current) = &mut v.status else { return };
                if current.device_id != gateway || current.ipc_path != ipc || current.peers != peers { return; }
                match result {
                    Ok(snapshot) if snapshot.device_id == gateway && snapshot.peers == peers => {
                        current.peer_forwards = snapshot.peer_forwards;
                    }
                    Err(e) => {
                        let mut directory = paxri_control::ForwardDirectory::default();
                        directory.errors.insert("Gateway".into(), format!("{e:#}"));
                        current.peer_forwards = Some(directory);
                    }
                    _ => return,
                }
                cx.notify();
            });
        }).detach();
    }

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
            .or(self.forward_filters.scope.device.as_ref())
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
                .or(self.forward_filters.scope.direction)
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
        let (listen, target) = match parse_endpoints(
            self.listen.read(cx).text(),
            self.target.read(cx).text(),
        ) {
            Ok(endpoints) => endpoints,
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
            let name = dashboard
                .status
                .as_ref()
                .map_or(owner.as_str(), |s| s.peer_display_name(owner));
            ui.labeled("Device", name.to_owned())
        } else {
            let options: Vec<_> = dashboard
                .status
                .as_ref()
                .map(|status| {
                    status
                        .peers
                        .keys()
                        .map(|name| {
                            (
                                name.clone(),
                                status.peer_display_name(name).to_owned(),
                                status.peer_platform(name),
                            )
                        })
                        .collect()
                })
                .unwrap_or_default();
            let peers = device_select(
                "rule-device",
                self.peer.clone(),
                "Choose a paired device",
                &options,
                cx.listener(|v, peer: &String, _, cx| {
                    v.peer = peer.clone();
                    cx.notify();
                }),
            );
            ui.labeled("Device", peers)
        };
        let modes = direction_select(
            "forwarding-mode",
            self.direction,
            cx.listener(|v, mode: &Direction, _, cx| {
                v.direction = *mode;
                cx.notify();
            }),
        );
        let (listen_label, target_label) = endpoint_labels(self.direction);
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
                    .child(ui.labeled(listen_label, self.listen.clone()))
                    .child(ui.labeled(target_label, self.target.clone())),
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
        let active = filters.has_selections() || !query.is_empty();
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
            .filter(|(_, name, f)| {
                filters.matches(&f.peer, name, f, s.peer_display_name(&f.peer), &query)
            })
            .collect();
        let narrow = width < NARROW_WIDTH;
        let devices: Vec<_> = s
            .peers
            .keys()
            .map(|k| {
                (
                    k.clone(),
                    s.peer_display_name(k).to_owned(),
                    s.peer_platform(k),
                )
            })
            .collect();
        let subtitle = if s.peer_forwards.is_none() && total == 0 && !s.peers.is_empty() {
            "Loading…".to_owned()
        } else {
            count_summary(rows.len(), total, active, "rule", "rules")
        };
        let page = page
            .py_2()
            .child(ui.page_heading(Icon::Forward, "Port forwarding", subtitle))
            .child(ui.separator());
        let mut table = ui
            .section()
            .child(filters.toolbar(&devices, |v| &mut v.forward_filters, cx));
        let mut failures = Vec::new();
        if let Some(directory) = &s.peer_forwards {
            for (peer, error) in &directory.errors {
                if s.peers.contains_key(peer) && !s.peer_supports_forward_rules(peer) {
                    continue;
                }
                if filters.scope.device.as_ref().is_none_or(|p| p == peer) || peer == "Gateway" {
                    failures.push(s.peer_display_name(peer));
                    if !rows.is_empty() {
                        table = table.child(
                            ui.muted(format!(
                                "{}: forwarding rules unavailable",
                                s.peer_display_name(peer)
                            ))
                            .id(SharedString::from(format!("forward-error-{peer}")))
                            .tooltip(crate::components::tooltip(error.clone().into())),
                        );
                    }
                }
            }
        }
        if rows.is_empty() {
            let state = if s.peers.is_empty() {
                ListState::empty(Icon::Forward, "No forwarding rules")
                    .detail("Pair a device to create a forwarding rule.")
            } else if s.peer_forwards.is_none() {
                ListState::loading("Loading forwarding rules…")
            } else if !failures.is_empty() {
                ListState::empty(Icon::Forward, "Forwarding rules unavailable")
                    .detail(failures.join(", "))
            } else if active && total > 0 {
                ListState::filtered("No matching rules")
            } else {
                ListState::empty(Icon::Forward, "No forwarding rules")
            };
            return page.child(table.child(state.render(ui)));
        }
        let rows: Vec<_> = rows
            .into_iter()
            .map(|(owner, name, rule)| {
                let owner = owner.map(String::as_str);
                let display_name = s.peer_display_name(&rule.peer);
                let actions = self.port_actions(name, owner, rule, display_name, cx);
                ui.forwarding_row(
                    format!("{owner:?}/{name}"),
                    name,
                    rule,
                    display_name,
                    narrow,
                    actions,
                )
            })
            .collect();
        page.child(table.child(ui.forwarding_table(ListView::new(), narrow, rows)))
    }
}
