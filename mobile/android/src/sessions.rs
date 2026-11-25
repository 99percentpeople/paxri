use super::*;
use alacritty_terminal::grid::{Dimensions, Scroll};
use paxri_ui::components::directory::{
    DeviceOption, SessionLocation, named_session_entries, session_list_notice,
};
use paxri_ui::components::{DropdownContent, Icon, ListView};
use paxri_ui::terminal::title::next_session_number;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct AttachmentId {
    pub(crate) session: u64,
    pub(crate) generation: u64,
}

pub(super) struct Session {
    pub(super) attachment: AttachmentId,
    pub(super) key: SessionKey,
    pub(super) peer: String,
    pub(super) name: SharedString,
    pub(super) state: SessionState,
    pub(super) screen: Emulator,
    pub(super) bell: paxri_ui::terminal::bell::BellState,
    pub(super) wheel: paxri_ui::terminal::mouse::Wheel,
    pub(super) touch_mouse: paxri_ui::terminal::mouse::TouchMouseDrag,
    pub(super) composing: Preedit,
    pub(super) title: Entity<TabTitle>,
    pub(super) input: mpsc::UnboundedSender<Frame>,
    pub(super) job: Option<JoinHandle<()>>,
    pub(super) retry_at: std::time::Instant,
}

pub(super) enum SessionState {
    Connecting,
    Connected,
    Reconnecting,
    Stopping,
    Ended(String),
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

    pub(super) fn connected(&self) -> bool {
        matches!(self.state, SessionState::Connected)
    }

    pub(super) fn detach(&mut self) {
        if let Some(job) = self.job.take() {
            job.abort();
        }
        // A result or replay from the old stream may already be in the UI queue.
        self.attachment.generation += 1;
        self.composing.clear();
        self.retry_at = std::time::Instant::now();
        if !matches!(self.state, SessionState::Stopping) {
            self.state = SessionState::Reconnecting;
        }
    }

    pub(super) fn labels(&self, online: bool, cx: &App) -> (SharedString, String) {
        let number = self.key.number;
        let subtitle = match &self.state {
            SessionState::Connected => format!("Session {number}"),
            SessionState::Connecting => format!("Session {number} · Connecting…"),
            SessionState::Reconnecting if !online => format!("Session {number} · Device offline"),
            SessionState::Reconnecting => format!("Session {number} · Reconnecting…"),
            SessionState::Stopping if !online => {
                format!("Session {number} · Device offline · Stop pending")
            }
            SessionState::Stopping => format!("Session {number} · Stopping…"),
            SessionState::Ended(reason) => format!("Session {number} · {reason}"),
        };
        (self.title.read(cx).label(&self.name), subtitle)
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
            if !matches!(s.state, SessionState::Ended(_))
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
            if !self.presence.online(peer)
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
        let Some(session) = self.sessions.get_mut(&id) else {
            return;
        };
        match session.state {
            SessionState::Connecting | SessionState::Connected | SessionState::Reconnecting => {
                session.state = SessionState::Stopping;
                let _ = session.input.send(Frame::Exit(0));
                self.retry_sessions();
            }
            SessionState::Stopping => return,
            SessionState::Ended(_) => self.dismiss_session(id, cx),
        }
        cx.notify();
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

    pub(super) fn retry_sessions(&mut self) {
        let Some(client) = &self.client else { return };
        if !*ACTIVITY_VISIBLE.borrow() {
            return;
        }
        let now = std::time::Instant::now();
        for session in self.sessions.values_mut() {
            if session.job.is_some()
                || session.retry_at > now
                || !self.presence.online(&session.peer)
            {
                continue;
            }
            let action = match session.state {
                SessionState::Reconnecting => {
                    let grid = session.grid_size();
                    TerminalRequest::Attach(
                        session.key,
                        PtySize {
                            rows: grid.rows,
                            cols: grid.cols,
                        },
                    )
                }
                SessionState::Stopping => TerminalRequest::End(session.key),
                _ => continue,
            };
            let (input, job) = client.clone().spawn_terminal(
                &self.rt,
                session.peer.clone(),
                action,
                self.updates.clone(),
                session.attachment,
            );
            session.input = input;
            session.job = Some(job);
        }
    }

    pub(super) fn dismiss_session(&mut self, id: u64, cx: &mut Context<Self>) {
        if self.sessions.remove(&id).is_some() && self.page == Page::Terminal(id) {
            self.navigate(self.terminal_return.clone(), cx);
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
                s.peer == peer && s.key == key && !matches!(s.state, SessionState::Ended(_))
            })
        {
            self.navigate(Page::Terminal(existing.attachment.session), cx);
            cx.notify();
            return;
        }
        let Some(client) = self.client.clone() else {
            return;
        };
        let Some(saved) = self.peers.get(&peer) else {
            return;
        };
        if !saved.supports_shell() || !self.presence.online(&peer) {
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
        self.sessions
            .retain(|_, s| s.peer != peer || s.key != key || !matches!(s.state, SessionState::Ended(_)));
        self.generation += 1;
        let id = self.generation;
        let font = self.font.clone().scaled(self.terminal_zoom);
        let grid = initial_size(window, font.cell, &self.keybar);
        let size = PtySize {
            cols: grid.cols,
            rows: grid.rows,
        };
        let action = if saved_key.is_some() {
            TerminalRequest::Attach(key, size)
        } else {
            TerminalRequest::Create(key, size)
        };
        let attachment = AttachmentId {
            session: id,
            generation: 0,
        };
        let (input, job) =
            client.spawn_terminal(&self.rt, peer.clone(), action, self.updates.clone(), attachment);
        let title = self.session_names.get(&peer, key).cloned().unwrap_or_default();
        self.sessions.insert(
            id,
            Session {
                attachment,
                key,
                peer,
                name,
                state: SessionState::Connecting,
                screen: Emulator::with_palette(grid, &Ui::new(window, cx).colors.terminal, font),
                bell: Default::default(),
                wheel: Default::default(),
                touch_mouse: Default::default(),
                composing: Preedit::default(),
                title: cx.new(|_| title),
                input,
                job: Some(job),
                retry_at: std::time::Instant::now(),
            },
        );
        self.assign_terminal_defaults(cx);
        self.navigate(Page::Terminal(id), cx);
        cx.notify();
    }

    pub(super) fn start_input(s: &Session) {
        let id = s.attachment.session;
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
        can_end: bool,
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
        if let Some(s) = self.sessions.values().find(|s| {
            s.peer == peer && s.key == key && !matches!(s.state, SessionState::Ended(_))
        }) {
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

    pub(super) fn sessions_subtitle(&self, cx: &App) -> Option<String> {
        let filters = &self.session_filters;
        let query = filters.query(cx);
        let mut total = 0;
        let mut shown = 0;
        let mut initial_request = false;
        for (peer, p) in &self.peers {
            let available = self.presence.online(peer) && p.supports_shell();
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
                    !matches!(s.state, SessionState::Ended(_)),
                    s.attachment.session,
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
        if (self.starting_network || initial_request) && total == 0 {
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
                let can_end = state != SessionLocation::Ended
                    && !local
                        .and_then(|id| self.sessions.get(&id))
                        .is_some_and(|s| matches!(s.state, SessionState::Stopping))
                    && !self.ending_hosted.contains(&(peer.clone(), key));
                let content = self.session_menu(&peer, key, title.clone(), can_end, cx);
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
                if self.starting_network && rows.is_empty() {
                    Some(paxri_ui::components::ListState::loading("Loading sessions…"))
                } else {
                    session_list_notice(total, rows.len(), initial_request, &failed)
                },
                |d, state| d.child(state.render(ui)),
            )
            .when(!rows.is_empty(), |page| {
                page.child(
                    ListView::scrollable("session-list")
                        .with_floating_action(has_action)
                        .children(rows),
                )
            })
            .when_some(launcher, |page, button| {
                page.child(paxri_ui::components::floating_corner().child(button))
            })
    }
}
