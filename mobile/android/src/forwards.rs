use super::*;
use paxri_control::{Direction, PeerForward, forwarding::Rules};
use paxri_ui::components::{
    ButtonVariant::{Destructive, Ghost, Outline},
    DropdownState, Icon, ListItem, Select, actions, heading,
    input::Input,
};
use paxri_ui::settings::{SettingsGroup, SettingsSection, SettingsTable};

enum Entry {
    Loading(u64, tokio::task::JoinHandle<()>),
    Ready(Result<Rules, String>),
}

impl Drop for Entry {
    fn drop(&mut self) {
        if let Self::Loading(_, task) = self {
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
    pub(super) fn invalidate(&mut self, owner: &str) {
        self.entries.remove(owner);
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
        if !matches!(entry, Entry::Loading(pending, _) if *pending == request) {
            return false;
        }
        *entry = Entry::Ready(result);
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
        let Page::Forwards(selected) = &self.page else {
            self.forwards.entries.clear();
            return;
        };
        let eligible = |id: &String, peer: &client::Peer| {
            selected.as_ref().is_none_or(|owner| owner == id)
                && peer.supports_forward_rules()
                && self.presence.online(id)
        };
        self.forwards
            .entries
            .retain(|id, _| self.peers.get(id).is_some_and(|p| eligible(id, p)));
        let Some(client) = &self.client else { return };
        for (id, peer) in &self.peers {
            if !eligible(id, peer) {
                continue;
            }
            match self.forwards.entries.get(id) {
                Some(Entry::Loading(..)) => continue,
                Some(Entry::Ready(_)) if !refresh => continue,
                _ => {}
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
            self.forwards
                .entries
                .insert(id.clone(), Entry::Loading(request, task));
        }
    }

    pub(super) fn forward_list(
        &self,
        selected: Option<&str>,
        ui: Ui,
        cx: &Context<Self>,
    ) -> Stateful<Div> {
        let mut page = div()
            .id("forward-list")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .restrict_scroll_to_axis()
            .p_4()
            .when(self.can_add_rule(selected), |page| page.pb(px(88.)))
            .flex()
            .flex_col()
            .gap_4();
        if self.forwards.entries.is_empty() {
            let message = match selected.and_then(|id| self.peers.get(id)) {
                Some(peer) if !peer.supports_forward_rules() => {
                    "This device does not share forwarding rules."
                }
                Some(_) => "Device offline.",
                None if self.client.is_none() => "Connecting…",
                None => "No connected devices share forwarding rules.",
            };
            return page.child(ui.muted(message));
        }
        for (id, entry) in &self.forwards.entries {
            let peer = &self.peers[id];
            let mut group = ui.section();
            if selected.is_none() {
                group = group.child(heading(peer.name.clone()));
            }
            if !peer.supports_creating_forward_rules() {
                group = group.child(ui.muted("Update this gateway to add rules.").text_xs());
            }
            group = match entry {
                Entry::Loading(..) => group.child(ui.muted("Loading forwarding rules…")),
                Entry::Ready(Err(_)) => group.child(ui.muted("Forwarding rules unavailable.")),
                Entry::Ready(Ok(rules)) if rules.is_empty() => {
                    group.child(ui.muted("No forwarding rules for this device."))
                }
                Entry::Ready(Ok(rules)) => {
                    for (name, rule) in rules {
                        let owner = id.clone();
                        let key = name.clone();
                        let label = ListItem::new(
                            div().font_weight(FontWeight::SEMIBOLD).child(name.clone()),
                        )
                        .detail(ui.muted(rule.route_for(&peer.name)))
                        .detail(ui.muted(rule.state().to_owned()).text_xs());
                        group = group.child(
                            ui.button(
                                Ghost,
                                SharedString::from(format!("rule-{id}-{name}")),
                                label,
                            )
                            .row()
                            .icon(Icon::Pencil)
                            .on_click(cx.listener(
                                move |this, _, _, cx| this.open_rule(Some((&owner, &key)), cx),
                            )),
                        );
                    }
                    group
                }
            };
            page = page.child(group);
        }
        page
    }
}

pub(super) struct RuleForm {
    pub inputs: [Entity<Input>; 3],
    pub device_menu: Entity<DropdownState>,
    pub direction_menu: Entity<DropdownState>,
    pub existing: Option<String>,
    direction: Direction,
    enabled: bool,
    confirm_remove: bool,
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
            enabled: false,
            confirm_remove: false,
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
                let Some(Entry::Ready(Ok(rules))) = self.forwards.entries.get(owner) else {
                    return;
                };
                let Some(rule) = rules.get(name) else { return };
                (Some(owner.to_owned()), Some(name), Some(rule))
            }
            None => {
                if !self.can_add_rule(scope.as_deref()) {
                    return;
                }
                let owner = scope.clone().or_else(|| {
                    let mut devices = self.rule_devices();
                    let (id, _) = devices.next()?;
                    devices.next().is_none().then(|| id.clone())
                });
                (owner, None, None)
            }
        };
        self.rule_form.existing = name.map(str::to_owned);
        self.rule_form.enabled = rule.is_some_and(|r| r.enabled());
        self.rule_form.direction = rule.map_or(Direction::Local, |r| r.direction);
        self.rule_form.confirm_remove = false;
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
        if self.rule_form.saving || !self.can_save_rule(Some(owner)) {
            return;
        }
        let Some((client, peer)) = self.client.as_ref().zip(self.peers.get(owner)) else {
            return;
        };
        let name = peer.name.clone();
        let owner = owner.clone();
        let client = client.clone();
        self.rule_form.saving = true;
        self.rule_form.confirm_remove = false;
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
        if !self.can_save_rule(owner) || self.rule_form.saving || self.rule_form.confirm_remove {
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
            "forward-listener",
            form.direction,
            cx.listener(|this, direction: &Direction, _, cx| {
                this.rule_form.direction = *direction;
                cx.notify();
            }),
        )
        .with_optional_state(Some(&form.direction_menu))
        .option(Direction::Local, "This phone")
        .option(Direction::Remote, "Paired device");
        fields.field("Listen on", direction);
        fields.field("Listen address", listen.clone());
        fields.field(
            if form.direction == Direction::Local {
                "Target on paired device"
            } else {
                "Target on this phone"
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
        if let Some(name) = &form.existing
            && form.confirm_remove
        {
            let key = name.clone();
            return page.child(ui.muted(format!("Remove {name}?"))).child(
                actions()
                    .child(
                        ui.button(Outline, "cancel-remove", "Cancel")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.rule_form.confirm_remove = false;
                                cx.notify();
                            })),
                    )
                    .child(ui.button(Destructive, "confirm-remove", "Remove").on_click(
                        cx.listener(move |this, _, _, cx| {
                            this.change_forward(PeerForward::Remove { name: key.clone() }, cx);
                        }),
                    )),
            );
        }
        page.when_some(form.existing.clone(), |page, name| {
            let enabled = !form.enabled;
            page.child(
                actions()
                    .child(
                        ui.button(
                            Outline,
                            "enable-forward",
                            if enabled { "Resume" } else { "Pause" },
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.change_forward(
                                PeerForward::SetEnabled {
                                    name: name.clone(),
                                    enabled,
                                },
                                cx,
                            );
                        })),
                    )
                    .child(
                        ui.button(Ghost, "remove-forward", "Remove")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.rule_form.confirm_remove = true;
                                cx.notify();
                            })),
                    ),
            )
        })
    }
}
