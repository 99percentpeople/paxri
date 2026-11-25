use super::{DevicesPane, overview};
use crate::{
    Dashboard, Workspace,
    components::{Ui, heading},
};
use gpui::{prelude::*, *};
use paxri_control::{Status, terminal::managed::Request};
use paxri_ui::components::directory::{DeviceOption, SessionLocation, session_list_notice};
use paxri_ui::components::{DropdownContent, ListView};
use std::time::Duration;

impl Dashboard {
    pub(crate) fn show_sessions(&mut self, peer: Option<String>, cx: &mut Context<Self>) {
        self.session_filters.for_device(peer, cx);
        self.show_devices_pane(DevicesPane::Sessions, cx);
        self.refresh_hosted_sessions(true, cx);
    }

    pub(crate) fn refresh_hosted_sessions(&mut self, include_sidebar: bool, cx: &mut Context<Self>) {
        let all = include_sidebar || self.devices_pane == Some(DevicesPane::Sessions);
        let Some(status) = &self.status else { return };
        let path = self
            .active_address()
            .unwrap_or_else(paxri_control::default_address);
        for (peer, id) in &status.peers {
            if !all
                && !matches!(&self.devices_pane, Some(DevicesPane::Peer(selected)) if selected == peer)
            {
                continue;
            }
            if !status.peer_online(peer)
                || !status
                    .peer_capabilities
                    .get(peer)
                    .is_some_and(|c| c.managed_terminals)
            {
                continue;
            }
            let sessions = self
                .terminal_sessions
                .entry(peer.clone().into())
                .or_insert_with(|| crate::workspace::PeerSessions::new(&status.device_id, id));
            if sessions.listing {
                continue;
            }
            sessions.listing = true;
            let revision = sessions.list_revision;
            let gateway = status.device_id.clone();
            let ipc = status.ipc_path.clone();
            let id = id.clone();
            let peer = peer.clone();
            let job = self.runtime.spawn({
                let peer = peer.clone();
                let path = path.clone();
                async move {
                    tokio::time::timeout(
                        Duration::from_secs(2),
                        paxri_control::managed_terminal(&path, peer, Request::List),
                    )
                    .await
                    .ok()?
                    .ok()
                    .map(|(_, keys)| keys)
                }
            });
            cx.spawn(async move |this, cx| {
                let result = job.await.ok().flatten();
                let _ = this.update(cx, |view, cx| {
                    if !view.status.as_ref().is_some_and(|s| {
                        s.device_id == gateway
                            && s.ipc_path == ipc
                            && s.peers.get(&peer) == Some(&id)
                    }) {
                        return;
                    }
                    if let Some(sessions) = view.terminal_sessions.get_mut(peer.as_str()) {
                        sessions.listing = false;
                        if sessions.list_revision != revision {
                            view.refresh_hosted_sessions(false, cx);
                            return;
                        }
                        sessions.list_failed = result.is_none();
                        sessions.listed = true;
                        if let Some(keys) = result {
                            if let Err(e) = view.session_titles.retain(&sessions.title_scope, |key| keys.contains(&key) || sessions.has_live_view(key, cx)) {
                                eprintln!("Cleaning up session titles: {e}");
                            }
                            sessions.hosted = keys;
                        }
                        cx.notify();
                    }
                });
            })
            .detach();
        }
    }

    pub(super) fn sessions_page(
        &self,
        status: &Status,
        ui: Ui,
        cx: &mut Context<Self>,
    ) -> (Div, Option<AnyElement>) {
        let filters = &self.session_filters;
        let query = filters.query(cx);
        let devices: Vec<DeviceOption> = status
            .peers
            .keys()
            .map(|name| {
                (
                    name.clone(),
                    status.peer_display_name(name).to_owned(),
                    status.peer_platform(name),
                )
            })
            .collect();
        let targets = devices
            .iter()
            .filter(|(id, _, _)| {
                status.can_open_shell(id) && filters.scope.device.as_ref().is_none_or(|v| v == id)
            })
            .cloned()
            .collect();
        let launcher = ui.session_launcher(targets, |peer, window, cx| {
            if let Some(workspace) = window.root::<Workspace>().flatten() {
                workspace.update(cx, |v, cx| v.new_terminal(peer.clone().into(), window, cx));
            }
        });
        let mut rows = Vec::new();
        let mut failed = Vec::new();
        let mut total = 0;
        let mut initial_request = false;
        for (alias, name, _) in &devices {
            let peer = SharedString::from(alias.clone());
            if status.peer_online(alias)
                && filters.scope.device.as_ref().is_none_or(|v| v == alias)
                && let Some(s) = self.terminal_sessions.get(&peer)
            {
                if s.list_failed {
                    failed.push(name.as_str());
                }
                initial_request |= s.listing && !s.listed;
            }
            for (key, title, state) in self.open_sessions(&peer, cx) {
                total += 1;
                if !filters.matches(&query, alias, name, &title, key.number, state) {
                    continue;
                }
                let peer = peer.clone();
                let actions = (state != SessionLocation::Ended).then(|| {
                    let id = SharedString::from(format!(
                        "session-actions-{alias}-{}-{}",
                        key.number, key.nonce
                    ));
                    let view = cx.weak_entity();
                    let peer = peer.clone();
                    let content = DropdownContent::new().end_session(move |w, cx| {
                        let _ = view.update(cx, |v, cx| v.end_listed_session(&peer, key, w, cx));
                    });
                    ui.actions_menu(id, format!("Actions for {title}").into(), content)
                });
                rows.push(ui.session_row(
                    alias,
                    name,
                    key,
                    title,
                    state,
                    actions,
                    move |_, w, cx| {
                        if let Some(ws) = w.root::<Workspace>().flatten() {
                            ws.update(cx, |v, cx| v.show_terminal(peer.clone(), key, w, cx));
                        }
                    },
                ));
            }
        }
        let section = ui
            .section()
            .child(heading("Sessions"))
            .child(
                filters
                    .controls(&devices, |v| &mut v.session_filters, cx)
                    .results(rows.len(), total),
            )
            .when(!rows.is_empty() && !failed.is_empty(), |section| {
                section.child(ui.muted(format!("Couldn’t refresh sessions on {}.", failed.join(", "))))
            })
            .when_some(
                session_list_notice(total, rows.len(), initial_request, &failed),
                |d, state| d.child(state.render(ui)),
            )
            .when(!rows.is_empty(), |page| page.child(ListView::new().children(rows)));
        (overview().child(section), launcher)
    }
}
