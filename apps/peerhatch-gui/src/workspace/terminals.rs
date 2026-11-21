use super::*;
use std::collections::BTreeMap;
use terminal::Terminal;

#[derive(Default)]
pub(crate) struct PeerSessions {
    last_number: usize,
    // The tabs own the PTYs. Listing a session must not keep it running.
    open: BTreeMap<usize, WeakEntity<Terminal>>,
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

    pub(crate) fn open_sessions<'a>(
        &'a self,
        peer: &'a SharedString,
        cx: &'a App,
    ) -> impl Iterator<Item = (usize, SharedString)> + 'a {
        self.terminal_sessions
            .get(peer)
            .into_iter()
            .flat_map(|s| &s.open)
            .filter_map(move |(&number, view)| {
                let view = view.upgrade()?;
                Some((number, view.read(cx).title.read(cx).label(peer)))
            })
    }

    fn register_terminal(
        &mut self,
        peer: SharedString,
        previous_number: Option<usize>,
        view: &Entity<Terminal>,
        cx: &mut Context<Self>,
    ) -> usize {
        let sessions = self.terminal_sessions.entry(peer.clone()).or_default();
        let number = previous_number.unwrap_or_else(|| {
            sessions.last_number += 1;
            sessions.last_number
        });
        sessions.open.insert(number, view.downgrade());
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
            d.unregister_terminal(&peer, number, id, cx)
        })
        .detach();
        cx.notify();
        number
    }

    fn unregister_terminal(
        &mut self,
        peer: &str,
        number: usize,
        id: EntityId,
        cx: &mut Context<Self>,
    ) {
        if let Some(sessions) = self.terminal_sessions.get_mut(peer)
            && sessions
                .open
                .get(&number)
                .is_some_and(|view| view.entity_id() == id)
        {
            sessions.open.remove(&number);
            cx.notify();
        }
    }
}

impl Workspace {
    fn terminal_tab(
        &self,
        peer: SharedString,
        previous: Option<(usize, &Entity<terminal::TabTitle>)>,
        w: &Window,
        cx: &mut Context<Self>,
    ) -> Page {
        let dashboard = self.dashboard.read(cx);
        let runtime = dashboard.runtime.clone();
        let ipc = dashboard.active_address();
        let title = previous
            .map(|(_, title)| title.read(cx).for_reconnect())
            .unwrap_or_default();
        let view = Terminal::open(
            &peer,
            title,
            &self.dashboard,
            &runtime,
            ipc.as_deref(),
            w,
            cx,
        );
        App::subscribe(cx, &view, Self::terminal_exited).detach();
        let number = self.dashboard.update(cx, |d, cx| {
            d.register_terminal(peer.clone(), previous.map(|(number, _)| number), &view, cx)
        });
        Page::terminal(peer, number, view, cx)
    }

    pub(crate) fn new_terminal(
        &mut self,
        peer: SharedString,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.dashboard.read(cx).peer_online(&peer) {
            return;
        }
        let page = self.terminal_tab(peer, None, w, cx);
        self.append_tab(page, w, cx);
    }

    pub(crate) fn show_terminal(
        &mut self,
        peer: SharedString,
        number: usize,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(view) = self
            .dashboard
            .read(cx)
            .terminal_sessions
            .get(&peer)
            .and_then(|s| s.open.get(&number))
            .and_then(WeakEntity::upgrade)
        {
            self.focus_open_page(&Page::terminal(peer, number, view, cx), w, cx);
        }
    }

    pub(super) fn reconnect_terminal(
        &mut self,
        _: &ReconnectTerminal,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.end_rename(EndRename::Blur, cx);
        let Page::Terminal {
            peer,
            number,
            view,
            title,
        } = &self.active
        else {
            return;
        };
        if view.read(cx).is_running() || !self.dashboard.read(cx).peer_online(peer) {
            return;
        }
        if let Some(index) = self.tabs.iter().position(|p| p == &self.active) {
            let page = self.terminal_tab(peer.clone(), Some((*number, title)), w, cx);
            self.dock
                .update(cx, |dock, _| dock.forget(&self.tabs[index]));
            self.tabs[index] = page.clone();
            self.focus_tab(page, w, cx);
        }
    }

    pub(super) fn forget_terminal(&self, page: &Page, cx: &mut Context<Self>) {
        if let Page::Terminal {
            peer, number, view, ..
        } = page
        {
            self.dashboard.update(cx, |d, cx| {
                d.unregister_terminal(peer, *number, view.entity_id(), cx)
            });
        }
    }
}
