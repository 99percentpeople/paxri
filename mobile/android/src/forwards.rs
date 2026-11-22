use super::*;
use peerhatch_control::{Direction, PeerForward, forwarding::Rules};
use peerhatch_ui::components::heading;
use peerhatch_ui::components::{
    ButtonVariant::{Destructive, Ghost, Outline},
    Icon,
    input::Input,
};

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
            if peer.capabilities.is_some_and(|c| c.create_forward_rules) {
                let owner = id.clone();
                group = group.child(
                    ui.button(Outline, SharedString::from(format!("add-{id}")), "Add rule")
                        .icon(Icon::Plus)
                        .on_click(
                            cx.listener(move |this, _, _, cx| this.open_rule(&owner, None, cx)),
                        ),
                );
            } else {
                group = group.child(ui.muted("Update this gateway to add rules.").text_xs());
            }
            group = match entry {
                Entry::Loading(..) => group.child(ui.muted("Loading forwarding rules…")),
                Entry::Ready(Err(error)) => group.child(ui.alert(error.clone())),
                Entry::Ready(Ok(rules)) if rules.is_empty() => {
                    group.child(ui.muted("No forwarding rules for this device."))
                }
                Entry::Ready(Ok(rules)) => {
                    for (name, rule) in rules {
                        let owner = id.clone();
                        let key = name.clone();
                        let label = div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(div().font_weight(FontWeight::SEMIBOLD).child(name.clone()))
                            .child(ui.muted(rule.route_for(&peer.name)))
                            .child(ui.muted(rule.state().to_owned()).text_xs());
                        group = group.child(
                            ui.button(
                                Ghost,
                                SharedString::from(format!("rule-{id}-{name}")),
                                label,
                            )
                            .row()
                            .icon(Icon::Pencil)
                            .on_click(cx.listener(
                                move |this, _, _, cx| this.open_rule(&owner, Some(&key), cx),
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
    pub existing: Option<String>,
    direction: Direction,
    enabled: bool,
    confirm_remove: bool,
    pub saving: bool,
    pub error: String,
}

impl RuleForm {
    pub(super) fn new(cx: &mut App) -> Self {
        Self {
            inputs: ["e.g. web", "8080 or IP:port", "3000 or host:port"]
                .map(|placeholder| Input::setting(placeholder, cx)),
            existing: None,
            direction: Direction::Local,
            enabled: false,
            confirm_remove: false,
            saving: false,
            error: String::new(),
        }
    }

    fn save_action(&self, cx: &App) -> Result<PeerForward, String> {
        let [name, listen, target] = &self.inputs;
        let listen = peerhatch_control::listen_address(listen.read(cx).text().trim())?;
        let target = peerhatch_control::tcp_target(target.read(cx).text().trim())?;
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
    fn open_rule(&mut self, owner: &str, name: Option<&str>, cx: &mut Context<Self>) {
        let Page::Forwards(scope) = &self.page else {
            return;
        };
        if self.rule_form.saving {
            return;
        }
        let rule = name.and_then(|key| match self.forwards.entries.get(owner)? {
            Entry::Ready(Ok(rules)) => rules.get(key),
            _ => None,
        });
        if name.is_some() && rule.is_none() {
            return;
        }
        self.rule_form.existing = name.map(str::to_owned);
        self.rule_form.enabled = rule.is_some_and(|r| r.enabled());
        self.rule_form.direction = rule.map_or(Direction::Local, |r| r.direction);
        self.rule_form.error.clear();
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
                owner: owner.to_owned(),
                scope: scope.clone(),
            },
            cx,
        );
        cx.notify();
    }

    fn change_forward(&mut self, action: PeerForward, cx: &mut Context<Self>) {
        let Page::ForwardEdit { owner, .. } = &self.page else {
            return;
        };
        if self.rule_form.saving {
            return;
        }
        let Some((client, peer)) = self.client.as_ref().zip(self.peers.get(owner)) else {
            return;
        };
        let name = peer.name.clone();
        let owner = owner.clone();
        let client = client.clone();
        self.rule_form.saving = true;
        self.rule_form.error.clear();
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

    pub(super) fn save_rule_button(
        &self,
        owner: &str,
        ui: Ui,
        cx: &Context<Self>,
    ) -> Option<AnyElement> {
        if !self.presence.online(owner) || self.rule_form.saving || self.rule_form.confirm_remove {
            return None;
        }
        Some(
            ui.button(Ghost, "save-forward", Icon::Check.draw(22.))
                .circular()
                .tooltip(peerhatch_ui::components::tooltip("Save rule".into()))
                .role(Role::Button)
                .aria_label("Save rule")
                .on_click(
                    cx.listener(|this, _, _, cx| match this.rule_form.save_action(cx) {
                        Ok(action) => this.change_forward(action, cx),
                        Err(error) => {
                            this.rule_form.error = error;
                            cx.notify();
                        }
                    }),
                )
                .into_any_element(),
        )
    }

    pub(super) fn rule_editor(&self, owner: &str, ui: Ui, cx: &Context<Self>) -> Stateful<Div> {
        let form = &self.rule_form;
        let [name, listen, target] = &form.inputs;
        let field = |label: &'static str, input: &Entity<Input>| {
            div()
                .flex()
                .flex_col()
                .gap_1()
                .child(label)
                .child(input.clone())
        };
        let choose = |key, text, direction| {
            ui.button(Outline, key, text)
                .pressed(form.direction == direction)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.rule_form.direction = direction;
                    cx.notify();
                }))
        };
        let mut page = div()
            .id("forward-edit-page")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&self.settings_scroll)
            .restrict_scroll_to_axis()
            .flex()
            .flex_col()
            .p_4()
            .gap_3()
            .child(match &form.existing {
                Some(key) => heading(key.clone()),
                None => field("Rule name", name),
            })
            .child(
                div().flex().flex_col().gap_1().child("Listen on").child(
                    div()
                        .flex()
                        .flex_wrap()
                        .gap_2()
                        .child(choose("listen-phone", "This phone", Direction::Local))
                        .child(choose("listen-peer", "Paired device", Direction::Remote)),
                ),
            )
            .child(field("Listen address", listen))
            .child(field(
                if form.direction == Direction::Local {
                    "Target on paired device"
                } else {
                    "Target on this phone"
                },
                target,
            ));
        if !form.error.is_empty() {
            page = page.child(ui.alert(form.error.clone()));
        }
        if form.saving {
            return page.child(ui.muted("Saving…"));
        }
        if !self.presence.online(owner) {
            return page.child(ui.muted("Device offline."));
        }
        if let Some(name) = &form.existing
            && form.confirm_remove
        {
            let key = name.clone();
            return page.child(ui.muted(format!("Remove {name}?"))).child(
                div()
                    .flex()
                    .gap_2()
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
                div()
                    .flex()
                    .flex_wrap()
                    .gap_2()
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
