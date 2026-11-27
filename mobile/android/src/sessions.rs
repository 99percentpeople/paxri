use super::*;
use alacritty_terminal::grid::{Dimensions, Scroll};
use paxri_ui::components::directory::{
    DeviceOption, named_session_entries, session_list_notice,
};
use paxri_ui::components::{DropdownContent, Icon, ListView};
use paxri_ui::terminal::title::next_session_number;

pub(super) use paxri_ui::terminal::lifecycle::{
    LifecycleEffect, LifecycleEvent, PeerConnectionState, PeerLifecycle, SessionAction,
    SessionState, StreamEnd,
};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct AttachmentId {
    pub(crate) session: u64,
    pub(crate) generation: u64,
}

pub(super) struct Session {
    pub(super) id: u64,
    pub(super) key: SessionKey,
    pub(super) peer: String,
    pub(super) name: SharedString,
    pub(super) screen: Emulator,
    pub(super) bell: paxri_ui::terminal::bell::BellState,
    pub(super) wheel: paxri_ui::terminal::mouse::Wheel,
    pub(super) touch_mouse: paxri_ui::terminal::mouse::TouchMouseDrag,
    pub(super) composing: Preedit,
    pub(super) title: Entity<TabTitle>,
    pub(super) input: mpsc::UnboundedSender<Frame>,
    pub(super) probe_now: Arc<tokio::sync::Notify>,
    pub(super) job: Option<JoinHandle<()>>,
    pub(super) attempt_started_at: std::time::Instant,
}

impl Session {
    pub(super) fn write_input(&mut self, bytes: Vec<u8>) {
        self.screen.input_started();
        self.screen.scroll(Scroll::Bottom);
        let _ = self.input.send(Frame::Data(bytes));
    }

    pub(super) fn grid_size(&self) -> GridSize {
        GridSize {
            rows: self.screen.term.screen_lines() as u16,
            cols: self.screen.term.columns() as u16,
        }
    }

    pub(super) fn abort_job(&mut self) {
        if let Some(job) = self.job.take() {
            job.abort();
        }
        self.composing.clear();
        self.attempt_started_at = std::time::Instant::now();
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        if let Some(job) = self.job.take() {
            job.abort();
        }
    }
}

impl Mobile {
    pub(super) fn peer_connection(&self, peer: &str) -> PeerConnectionState {
        self.lifecycles
            .get(peer)
            .map(PeerLifecycle::connection)
            .unwrap_or_default()
    }

    pub(super) fn session_state(&self, id: u64) -> Option<&SessionState> {
        let session = self.sessions.get(&id)?;
        self.lifecycles.get(&session.peer)?.session_state(id)
    }

    pub(super) fn session_generation_matches(&self, att: AttachmentId) -> bool {
        let Some(session) = self.sessions.get(&att.session) else {
            return false;
        };
        self.lifecycles
            .get(&session.peer)
            .and_then(|lifecycle| lifecycle.session(att.session))
            .is_some_and(|managed| managed.generation == att.generation)
    }

    pub(super) fn session_connected(&self, id: u64) -> bool {
        self.session_state(id).is_some_and(SessionState::is_active)
    }

    pub(super) fn session_labels(&self, id: u64, cx: &App) -> (SharedString, String) {
        let session = &self.sessions[&id];
        let connection = self.peer_connection(&session.peer);
        let subtitle = self
            .session_state(id)
            .map(|state| state.subtitle(session.key.number, connection))
            .unwrap_or_else(|| format!("Session {}", session.key.number));
        (session.title.read(cx).label(&session.name), subtitle)
    }

    pub(super) fn emit_lifecycle(
        &mut self,
        peer: &str,
        event: LifecycleEvent,
        cx: &mut Context<Self>,
    ) {
        let effects = self
            .lifecycles
            .entry(peer.to_owned())
            .or_default()
            .handle(event);
        self.apply_lifecycle_effects(peer, effects, cx);
    }

    pub(super) fn dispatch_all_pending(&mut self, cx: &mut Context<Self>) {
        let peers: Vec<String> = self.lifecycles.keys().cloned().collect();
        let active_session = match self.page {
            Page::Terminal(id) => Some(id),
            _ => None,
        };
        let visible = *ACTIVITY_VISIBLE.borrow();
        for peer in peers {
            self.emit_lifecycle(
                &peer,
                LifecycleEvent::DispatchPending {
                    active_session,
                    visible,
                },
                cx,
            );
        }
    }

    fn spawn_session_task(
        &mut self,
        id: u64,
        generation: u64,
        connection_id: u64,
        action: TerminalRequest,
    ) {
        let Some(client) = self.client.clone() else {
            return;
        };
        let Some(session) = self.sessions.get_mut(&id) else {
            return;
        };
        if let Some(old) = session.job.take() {
            old.abort();
        }
        let attachment = AttachmentId {
            session: id,
            generation,
        };
        let (input, probe_now, job) = client.spawn_terminal(
            &self.rt,
            session.peer.clone(),
            connection_id,
            action,
            self.updates.clone(),
            attachment,
        );
        session.input = input;
        session.probe_now = probe_now;
        session.attempt_started_at = std::time::Instant::now();
        session.job = Some(job);
    }

    fn apply_lifecycle_effects(
        &mut self,
        peer: &str,
        effects: Vec<LifecycleEffect>,
        cx: &mut Context<Self>,
    ) {
        let mut dispatch_pending = false;
        for effect in effects {
            match effect {
                LifecycleEffect::SpawnCreate {
                    id,
                    generation,
                    connection_id,
                } => {
                    if let Some(session) = self.sessions.get(&id) {
                        let grid = session.grid_size();
                        let action = TerminalRequest::Create(
                            session.key,
                            PtySize {
                                rows: grid.rows,
                                cols: grid.cols,
                            },
                        );
                        self.spawn_session_task(id, generation, connection_id, action);
                    }
                }
                LifecycleEffect::SpawnAttach {
                    id,
                    generation,
                    connection_id,
                } => {
                    if let Some(session) = self.sessions.get(&id) {
                        let grid = session.grid_size();
                        let action = TerminalRequest::Attach(
                            session.key,
                            PtySize {
                                rows: grid.rows,
                                cols: grid.cols,
                            },
                        );
                        self.spawn_session_task(id, generation, connection_id, action);
                    }
                }
                LifecycleEffect::SpawnEnd {
                    id,
                    generation,
                    connection_id,
                } => {
                    if let Some(session) = self.sessions.get(&id) {
                        let action = TerminalRequest::End(session.key);
                        self.spawn_session_task(id, generation, connection_id, action);
                    }
                }
                LifecycleEffect::SendExit { id } => {
                    if let Some(session) = self.sessions.get(&id) {
                        let _ = session.input.send(Frame::Exit(0));
                    }
                }
                LifecycleEffect::ProbeStream { id } => {
                    if let Some(session) = self.sessions.get(&id) {
                        session.probe_now.notify_one();
                    }
                }
                LifecycleEffect::AbortJob { id } => {
                    if let Some(session) = self.sessions.get_mut(&id) {
                        session.abort_job();
                    }
                }
                LifecycleEffect::ClearComposing { id } => {
                    if let Some(session) = self.sessions.get_mut(&id) {
                        session.composing.clear();
                    }
                }
                LifecycleEffect::StartInput { id } => {
                    if self.page == Page::Terminal(id)
                        && let Some(session) = self.sessions.get(&id)
                    {
                        Self::start_input(session);
                    }
                }
                LifecycleEffect::StopInput { id } => {
                    if self.page == Page::Terminal(id) {
                        gpui_mobile::set_text_input_callback(None);
                        gpui_mobile::hide_keyboard();
                    }
                }
                LifecycleEffect::ScheduleRetry { id } => {
                    let updates = self.updates.clone();
                    let peer = peer.to_owned();
                    self.rt.spawn(async move {
                        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                        let _ = updates.send(Update::RetrySession(peer, id)).await;
                    });
                }
                LifecycleEffect::DismissSession {
                    id,
                    remove_hosted,
                    toast_gone,
                } => {
                    if toast_gone && self.page == Page::Terminal(id) {
                        Toast::show("Session has ended on the gateway", self.window, cx);
                    }
                    if let Some(removed) = self.sessions.remove(&id) {
                        if remove_hosted {
                            self.remove_hosted(&removed.peer, removed.key);
                        }
                        if self.page == Page::Terminal(id) {
                            self.navigate(self.terminal_return.clone(), cx);
                        }
                    }
                }
                LifecycleEffect::SyncDefaults => {
                    self.assign_terminal_defaults(cx);
                }
                LifecycleEffect::DispatchPending => {
                    dispatch_pending = true;
                }
                LifecycleEffect::RefreshPresence => {
                    if let Some(client) = &self.client {
                        client.refresh_presence();
                    }
                }
            }
        }
        if dispatch_pending {
            let active_session = match self.page {
                Page::Terminal(id) => Some(id),
                _ => None,
            };
            let visible = *ACTIVITY_VISIBLE.borrow();
            let next = self
                .lifecycles
                .entry(peer.to_owned())
                .or_default()
                .handle(LifecycleEvent::DispatchPending {
                    active_session,
                    visible,
                });
            if !next.is_empty() {
                self.apply_lifecycle_effects(peer, next, cx);
            }
        }
    }

    pub(super) fn assign_terminal_defaults(&mut self, cx: &mut App) {
        let titles = self
            .sessions
            .values()
            .map(|s| (s.name.clone(), s.title.clone()))
            .collect::<Vec<_>>();
        let open: BTreeSet<_> = self
            .sessions
            .values()
            .map(|s| (s.peer.as_str(), s.key))
            .collect();
        let reserved = self
            .hosted
            .iter()
            .flat_map(|(peer, keys)| {
                let fallback: SharedString = self
                    .peers
                    .get(peer)
                    .map_or(peer.as_str(), |p| p.name.as_str())
                    .to_owned()
                    .into();
                self.session_names
                    .reserved_labels(peer, &fallback, |key| {
                        keys.contains(&key) && !open.contains(&(peer.as_str(), key))
                    })
            })
            .collect();
        TabTitle::assign_with_reserved(&titles, reserved, cx);
        for s in self.sessions.values() {
            if self
                .session_state(s.id)
                .is_some_and(SessionState::is_confirmed_on_host)
                && let Err(e) = self.session_names.set(&s.peer, s.key, s.title.read(cx))
            {
                log::warn!("Saving session title: {e}");
            }
        }
    }

    pub(super) fn refresh_sessions(&mut self) {
        let Some(client) = &self.client else { return };
        for (peer, saved) in &self.peers {
            match &self.page {
                Page::Devices | Page::Sessions(_) => {}
                Page::Peer(selected) if selected == peer => {}
                _ => continue,
            }
            if !self.peer_connection(peer).is_connected()
                || !saved.capabilities.is_some_and(|c| c.managed_terminals)
                || self.listing_peers.contains_key(peer)
            {
                continue;
            }
            self.listing_peers.insert(peer.clone(), Vec::new());
            let peer = peer.clone();
            let client = client.clone();
            let updates = self.updates.clone();
            self.rt.spawn(async move {
                let keys = tokio::time::timeout(
                    std::time::Duration::from_secs(2),
                    client.manage_terminal(&peer, TerminalRequest::List),
                )
                .await
                .ok()
                .and_then(Result::ok);
                let _ = updates.send(Update::Hosted(peer, keys)).await;
            });
        }
    }

    pub(super) fn end_session(&mut self, id: u64, cx: &mut Context<Self>) {
        let Some(peer) = self.sessions.get(&id).map(|s| s.peer.clone()) else {
            return;
        };
        self.emit_lifecycle(&peer, LifecycleEvent::UserStop { id }, cx);
        cx.notify();
    }

    pub(super) fn dismiss_session(&mut self, id: u64, cx: &mut Context<Self>) {
        let Some(peer) = self.sessions.get(&id).map(|s| s.peer.clone()) else {
            return;
        };
        self.emit_lifecycle(&peer, LifecycleEvent::Dismiss { id }, cx);
    }

    pub(super) fn remove_hosted(&mut self, peer: &str, key: SessionKey) {
        if let Err(e) = self.session_names.retain(peer, |k| k != key) {
            log::warn!("Removing session title: {e}");
        }
        if let Some(keys) = self.hosted.get_mut(peer) {
            keys.retain(|k| *k != key);
        }
        if let Some(ended) = self.listing_peers.get_mut(peer) {
            ended.push(key);
        }
    }

    pub(super) fn open(
        &mut self,
        peer: String,
        saved_key: Option<SessionKey>,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(key) = saved_key
            && let Some(existing) = self.sessions.values().find(|s| {
                s.peer == peer
                    && s.key == key
                    && self.session_state(s.id).is_some_and(SessionState::is_open)
            })
        {
            self.navigate(Page::Terminal(existing.id), cx);
            cx.notify();
            return;
        }
        if self.client.is_none() {
            return;
        }
        let Some(saved) = self.peers.get(&peer) else {
            return;
        };
        if !saved.supports_shell() || !self.peer_connection(&peer).is_connected() {
            return;
        }
        let name = saved.name.clone().into();
        let number = self.last_session_number.entry(peer.clone()).or_default();
        let key = match saved_key {
            Some(key) => key,
            None => {
                *number = next_session_number(
                    self.hosted.get(&peer).into_iter().flatten().copied(),
                    *number,
                );
                match SessionKey::new(*number) {
                    Ok(key) => key,
                    Err(e) => {
                        Toast::show(e.to_string(), self.window, cx);
                        return;
                    }
                }
            }
        };
        let stale_ids: Vec<u64> = self
            .sessions
            .values()
            .filter(|s| {
                s.peer == peer
                    && s.key == key
                    && self.session_state(s.id).is_none_or(SessionState::is_ended)
            })
            .map(|s| s.id)
            .collect();
        for stale in stale_ids {
            self.emit_lifecycle(&peer, LifecycleEvent::Dismiss { id: stale }, cx);
        }
        self.generation += 1;
        let id = self.generation;
        let font = self.font.clone().scaled(self.terminal_zoom);
        let grid = initial_size(window, font.cell, &self.keybar);
        let title = self.session_names.get(&peer, key).cloned().unwrap_or_default();
        let (input, _) = mpsc::unbounded_channel();
        self.sessions.insert(
            id,
            Session {
                id,
                key,
                peer: peer.clone(),
                name,
                screen: Emulator::with_palette(grid, &Ui::new(window, cx).colors.terminal, font),
                bell: Default::default(),
                wheel: Default::default(),
                touch_mouse: Default::default(),
                composing: Preedit::default(),
                title: cx.new(|_| title),
                input,
                probe_now: Arc::new(tokio::sync::Notify::new()),
                job: None,
                attempt_started_at: std::time::Instant::now(),
            },
        );
        self.emit_lifecycle(
            &peer,
            LifecycleEvent::OpenSession {
                id,
                attach: saved_key.is_some(),
            },
            cx,
        );
        if self.session_state(id).is_none() {
            self.sessions.remove(&id);
            return;
        }
        self.navigate(Page::Terminal(id), cx);
        cx.notify();
    }

    pub(super) fn start_input(s: &Session) {
        let id = s.id;
        gpui_mobile::set_text_input_callback(Some(Box::new(move |text| {
            if text.is_empty() {
                return;
            }
            post(Update::HardwareText(id, text.to_owned()));
        })));
    }

    pub(super) fn session_menu(
        &self,
        peer: &str,
        key: SessionKey,
        title: SharedString,
        action: Option<SessionAction>,
        local: Option<u64>,
        cx: &Context<Self>,
    ) -> DropdownContent {
        let peer = peer.to_owned();
        let view = cx.weak_entity();
        let rename_peer = peer.clone();
        let menu = DropdownContent::new().item(
            "rename-session",
            Icon::Pencil.label("Rename session"),
            move |window, cx| {
                let _ = view.update(cx, |v, cx| {
                    v.open_rename_session_modal(
                        rename_peer.clone(),
                        key,
                        title.to_string(),
                        window,
                        cx,
                    );
                });
            },
        );
        let Some(action) = action else {
            return menu;
        };
        let view = cx.weak_entity();
        match action {
            SessionAction::End => menu.end_session(move |_, cx| {
                let _ = view.update(cx, |v, cx| v.end_listed_session(&peer, key, cx));
            }),
            SessionAction::Cancel => menu.item(
                "cancel-session",
                Icon::Close.label("Cancel session"),
                move |_, cx| {
                    let _ = view.update(cx, |v, cx| {
                        if let Some(id) = local {
                            v.end_session(id, cx);
                        } else {
                            v.end_listed_session(&peer, key, cx);
                        }
                    });
                },
            ),
            SessionAction::Close => menu.item(
                "close-session",
                Icon::Close.label("Close session"),
                move |_, cx| {
                    let _ = view.update(cx, |v, cx| {
                        if let Some(id) = local {
                            v.dismiss_session(id, cx);
                            cx.notify();
                        }
                    });
                },
            ),
        }
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
        let mut title = local
            .map(|s| s.title.read(cx))
            .or_else(|| self.session_names.get(&peer, key))
            .cloned()
            .unwrap_or_default();
        if !title.rename(&name) {
            return;
        }
        if let Err(e) = self.session_names.set(&peer, key, &title) {
            Toast::show(format!("Couldn’t save session name: {e}"), self.window, cx);
            return;
        }
        if let Some(s) = local {
            s.title.update(cx, |current, cx| {
                *current = title;
                cx.notify();
            });
            self.assign_terminal_defaults(cx);
        }
        cx.notify();
    }

    fn end_listed_session(&mut self, peer: &str, key: SessionKey, cx: &mut Context<Self>) {
        if let Some(id) = self
            .sessions
            .values()
            .find(|s| {
                s.peer == peer
                    && s.key == key
                    && self.session_state(s.id).is_some_and(SessionState::is_open)
            })
            .map(|s| s.id)
        {
            self.end_session(id, cx);
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

    pub(super) fn sessions_subtitle(&self, cx: &App) -> Option<String> {
        let filters = &self.session_filters;
        let query = filters.query(cx);
        let mut total = 0;
        let mut shown = 0;
        let mut initial_request = false;
        for (peer, p) in &self.peers {
            let available = self.peer_connection(peer).is_connected() && p.supports_shell();
            if available && filters.scope.device.as_ref().is_none_or(|v| v == peer) {
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
                    self.session_state(s.id).is_some_and(SessionState::is_open),
                    s.id,
                )
            });
            let entries = named_session_entries(
                p.name.clone().into(),
                peer,
                &self.session_names,
                hosted,
                local,
            );
            total += entries.len();
            for (key, (title, state, _)) in entries {
                if filters.matches(&query, peer, &p.name, &title, key.number, state) {
                    shown += 1;
                }
            }
        }
        if (self.gateway_status.is_transitioning() || initial_request) && total == 0 {
            return Some("Loading…".into());
        }
        let filtered = filters.has_selections() || !query.is_empty();
        Some(paxri_ui::components::directory::count_summary(
            shown, total, filtered, "session", "sessions",
        ))
    }

    pub(super) fn sessions_page(&self, ui: Ui, cx: &Context<Self>) -> Div {
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
                self.peer_connection(id).is_connected()
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
            let available =
                self.peer_connection(peer).is_connected() && self.peers[peer].supports_shell();
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
                    self.session_state(s.id).is_some_and(SessionState::is_open),
                    s.id,
                )
            });
            let entries = named_session_entries(
                name.clone().into(),
                peer,
                &self.session_names,
                hosted,
                local,
            );
            total += entries.len();
            for (key, (title, state, local)) in entries {
                if !filters.matches(&query, peer, name, &title, key.number, state) {
                    continue;
                }
                let peer = peer.clone();
                let id = SharedString::from(format!(
                    "session-actions-{peer}-{}-{}",
                    key.number, key.nonce
                ));
                let action = if let Some(local_id) = local {
                    self.session_state(local_id).map(SessionState::action)
                } else if !self.ending_hosted.contains(&(peer.clone(), key)) {
                    Some(SessionAction::End)
                } else {
                    None
                };
                let content = self.session_menu(&peer, key, title.clone(), action, local, cx);
                let actions = ui.actions_menu(id, format!("Actions for {title}").into(), content);
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
            .child(filters.controls(&devices, |v| &mut v.session_filters, cx))
            .when(!rows.is_empty() && !failed.is_empty(), |d| {
                d.child(ui.muted(format!(
                    "Couldn’t refresh sessions on {}.",
                    failed.join(", ")
                )))
            })
            .when_some(
                if self.gateway_status.is_transitioning() && rows.is_empty() {
                    Some(paxri_ui::components::ListState::loading("Loading sessions…"))
                } else if matches!(self.gateway_status, GatewayStatus::Stopped) && rows.is_empty() {
                    Some(
                        paxri_ui::components::ListState::empty(Icon::Server, "Gateway stopped")
                            .detail("Start the gateway in Settings → Gateway."),
                    )
                } else {
                    session_list_notice(total, rows.len(), initial_request, &failed)
                },
                |d, state| d.child(state.render(ui)),
            )
            .when(!rows.is_empty(), |page| {
                page.child(
                    ListView::scrollable("session-list")
                        .divided()
                        .with_floating_action(has_action)
                        .children(rows),
                )
            })
            .when_some(launcher, |page, button| {
                page.child(paxri_ui::components::floating_corner().child(button))
            })
    }
}
