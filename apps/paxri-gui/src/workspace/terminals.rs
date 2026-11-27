use super::*;
use std::collections::{BTreeMap, BTreeSet};
use terminal::Terminal;
use paxri_ui::{
    components::directory::{SessionLocation, named_session_entries},
    terminal::title::next_session_number,
};

#[derive(Default)]
pub(crate) struct PeerSessions {
    pub(crate) title_scope: String,
    last_number: usize,
    pub(crate) hosted: Vec<SessionKey>,
    pub(crate) listing: bool,
    pub(crate) list_failed: bool,
    pub(crate) listed: bool,
    pub(crate) list_revision: u64,
    ending: BTreeSet<SessionKey>,
    open: BTreeMap<SessionKey, WeakEntity<Terminal>>,
}

impl PeerSessions {
    pub(crate) fn new(gateway: &str, peer_id: &str) -> Self {
        Self { title_scope: format!("{gateway}/{peer_id}"), ..Self::default() }
    }

    pub(crate) fn has_live_view(&self, key: SessionKey, cx: &App) -> bool {
        self.open.get(&key).and_then(WeakEntity::upgrade).is_some_and(|v| v.read(cx).is_running())
    }

    fn remove_hosted(&mut self, key: SessionKey) {
        self.hosted.retain(|k| *k != key);
        self.list_revision += 1;
    }
}

impl Dashboard {
    pub(crate) fn sync_peer_connections(&self, status: &paxri_control::Status, cx: &mut App) {
        use paxri_ui::terminal::lifecycle::PeerConnectionState;
        for (peer, sessions) in &self.terminal_sessions {
            let phase = status.peer_phase(peer);
            let conn_id = status
                .peer_connection_id(peer)
                .or_else(|| status.peer_online(peer).then_some(1));
            let state = PeerConnectionState::from_phase(phase, conn_id);
            for weak in sessions.open.values() {
                if let Some(view) = weak.upgrade() {
                    view.update(cx, |terminal, cx| terminal.on_connection_changed(state, cx));
                }
            }
        }
    }

    pub(crate) fn end_listed_session(
        &mut self,
        peer: &SharedString,
        key: SessionKey,
        w: &Window,
        cx: &mut Context<Self>,
    ) {
        let path = self
            .active_address()
            .unwrap_or_else(paxri_control::default_address);
        let Some(status) = &self.status else { return };
        let Some(id) = status.peers.get(peer.as_ref()).cloned() else {
            return;
        };
        let Some(sessions) = self.terminal_sessions.get_mut(peer) else {
            return;
        };
        if let Some(view) = sessions.open.get(&key).and_then(WeakEntity::upgrade)
            && view.read(cx).is_running()
        {
            view.update(cx, |terminal, cx| terminal.end(cx));
            return;
        }
        if !sessions.hosted.contains(&key) || !sessions.ending.insert(key) {
            return;
        }
        let gateway = status.device_id.clone();
        let ipc = status.ipc_path.clone();
        let peer = peer.clone();
        let window = w.window_handle();
        let job = self.runtime.spawn({
            let id = id.clone();
            async move {
                paxri_control::managed_terminal(
                    &path,
                    id,
                    paxri_control::terminal::managed::Request::End(key),
                )
                .await
                .map(|_| ())
            }
        });
        cx.spawn(async move |this, cx| {
            let result = job.await.map_err(anyhow::Error::from).and_then(|r| r);
            let _ = this.update(cx, |view, cx| {
                if !view.status.as_ref().is_some_and(|s| {
                    s.device_id == gateway
                        && s.ipc_path == ipc
                        && s.peers.get(peer.as_ref()) == Some(&id)
                }) {
                    return;
                }
                if let Some(sessions) = view.terminal_sessions.get_mut(&peer) {
                    sessions.ending.remove(&key);
                    match result {
                        Ok(()) => {
                            sessions.remove_hosted(key);
                            if let Err(e) = view.session_titles.retain(&sessions.title_scope, |k| k != key) {
                                eprintln!("Removing session title: {e}");
                            }
                        }
                        Err(error) => crate::components::Toast::show(
                            format!("Couldn’t end session: {error}"),
                            window,
                            cx,
                        ),
                    }
                }
                view.refresh_hosted_sessions(false, cx);
                cx.notify();
            });
        })
        .detach();
    }

    fn assign_terminal_defaults(&mut self, cx: &mut App) {
        let mut titles = Vec::new();
        let mut reserved = Vec::new();
        for (peer, sessions) in &self.terminal_sessions {
            let name: SharedString = self
                .status
                .as_ref()
                .map_or(peer.as_ref(), |s| s.peer_display_name(peer))
                .to_owned()
                .into();
            reserved.extend(self.session_titles.reserved_labels(
                &sessions.title_scope,
                &name,
                |key| !sessions.open.get(&key).is_some_and(|v| v.upgrade().is_some()),
            ));
            for view in sessions.open.values().filter_map(WeakEntity::upgrade) {
                titles.push((name.clone(), view.read(cx).title.clone()));
            }
        }
        terminal::TabTitle::assign_with_reserved(&titles, reserved, cx);
        for sessions in self.terminal_sessions.values() {
            for (key, weak) in &sessions.open {
                if let Some(view) = weak.upgrade() {
                    let view = view.read(cx);
                    if view.is_confirmed_on_host()
                        && let Err(e) = self.session_titles.set(
                            &sessions.title_scope,
                            *key,
                            view.title.read(cx),
                        )
                    {
                        eprintln!("Saving session title: {e}");
                    }
                }
            }
        }
    }

    pub(crate) fn open_sessions(
        &self,
        peer: &SharedString,
        cx: &App,
    ) -> Vec<(SessionKey, SharedString, SessionLocation)> {
        let Some(sessions) = self.terminal_sessions.get(peer) else {
            return Vec::new();
        };
        let hosted = sessions.hosted.iter().filter(|_| self.can_open_shell(peer)).copied();
        let name: SharedString = self.status.as_ref().map_or(peer.as_ref(), |s| s.peer_display_name(peer)).to_owned().into();
        let local = sessions.open.iter().filter_map(|(&key, weak)| {
            let view = weak.upgrade()?;
            let view = view.read(cx);
            Some((key, view.title.read(cx).label(&name), view.is_running(), ()))
        });
        named_session_entries(
            name.clone(),
            &sessions.title_scope,
            &self.session_titles,
            hosted,
            local,
        )
            .into_iter()
            .map(|(key, (title, state, _))| (key, title, state))
            .collect()
    }

    fn register_terminal(
        &mut self,
        peer: SharedString,
        key: SessionKey,
        view: &Entity<Terminal>,
        cx: &mut Context<Self>,
    ) {
        let Some(status) = &self.status else { return };
        let Some(peer_id) = status.peers.get(peer.as_ref()) else { return };
        let sessions = self.terminal_sessions.entry(peer.clone()).or_insert_with(|| PeerSessions::new(&status.device_id, peer_id));
        let title = view.read(cx).title.clone();
        let bell = view.read(cx).bell.clone();
        if let Some(saved) = self.session_titles.get(&sessions.title_scope, key) {
            title.update(cx, |title, _| title.clone_from(saved));
        }
        sessions.last_number = sessions.last_number.max(key.number);
        sessions.list_revision += 1;
        sessions.open.insert(key, view.downgrade());
        self.assign_terminal_defaults(cx);
        cx.observe(&title, |d, _, cx| {
            d.assign_terminal_defaults(cx);
            cx.notify();
        })
        .detach();
        cx.observe(&bell, |_, _, cx| cx.notify()).detach();
        let id = view.entity_id();
        // Closing a native window also releases its tabs.
        cx.observe_release(view, move |d, terminal, cx| {
            d.unregister_terminal(&peer, key, id, terminal, cx);
            cx.notify();
        })
        .detach();
        cx.notify();
    }

    fn unregister_terminal(
        &mut self,
        peer: &str,
        key: SessionKey,
        id: EntityId,
        terminal: &Terminal,
        cx: &App,
    ) {
        if let Some(sessions) = self.terminal_sessions.get_mut(peer)
            && sessions
                .open
                .get(&key)
                .is_some_and(|view| view.entity_id() == id)
        {
            if terminal.has_exited() {
                sessions.remove_hosted(key);
                if let Err(e) = self.session_titles.retain(&sessions.title_scope, |k| k != key) {
                    eprintln!("Removing session title: {e}");
                }
            } else if terminal.is_confirmed_on_host() {
                if let Err(e) =
                    self.session_titles
                        .set(&sessions.title_scope, key, terminal.title.read(cx))
                {
                    eprintln!("Saving session title: {e}");
                }
                if !sessions.hosted.contains(&key) {
                    sessions.hosted.push(key);
                }
            }
            sessions.open.remove(&key);
        }
    }
}

impl Workspace {
    fn terminal_tab(
        &self,
        peer: SharedString,
        key: SessionKey,
        attach: bool,
        w: &Window,
        cx: &mut Context<Self>,
    ) -> Page {
        let dashboard = self.dashboard.read(cx);
        let runtime = dashboard.runtime.clone();
        let ipc = dashboard.active_address();
        let connection_id = dashboard
            .status
            .as_ref()
            .and_then(|s| s.peer_connection_id(&peer))
            .unwrap_or(1);
        let view = Terminal::open(
            &peer,
            key,
            attach,
            connection_id,
            &runtime,
            ipc.as_deref(),
            w,
            cx,
        );
        App::subscribe(cx, &view, Self::terminal_exited).detach();
        self.dashboard.update(cx, |d, cx| {
            d.register_terminal(peer.clone(), key, &view, cx)
        });
        Page::terminal(peer, key, view, cx)
    }

    pub(crate) fn new_terminal(
        &mut self,
        peer: SharedString,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.dashboard.read(cx).can_open_shell(&peer) {
            return;
        }
        let dashboard = self.dashboard.read(cx);
        let number = dashboard
            .terminal_sessions
            .get(&peer)
            .map_or(1, |s| next_session_number(s.hosted.iter().copied(), s.last_number));
        let key = match SessionKey::new(number) {
            Ok(key) => key,
            Err(e) => {
                crate::components::Toast::show(e.to_string(), w.window_handle(), cx);
                return;
            }
        };
        let page = self.terminal_tab(peer, key, false, w, cx);
        self.append_tab(page, w, cx);
    }

    pub(crate) fn show_terminal(
        &mut self,
        peer: SharedString,
        key: SessionKey,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let sessions = self.dashboard.read(cx).terminal_sessions.get(&peer);
        if let Some(view) = sessions.and_then(|s| {
            let view = s.open.get(&key)?.upgrade()?;
            (view.read(cx).is_running() || !s.hosted.contains(&key) || !self.dashboard.read(cx).can_open_shell(&peer)).then_some(view)
        }) {
            self.focus_open_page(&Page::terminal(peer, key, view, cx), w, cx);
        } else if self.dashboard.read(cx).can_open_shell(&peer) {
            let page = self.terminal_tab(peer, key, true, w, cx);
            self.append_tab(page, w, cx);
        }
    }

    pub(super) fn forget_terminal(&self, page: &Page, cx: &mut Context<Self>) {
        if let Page::Terminal {
            peer, key, view, ..
        } = page
        {
            self.dashboard.update(cx, |d, cx| {
                d.unregister_terminal(peer, *key, view.entity_id(), view.read(cx), cx);
                cx.notify();
            });
        }
    }
}
