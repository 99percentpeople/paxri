use super::*;
use paxri_control::{Direction, PeerForward, forwarding::Rules};
use paxri_ui::components::{
    ButtonVariant::Ghost,
    DropdownState, Icon, ListState, ListView, Select,
    forwarding::{RuleAction, RuleMenu},
    input::Input,
};
use paxri_ui::settings::{SettingsGroup, SettingsSection, SettingsTable};

#[derive(Default)]
struct Entry {
    request: Option<(u64, tokio::task::JoinHandle<()>)>,
    rules: Option<Rules>,
    error: Option<String>,
}

impl Drop for Entry {
    fn drop(&mut self) {
        if let Some((_, task)) = &self.request {
            task.abort();
        }
    }
}

#[derive(Default)]
pub(super) struct ForwardList {
    entries: BTreeMap<String, Entry>,
    next_request: u64,
    pub(super) menus: Vec<Entity<DropdownState>>,
}

impl ForwardList {
    pub(super) fn invalidate(&mut self, owner: &str) {
        if let Some(entry) = self.entries.get_mut(owner) {
            if let Some((_, task)) = entry.request.take() {
                task.abort();
            }
        }
    }

    pub(super) fn complete(
        &mut self,
        peer: &str,
        request: u64,
        result: Result<Rules, String>,
    ) -> bool {
        let Some(entry) = self.entries.get_mut(peer) else {
            return false;
        };
        if !matches!(&entry.request, Some((pending, _)) if *pending == request) {
            return false;
        }
        entry.request.take();
        match result {
            Ok(rules) => {
                entry.rules = Some(rules);
                entry.error = None;
            }
            Err(error) => entry.error = Some(error),
        }
        true
    }
}

impl Mobile {
    fn rule_devices(&self) -> impl Iterator<Item = (&String, &client::Peer)> {
        self.peers.iter().filter(|(id, peer)| {
            self.client.is_some()
                && self.presence.online(id)
                && peer.supports_creating_forward_rules()
        })
    }

    pub(super) fn can_add_rule(&self, scope: Option<&str>) -> bool {
        self.rule_devices()
            .any(|(id, _)| scope.is_none_or(|scope| scope == id))
    }

    pub(super) fn sync_forwards(&mut self, refresh: bool) {
        let Page::Forwards(_) = &self.page else {
            self.forwards.entries.clear();
            self.forwards.menus.clear();
            return;
        };
        let eligible = |id: &String, peer: &client::Peer| {
            peer.supports_forward_rules() && self.presence.online(id)
        };
        self.forwards
            .entries
            .retain(|id, _| self.peers.get(id).is_some_and(|p| eligible(id, p)));
        let Some(client) = &self.client else { return };
        for (id, peer) in &self.peers {
            if !eligible(id, peer) {
                continue;
            }
            let entry = self.forwards.entries.entry(id.clone()).or_default();
            if entry.request.is_some()
                || (!refresh && (entry.rules.is_some() || entry.error.is_some()))
            {
                continue;
            }
            self.forwards.next_request += 1;
            let request = self.forwards.next_request;
            let owner = id.clone();
            let name = peer.name.clone();
            let client = client.clone();
            let updates = self.updates.clone();
            let task = self.rt.spawn(async move {
                let result = client
                    .manage_forward(&owner, &name, PeerForward::List)
                    .await
                    .map_err(|e| format!("{e:#}"));
                let _ = updates
                    .send(Update::ForwardRules(owner, request, result))
                    .await;
            });
            entry.request = Some((request, task));
        }
    }

    pub(super) fn forward_list(
        &mut self,
        ui: Ui,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let filters = &self.forward_filters;
        let query = filters.query(cx);
        let terms: Vec<_> = query.split_whitespace().collect();
        let mut options: Vec<_> = self
            .peers
            .iter()
            .map(|(id, peer)| (id.clone(), peer.name.clone(), peer.platform))
            .collect();
        options.sort_by(|a, b| a.1.cmp(&b.1).then(a.0.cmp(&b.0)));
        let mut body = ListView::scrollable("forward-list")
            .with_floating_action(self.can_add_rule(filters.device.as_deref()));
        let narrow = window.viewport_size().width < px(650.);
        self.forwards.menus.clear();
        let mut count = 0;
        let mut pending = false;
        let mut failures = Vec::new();
        let mut rows = Vec::new();
        for (id, name, _) in &options {
            if filters
                .device
                .as_ref()
                .is_some_and(|selected| selected != id)
            {
                continue;
            }
            if let Some(entry) = self.forwards.entries.get(id) {
                pending |= entry.request.is_some() && entry.rules.is_none();
                if let Some(error) = &entry.error {
                    failures.push(format!("{name}: {error}"));
                }
                if let Some(rules) = &entry.rules {
                    count += rules.len();
                    for (key, rule) in rules {
                        if !filters.matches(id, key, rule, name, &terms) {
                            continue;
                        }
                        let owner = id.clone();
                        let rule_key = key.clone();
                        let route = rule.route_for(name);
                        let view = cx.weak_entity();
                        let menu_id = format!("rule-actions-{id}/{key}");
                        let state = DropdownState::local(
                            SharedString::from(menu_id.clone()).into(),
                            window,
                            cx,
                        );
                        self.forwards.menus.push(state.clone());
                        let menu =
                            RuleMenu::new(menu_id, key, rule.enabled(), move |action, _, cx| {
                                let _ = view.update(cx, |v, cx| match action {
                                    RuleAction::Edit => v.open_rule(Some((&owner, &rule_key)), cx),
                                    RuleAction::SetEnabled(enabled) => v.send_forward(
                                        &owner,
                                        PeerForward::SetEnabled {
                                            name: rule_key.clone(),
                                            enabled,
                                        },
                                        cx,
                                    ),
                                    RuleAction::Remove => {
                                        if !v.rule_form.saving
                                            && let Err(error) = activity_action(
                                                &v.android,
                                                ActivityAction::ConfirmRemoveForward {
                                                    owner: &owner,
                                                    name: &rule_key,
                                                    route: &route,
                                                },
                                            )
                                        {
                                            Toast::show(error, v.window, cx);
                                        }
                                    }
                                });
                            })
                            .with_state(state);
                        rows.push(ui.forwarding_row(
                            format!("{id}/{key}"),
                            key,
                            rule,
                            name,
                            narrow,
                            menu,
                        ));
                    }
                }
            }
        }
        if rows.is_empty() {
            let selected = filters
                .device
                .as_ref()
                .and_then(|id| self.peers.get(id).map(|p| (id, p)));
            let state = if self.starting_network || pending {
                ListState::loading("Loading forwarding rules…")
            } else if self.client.is_none() {
                ListState::empty(Icon::Server, "Network unavailable")
            } else if !failures.is_empty() {
                ListState::empty(Icon::Forward, "Forwarding rules unavailable")
                    .detail(failures.join("\n"))
            } else if self.peers.is_empty() {
                ListState::empty(Icon::Forward, "No forwarding rules")
                    .detail("Pair a device to create a forwarding rule.")
            } else if let Some((_, p)) = selected
                && !p.supports_forward_rules()
            {
                ListState::empty(Icon::Forward, "Forwarding unavailable")
                    .detail("This device does not share forwarding rules.")
            } else if let Some((id, _)) = selected
                && !self.presence.online(id)
            {
                ListState::empty(Icon::Monitor, "Device offline")
            } else if self.forwards.entries.is_empty() {
                ListState::empty(Icon::Forward, "No devices available")
                    .detail("Connect a device that shares forwarding rules.")
            } else if count > 0 {
                ListState::filtered("No matching rules")
            } else if !self.can_add_rule(filters.device.as_deref()) {
                ListState::empty(Icon::Forward, "No forwarding rules")
                    .detail("Update the gateway to create forwarding rules.")
            } else {
                ListState::empty(Icon::Forward, "No forwarding rules")
            };
            body = body.child(state.render(ui));
        } else {
            for error in failures {
                body = body.child(ui.muted(error));
            }
            body = body.divided();
            if !narrow {
                body = body.child(ui.forwarding_header());
            }
            body = body.children(rows);
        }
        div()
            .min_h_0()
            .flex_1()
            .flex()
            .flex_col()
            .px_3()
            .pt_2()
            .gap_2()
            .child(
                filters
                    .toolbar(&options, |v| &mut v.forward_filters, cx)
                    .with_state(&self.filter_menu),
            )
            .child(body)
    }
}

pub(super) struct RuleForm {
    pub inputs: [Entity<Input>; 3],
    pub device_menu: Entity<DropdownState>,
    pub direction_menu: Entity<DropdownState>,
    pub existing: Option<String>,
    direction: Direction,
    pub saving: bool,
}

impl RuleForm {
    pub(super) fn new(window: &mut Window, cx: &mut Context<Mobile>) -> Self {
        Self {
            inputs: ["e.g. web", "8080 or IP:port", "3000 or host:port"]
                .map(|placeholder| Input::setting(placeholder, cx)),
            device_menu: DropdownState::new(window, cx),
            direction_menu: DropdownState::new(window, cx),
            existing: None,
            direction: Direction::Local,
            saving: false,
        }
    }

    fn save_action(&self, cx: &App) -> Result<PeerForward, String> {
        let [name, listen, target] = &self.inputs;
        let listen = paxri_control::listen_address(listen.read(cx).text().trim())?;
        let target = paxri_control::tcp_target(target.read(cx).text().trim())?;
        let direction = self.direction;
        if let Some(name) = &self.existing {
            Ok(PeerForward::Edit {
                name: name.clone(),
                listen,
                target,
                direction,
            })
        } else {
            let name = name.read(cx).text().trim();
            let name = if name.is_empty() {
                format!("tcp-{}", listen.port())
            } else {
                name.to_owned()
            };
            Ok(PeerForward::Create {
                name,
                listen,
                target,
                direction,
            })
        }
    }
}

impl Mobile {
    pub(super) fn open_rule(&mut self, existing: Option<(&str, &str)>, cx: &mut Context<Self>) {
        let Page::Forwards(scope) = &self.page else {
            return;
        };
        if self.rule_form.saving {
            return;
        }
        let (owner, name, rule) = match existing {
            Some((owner, name)) => {
                let Some(rules) = self
                    .forwards
                    .entries
                    .get(owner)
                    .and_then(|entry| entry.rules.as_ref())
                else {
                    return;
                };
                let Some(rule) = rules.get(name) else { return };
                (Some(owner.to_owned()), Some(name), Some(rule))
            }
            None => {
                if !self.can_add_rule(self.forward_filters.device.as_deref()) {
                    return;
                }
                let owner = self.forward_filters.device.clone().or_else(|| {
                    let mut devices = self.rule_devices();
                    let (id, _) = devices.next()?;
                    devices.next().is_none().then(|| id.clone())
                });
                (owner, None, None)
            }
        };
        self.rule_form.existing = name.map(str::to_owned);
        self.rule_form.direction = rule
            .map_or(self.forward_filters.direction.unwrap_or_default(), |r| {
                r.direction
            });
        let values = [
            name.unwrap_or_default(),
            rule.map_or("", |r| &r.listen),
            rule.map_or("", |r| &r.target),
        ];
        for (input, value) in self.rule_form.inputs.iter().zip(values) {
            input.update(cx, |input, cx| input.set_text(value, cx));
        }
        self.navigate(
            Page::ForwardEdit {
                owner,
                scope: scope.clone(),
            },
            cx,
        );
        cx.notify();
    }

    fn change_forward(&mut self, action: PeerForward, cx: &mut Context<Self>) {
        let Page::ForwardEdit {
            owner: Some(owner), ..
        } = &self.page
        else {
            return;
        };
        let owner = owner.clone();
        self.send_forward(&owner, action, cx);
    }

    pub(super) fn remove_forward(&mut self, owner: &str, name: String, cx: &mut Context<Self>) {
        if !matches!(self.page, Page::Forwards(_)) {
            return;
        }
        if self.presence.online(owner)
            && !self.forwards.entries.get(owner)
                .and_then(|entry| entry.rules.as_ref())
                .is_some_and(|rules| rules.contains_key(&name))
        {
            Toast::show("Forwarding rule unavailable", self.window, cx);
            return;
        }
        self.send_forward(owner, PeerForward::Remove { name }, cx);
    }

    fn send_forward(&mut self, owner: &str, action: PeerForward, cx: &mut Context<Self>) {
        if self.rule_form.saving {
            return;
        }
        let Some((client, peer)) = self.client.as_ref().zip(self.peers.get(owner))
            .filter(|_| self.presence.online(owner))
        else {
            Toast::show("Device offline", self.window, cx);
            return;
        };
        let name = peer.name.clone();
        let owner = owner.to_owned();
        let client = client.clone();
        self.rule_form.saving = true;
        let updates = self.updates.clone();
        self.rt.spawn(async move {
            let result = client
                .manage_forward(&owner, &name, action)
                .await
                .map(|_| ())
                .map_err(|e| format!("{e:#}"));
            let _ = updates.send(Update::ForwardChanged(owner, result)).await;
        });
        cx.notify();
    }

    fn can_save_rule(&self, owner: Option<&str>) -> bool {
        owner.is_some_and(|id| {
            if self.rule_form.existing.is_some() {
                self.client.is_some() && self.presence.online(id)
            } else {
                self.can_add_rule(Some(id))
            }
        })
    }

    pub(super) fn save_rule_button(
        &self,
        owner: Option<&str>,
        ui: Ui,
        cx: &Context<Self>,
    ) -> Option<AnyElement> {
        if !self.can_save_rule(owner) || self.rule_form.saving {
            return None;
        }
        Some(
            ui.button(Ghost, "save-forward", Icon::Check.draw(22.))
                .circular()
                .tooltip(paxri_ui::components::tooltip("Save rule".into()))
                .role(Role::Button)
                .aria_label("Save rule")
                .on_click(
                    cx.listener(|this, _, _, cx| match this.rule_form.save_action(cx) {
                        Ok(action) => this.change_forward(action, cx),
                        Err(error) => Toast::show(error, this.window, cx),
                    }),
                )
                .into_any_element(),
        )
    }

    pub(super) fn rule_editor(
        &self,
        owner: Option<&str>,
        ui: Ui,
        cx: &Context<Self>,
    ) -> Stateful<Div> {
        let form = &self.rule_form;
        let [name, listen, target] = &form.inputs;
        let mut fields = SettingsGroup::new(SettingsSection::new(
            SettingsTable::Root,
            "Port forwarding",
            Icon::Forward,
        ));
        if form.existing.is_some() || form.saving {
            fields.field(
                "Device",
                owner
                    .and_then(|id| self.peers.get(id))
                    .map(|p| p.name.clone())
                    .unwrap_or_default(),
            );
        } else {
            let mut devices = Select::new(
                "rule-device",
                owner.map(str::to_owned),
                cx.listener(|this, id: &String, _, cx| {
                    if let Page::ForwardEdit { owner, .. } = &mut this.page {
                        *owner = Some(id.clone());
                        cx.notify();
                    }
                }),
            )
            .with_optional_state(Some(&form.device_menu))
            .placeholder(match owner.and_then(|id| self.peers.get(id)) {
                Some(peer) => format!("{} (unavailable)", peer.name),
                None => "Choose a paired device".to_owned(),
            });
            for (id, peer) in self.rule_devices() {
                devices = devices.option_with_icon(
                    id.clone(),
                    peer.name.clone(),
                    Icon::device(peer.platform),
                );
            }
            fields.field("Device", devices);
        }
        if let Some(key) = &form.existing {
            fields.field("Rule name", key.clone());
        } else {
            fields.field("Rule name", name.clone());
        }
        let direction = Select::new(
            "forward-direction",
            form.direction,
            cx.listener(|this, direction: &Direction, _, cx| {
                this.rule_form.direction = *direction;
                cx.notify();
            }),
        )
        .with_optional_state(Some(&form.direction_menu))
        .option(Direction::Local, "Forward")
        .option(Direction::Remote, "Reverse");
        fields.field("Direction", direction);
        fields.field(
            if form.direction == Direction::Local {
                "Listen on this device"
            } else {
                "Listen on peer"
            },
            listen.clone(),
        );
        fields.field(
            if form.direction == Direction::Local {
                "Target on peer"
            } else {
                "Target on this device"
            },
            target.clone(),
        );
        let page = self
            .settings_body("forward-edit-page")
            .child(fields.render_rows(ui));
        if form.saving {
            return page.child(ui.muted("Saving…"));
        }
        if owner.is_some_and(|id| !self.presence.online(id)) {
            return page.child(ui.muted("Device offline."));
        }
        if form.existing.is_none() && !self.can_add_rule(None) {
            return page.child(ui.muted("No connected devices can create forwarding rules."));
        }
        page
    }
}
