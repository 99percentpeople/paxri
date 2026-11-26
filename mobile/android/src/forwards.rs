use super::*;
use paxri_control::{Direction, PeerForward, forwarding::Rules};
use paxri_ui::components::{
    ButtonVariant::Ghost,
    Icon, ListState, ListView,
    forwarding::{
        NARROW_WIDTH, RuleAction, RuleMenu, device_select, direction_select, direction_summary,
        endpoint_labels, parse_endpoints,
    },
    input::Input,
};
use paxri_ui::settings::{SettingsGroup, SettingsSection, SettingsTable};

#[derive(Default)]
struct Entry {
    request: Option<(u64, tokio::task::JoinHandle<()>)>,
    checked: Option<std::time::Instant>,
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
}

impl ForwardList {
    pub(super) fn count(&self, id: &str) -> Option<usize> {
        let entry = self.entries.get(id)?;
        if entry.error.is_some() { return None; }
        entry.rules.as_ref().map(Rules::len)
    }

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
        entry.checked = Some(std::time::Instant::now());
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
        if !matches!(self.page, Page::Devices | Page::Peer(_) | Page::Forwards(_)) {
            self.forwards.entries.clear();
            return;
        }
        let eligible = |id: &String, peer: &client::Peer| {
            peer.supports_forward_rules() && self.presence.online(id)
        };
        self.forwards
            .entries
            .retain(|id, _| self.peers.get(id).is_some_and(|p| eligible(id, p)));
        let Some(client) = &self.client else { return };
        for (id, peer) in &self.peers {
            if !eligible(id, peer) || matches!(&self.page, Page::Peer(selected) if selected != id) {
                continue;
            }
            let entry = self.forwards.entries.entry(id.clone()).or_default();
            if entry.request.is_some()
                || (!refresh && entry.checked.is_some_and(|at| at.elapsed() < std::time::Duration::from_secs(8)))
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

    pub(super) fn forwards_subtitle(&self, cx: &App) -> Option<String> {
        let filters = &self.forward_filters;
        let query = filters.query(cx);
        let mut total = 0;
        let mut shown = 0;
        let mut pending = false;
        for (id, peer) in &self.peers {
            let Some(entry) = self.forwards.entries.get(id) else {
                continue;
            };
            if let Some(rules) = &entry.rules {
                total += rules.len();
            }
            if filters
                .scope
                .device
                .as_ref()
                .is_some_and(|selected| selected != id)
            {
                continue;
            }
            pending |= entry.request.is_some() && entry.rules.is_none();
            if let Some(rules) = &entry.rules {
                for (key, rule) in rules {
                    if filters.matches(id, key, rule, &peer.name, &query) {
                        shown += 1;
                    }
                }
            }
        }
        if (self.starting_network || pending) && total == 0 {
            return Some("Loading…".into());
        }
        let filtered = filters.has_selections() || !query.is_empty();
        Some(paxri_ui::components::directory::count_summary(
            shown, total, filtered, "rule", "rules",
        ))
    }

    pub(super) fn forward_list(&self, ui: Ui, window: &Window, cx: &Context<Self>) -> Div {
        let filters = &self.forward_filters;
        let query = filters.query(cx);
        let mut options: Vec<_> = self
            .peers
            .iter()
            .map(|(id, peer)| (id.clone(), peer.name.clone(), peer.platform))
            .collect();
        options.sort_by(|a, b| a.1.cmp(&b.1).then(a.0.cmp(&b.0)));
        let mut body = ListView::scrollable("forward-list")
            .with_floating_action(self.can_add_rule(filters.scope.device.as_deref()));
        let narrow = window.viewport_size().width < NARROW_WIDTH;
        let mut total = 0;
        let mut pending = false;
        let mut failures = Vec::new();
        let mut rows = Vec::new();
        for (id, name, _) in &options {
            let Some(entry) = self.forwards.entries.get(id) else {
                continue;
            };
            if let Some(rules) = &entry.rules {
                total += rules.len();
            }
            if filters
                .scope
                .device
                .as_ref()
                .is_some_and(|selected| selected != id)
            {
                continue;
            }
            pending |= entry.request.is_some() && entry.rules.is_none();
            if let Some(error) = &entry.error {
                failures.push(format!("{name}: {error}"));
            }
            if let Some(rules) = &entry.rules {
                for (key, rule) in rules {
                    if !filters.matches(id, key, rule, name, &query) {
                        continue;
                    }
                    let owner = id.clone();
                    let rule_key = key.clone();
                    let title = rule.display_name(key);
                    let route = rule.route_for(name);
                    let view = cx.weak_entity();
                    let menu_id = format!("rule-actions-{id}/{key}");
                    let menu =
                        RuleMenu::new(menu_id, &title, rule.enabled(), move |action, _, cx| {
                            let _ = view.update(cx, |v, cx| match action {
                                RuleAction::Edit => v.open_rule(Some((&owner, &rule_key)), cx),
                                RuleAction::SetEnabled(enabled) => v.send_forward(
                                    &owner,
                                    PeerForward::SetEnabled {
                                        id: rule_key.clone(),
                                        enabled,
                                    },
                                    cx,
                                ),
                                RuleAction::Remove => {
                                    if !v.rule_form.saving {
                                        v.confirm_remove_forward(
                                            owner.clone(),
                                            rule_key.clone(),
                                            route.clone(),
                                            cx,
                                        );
                                    }
                                }
                            });
                        });
                    rows.push(ui.forwarding_row(
                        format!("{id}/{key}"),
                        &title,
                        rule,
                        name,
                        narrow,
                        menu,
                    ));
                }
            }
        }
        if rows.is_empty() {
            let selected = filters
                .scope
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
            } else if total > 0 {
                ListState::filtered("No matching rules")
            } else if !self.can_add_rule(filters.scope.device.as_deref()) {
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
            body = ui.forwarding_table(body, narrow, rows);
        }
        div()
            .min_h_0()
            .flex_1()
            .flex()
            .flex_col()
            .px_3()
            .pt_2()
            .gap_2()
            .child(filters.toolbar(&options, |v| &mut v.forward_filters, cx))
            .child(body)
    }
}

pub(super) struct RuleForm {
    pub inputs: [Entity<Input>; 3],
    pub existing: Option<String>,
    direction: Direction,
    pub saving: bool,
}

impl RuleForm {
    pub(super) fn new(cx: &mut Context<Mobile>) -> Self {
        Self {
            inputs: ["e.g. web (optional)", "8080 or IP:port", "3000 or host:port"]
                .map(|placeholder| Input::setting(placeholder, cx)),
            existing: None,
            direction: Direction::Local,
            saving: false,
        }
    }

    fn save_action(&self, cx: &App) -> Result<PeerForward, String> {
        let [name, listen, target] = &self.inputs;
        let (listen, target) = parse_endpoints(listen.read(cx).text(), target.read(cx).text())?;
        let direction = self.direction;
        let entered = name.read(cx).text().trim();
        let name = (!entered.is_empty()).then(|| entered.to_owned());
        if let Some(id) = &self.existing {
            Ok(PeerForward::Edit {
                id: id.clone(),
                name,
                listen,
                target,
                direction,
            })
        } else {
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
        let (owner, id, rule) = match existing {
            Some((owner, id)) => {
                let Some(rules) = self
                    .forwards
                    .entries
                    .get(owner)
                    .and_then(|entry| entry.rules.as_ref())
                else {
                    return;
                };
                let Some(rule) = rules.get(id) else { return };
                (Some(owner.to_owned()), Some(id), Some(rule))
            }
            None => {
                if !self.can_add_rule(self.forward_filters.scope.device.as_deref()) {
                    return;
                }
                let owner = self.forward_filters.scope.device.clone().or_else(|| {
                    let mut devices = self.rule_devices();
                    let (id, _) = devices.next()?;
                    devices.next().is_none().then(|| id.clone())
                });
                (owner, None, None)
            }
        };
        self.rule_form.existing = id.map(str::to_owned);
        self.rule_form.direction = rule
            .map_or(self.forward_filters.scope.direction.unwrap_or_default(), |r| {
                r.direction
            });
        let values = [
            rule.and_then(|r| r.name.as_deref()).unwrap_or(""),
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

    pub(super) fn remove_forward(&mut self, owner: &str, id: String, cx: &mut Context<Self>) {
        if !matches!(self.page, Page::Forwards(_)) {
            return;
        }
        if self.presence.online(owner)
            && !self.forwards.entries.get(owner)
                .and_then(|entry| entry.rules.as_ref())
                .is_some_and(|rules| rules.contains_key(&id))
        {
            Toast::show("Forwarding rule unavailable", self.window, cx);
            return;
        }
        self.send_forward(owner, PeerForward::Remove { id }, cx);
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
    ) -> paxri_ui::components::ScrollArea {
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
            let options: Vec<_> = self
                .rule_devices()
                .map(|(id, peer)| (id.clone(), peer.name.clone(), peer.platform))
                .collect();
            let placeholder = match owner.and_then(|id| self.peers.get(id)) {
                Some(peer) => format!("{} (unavailable)", peer.name),
                None => "Choose a paired device".to_owned(),
            };
            let devices = device_select(
                "rule-device",
                owner.map(str::to_owned),
                placeholder,
                &options,
                cx.listener(|this, id: &String, _, cx| {
                    if let Page::ForwardEdit { owner, .. } = &mut this.page {
                        *owner = Some(id.clone());
                        cx.notify();
                    }
                }),
            );
            fields.field("Device", devices);
        }
        fields.field("Rule name", name.clone());
        let direction = direction_select(
            "forward-direction",
            form.direction,
            cx.listener(|this, direction: &Direction, _, cx| {
                this.rule_form.direction = *direction;
                cx.notify();
            }),
        );
        let (listen_label, target_label) = endpoint_labels(form.direction);
        let peer_name = owner
            .and_then(|id| self.peers.get(id))
            .map_or("the peer", |p| p.name.as_str());
        fields.field("Direction", direction);
        fields.field(listen_label, listen.clone());
        fields.field(target_label, target.clone());
        let mut page = Self::settings_column()
            .child(fields.render_rows(ui))
            .child(ui.muted(direction_summary(form.direction, peer_name)));
        if form.saving {
            page = page.child(ui.muted("Saving…"));
        } else if owner.is_some_and(|id| !self.presence.online(id)) {
            page = page.child(ui.muted("Device offline."));
        } else if form.existing.is_none() && !self.can_add_rule(None) {
            page = page.child(ui.muted("No connected devices can create forwarding rules."));
        }
        self.settings_body("forward-edit-page", page)
    }
}
