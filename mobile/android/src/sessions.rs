use super::*;
use paxri_ui::components::directory::{DeviceOption, session_entries, session_list_notice};

impl Mobile {
    pub(super) fn show_sessions(&mut self, peer: Option<String>, cx: &mut Context<Self>) {
        self.session_filters.for_device(peer.clone(), cx);
        self.navigate(Page::Sessions(peer), cx);
        cx.notify();
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
                    self.listing_peers.contains(peer) && !self.hosted.contains_key(peer);
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
                    s.id,
                )
            });
            let entries = session_entries(name.clone().into(), hosted, local);
            total += entries.len();
            for (key, (title, state, local)) in entries {
                if !filters.matches(&query, peer, name, &title, key.number, state) {
                    continue;
                }
                let peer = peer.clone();
                rows.push(
                    ui.session_row(&peer, name, key.number, key.nonce, title, state)
                        .on_click(cx.listener(move |v, _, w, cx| {
                            if let Some(id) = local {
                                v.navigate(Page::Terminal(id), cx);
                                cx.notify();
                            } else {
                                v.open(peer.clone(), Some(key), w, cx);
                            }
                        })),
                );
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
            .child(filters.controls(&devices, |v| &mut v.session_filters, cx).with_state(&self.filter_menu))
            .when_some(
                session_list_notice(total, rows.len(), initial_request, &failed),
                |d, message| d.child(ui.muted(message)),
            )
            .child(
                div()
                    .id("session-list")
                    .min_h_0()
                    .flex_1()
                    .overflow_y_scroll()
                    .restrict_scroll_to_axis()
                    .flex()
                    .flex_col()
                    .pb_3()
                    .when(has_action, |list| {
                        list.pb(paxri_ui::components::FLOATING_ACTION_CLEARANCE)
                    })
                    .children(rows),
            )
            .when_some(launcher, |page, button| {
                page.child(paxri_ui::components::floating_corner().child(button))
            })
    }
}
