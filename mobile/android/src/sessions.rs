use super::*;
use paxri_ui::components::directory::{
    DeviceOption, SessionLocation, session_entries, session_list_notice,
};
use paxri_ui::components::{DropdownContent, DropdownState, Icon, ListView};

impl Mobile {
    pub(super) fn session_menu(
        &self,
        peer: &str,
        key: SessionKey,
        title: SharedString,
        can_end: bool,
        cx: &Context<Self>,
    ) -> DropdownContent {
        let peer = peer.to_owned();
        let view = cx.weak_entity();
        let rename_peer = peer.clone();
        let menu = DropdownContent::new().item(
            "rename-session",
            Icon::Pencil.label("Rename session"),
            move |_, cx| {
                let _ = view.update(cx, |v, cx| {
                    if let Err(e) = activity_action(
                        &v.android,
                        ActivityAction::RenameSession(&rename_peer, key, &title),
                    ) {
                        Toast::show(e, v.window, cx);
                    }
                });
            },
        );
        if !can_end {
            return menu;
        }
        let view = cx.weak_entity();
        menu.end_session(move |_, cx| {
            let _ = view.update(cx, |v, cx| v.end_listed_session(&peer, key, cx));
        })
    }

    pub(super) fn rename_session(
        &mut self,
        peer: String,
        key: SessionKey,
        name: String,
        cx: &mut Context<Self>,
    ) {
        let local = self
            .sessions
            .values()
            .find(|s| s.peer == peer && s.key == key);
        if local.is_none()
            && !self
                .hosted
                .get(&peer)
                .is_some_and(|keys| keys.contains(&key))
        {
            return;
        }
        let mut title = local.map(|s| s.title.read(cx)).or_else(|| self.session_names.get(&peer, key)).cloned().unwrap_or_default();
        if !title.rename(&name) { return; }
        if let Err(e) = self.session_names.set(&peer, key, &title) {
            Toast::show(format!("Couldn’t save session name: {e}"), self.window, cx);
            return;
        }
        if let Some(s) = local {
            s.title.update(cx, |current, cx| { *current = title; cx.notify(); });
            self.assign_terminal_defaults(cx);
        }
        cx.notify();
    }

    fn end_listed_session(&mut self, peer: &str, key: SessionKey, cx: &mut Context<Self>) {
        if let Some(s) = self
            .sessions
            .values()
            .find(|s| s.peer == peer && s.key == key && !matches!(s.state, SessionState::Ended(_)))
        {
            self.end_session(s.attachment.session, cx);
            return;
        }
        let Some(client) = self.client.clone() else {
            return;
        };
        let peer = peer.to_owned();
        if !self
            .hosted
            .get(&peer)
            .is_some_and(|keys| keys.contains(&key))
            || !self.ending_hosted.insert((peer.clone(), key))
        {
            return;
        }
        let tx = self.updates.clone();
        self.rt.spawn(async move {
            let result = client
                .manage_terminal(&peer, TerminalRequest::End(key))
                .await
                .map(|_| ())
                .map_err(|e| e.to_string());
            let _ = tx.send(Update::EndedHosted(peer, key, result)).await;
        });
    }

    pub(super) fn show_sessions(&mut self, peer: Option<String>, cx: &mut Context<Self>) {
        self.session_filters.for_device(peer.clone(), cx);
        self.navigate(Page::Sessions(peer), cx);
        cx.notify();
    }

    pub(super) fn sessions_page(&mut self, ui: Ui, window: &mut Window, cx: &mut Context<Self>) -> Div {
        self.session_menus.clear();
        let filters = &self.session_filters;
        let query = filters.query(cx);
        let mut devices: Vec<DeviceOption> = self
            .peers
            .iter()
            .map(|(id, p)| (id.clone(), p.name.clone(), p.platform))
            .collect();
        devices.sort_by(|a, b| a.1.cmp(&b.1).then(a.0.cmp(&b.0)));
        let targets = devices
            .iter()
            .filter(|(id, _, _)| {
                self.presence.online(id)
                    && self.peers[id].supports_shell()
                    && filters.scope.device.as_ref().is_none_or(|v| v == id)
            })
            .cloned()
            .collect();
        let launcher = ui.session_launcher(
            targets,
            cx.listener(|v, id: &String, w, cx| v.open(id.clone(), None, w, cx)),
        );
        let mut rows = Vec::new();
        let mut failed = Vec::new();
        let mut total = 0;
        let mut initial_request = false;
        for (peer, name, _) in &devices {
            let available = self.presence.online(peer) && self.peers[peer].supports_shell();
            if available && filters.scope.device.as_ref().is_none_or(|v| v == peer) {
                if self.session_list_errors.contains(peer) {
                    failed.push(name.as_str());
                }
                initial_request |=
                    self.listing_peers.contains_key(peer) && !self.hosted.contains_key(peer);
            }
            let hosted = self
                .hosted
                .get(peer)
                .filter(|_| available)
                .into_iter()
                .flatten()
                .copied();
            let local = self.sessions.values().filter(|s| &s.peer == peer).map(|s| {
                (
                    s.key,
                    s.title.read(cx).label(&s.name),
                    !matches!(s.state, SessionState::Ended(_)),
                    s.attachment.session,
                )
            });
            let entries = session_entries(name.clone().into(), hosted, local);
            total += entries.len();
            for (key, (title, state, local)) in entries {
                let title = if local.is_none() { self.session_names.get(peer, key).map(|t| t.label(&title)).unwrap_or(title) } else { title };
                if !filters.matches(&query, peer, name, &title, key.number, state) {
                    continue;
                }
                let peer = peer.clone();
                let id = SharedString::from(format!(
                    "session-actions-{peer}-{}-{}",
                    key.number, key.nonce
                ));
                let menu = DropdownState::local(id.clone().into(), window, cx);
                let can_end = state != SessionLocation::Ended
                    && !local
                        .and_then(|id| self.sessions.get(&id))
                        .is_some_and(|s| matches!(s.state, SessionState::Stopping))
                    && !self.ending_hosted.contains(&(peer.clone(), key));
                let content = self.session_menu(&peer, key, title.clone(), can_end, cx);
                let actions = ui.actions_menu(
                    id,
                    format!("Actions for {title}").into(),
                    &menu,
                    content,
                    cx,
                );
                self.session_menus.push(menu);
                let target = peer.clone();
                rows.push(ui.session_row(
                    &peer,
                    name,
                    key,
                    title,
                    state,
                    Some(actions),
                    cx.listener(move |v, _, w, cx| {
                        if let Some(id) = local {
                            v.navigate(Page::Terminal(id), cx);
                            cx.notify();
                        } else {
                            v.open(target.clone(), Some(key), w, cx);
                        }
                    }),
                ));
            }
        }
        let has_action = launcher.is_some();
        div()
            .relative()
            .min_h_0()
            .flex_1()
            .flex()
            .flex_col()
            .px_3()
            .pt_2()
            .gap_2()
            .child(filters.controls(&devices, |v| &mut v.session_filters, cx).with_state(&self.page_menu))
            .when(!rows.is_empty() && !failed.is_empty(), |d| d.child(ui.muted(format!("Couldn’t refresh sessions on {}.", failed.join(", ")))))
            .when_some(
                if self.starting_network && rows.is_empty() { Some(paxri_ui::components::ListState::loading("Loading sessions…")) }
                else { session_list_notice(total, rows.len(), initial_request, &failed) },
                |d, state| d.child(state.render(ui)),
            )
            .when(!rows.is_empty(), |page| page.child(
                ListView::scrollable("session-list")
                    .with_floating_action(has_action)
                    .children(rows),
            ))
            .when_some(launcher, |page, button| {
                page.child(paxri_ui::components::floating_corner().child(button))
            })
    }
}
