use super::*;
use std::collections::BTreeMap;
use terminal::Terminal;

#[derive(Default)]
pub(crate) struct PeerSessions {
    last_number: usize,
    pub(crate) hosted: Vec<SessionKey>,
    open: BTreeMap<SessionKey, WeakEntity<Terminal>>,
}

impl Dashboard {
    fn assign_terminal_defaults(&self, cx: &mut App) {
        let mut titles = Vec::new();
        for (peer, sessions) in &self.terminal_sessions {
            for view in sessions.open.values().filter_map(WeakEntity::upgrade) {
                titles.push((peer.clone(), view.read(cx).title.clone()));
            }
        }
        terminal::TabTitle::assign_defaults(&titles, cx);
    }

    pub(crate) fn open_sessions(
        &self,
        peer: &SharedString,
        cx: &App,
    ) -> Vec<(SessionKey, SharedString)> {
        let Some(sessions) = self.terminal_sessions.get(peer) else {
            return Vec::new();
        };
        let mut rows: BTreeMap<_, _> = sessions
            .hosted
            .iter()
            .map(|&key| (key, peer.clone()))
            .collect();
        for (&key, view) in &sessions.open {
            if let Some(view) = view.upgrade() {
                rows.insert(key, view.read(cx).title.read(cx).label(peer));
            }
        }
        rows.into_iter().collect()
    }

    fn register_terminal(
        &mut self,
        peer: SharedString,
        key: SessionKey,
        view: &Entity<Terminal>,
        cx: &mut Context<Self>,
    ) {
        let sessions = self.terminal_sessions.entry(peer.clone()).or_default();
        sessions.last_number = sessions.last_number.max(key.number);
        sessions.open.insert(key, view.downgrade());
        self.assign_terminal_defaults(cx);
        let title = view.read(cx).title.clone();
        cx.observe(&title, |d, _, cx| {
            d.assign_terminal_defaults(cx);
            cx.notify();
        })
        .detach();
        let id = view.entity_id();
        // Closing a native window also releases its tabs.
        cx.observe_release(view, move |d, _, cx| {
            d.unregister_terminal(&peer, key, id, cx)
        })
        .detach();
        cx.notify();
    }

    fn unregister_terminal(
        &mut self,
        peer: &str,
        key: SessionKey,
        id: EntityId,
        cx: &mut Context<Self>,
    ) {
        if let Some(sessions) = self.terminal_sessions.get_mut(peer)
            && sessions
                .open
                .get(&key)
                .is_some_and(|view| view.entity_id() == id)
        {
            sessions.open.remove(&key);
            cx.notify();
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
        let view = Terminal::open(&peer, key, attach, &runtime, ipc.as_deref(), w, cx);
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
        let number = dashboard.terminal_sessions.get(&peer).map_or(0, |s| {
            s.hosted
                .iter()
                .map(|k| k.number)
                .max()
                .unwrap_or(0)
                .max(s.last_number)
        });
        let key = match SessionKey::new(number.saturating_add(1)) {
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
            (view.read(cx).is_running() || !s.hosted.contains(&key)).then_some(view)
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
                d.unregister_terminal(peer, *key, view.entity_id(), cx)
            });
        }
    }
}
