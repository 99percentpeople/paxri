use crate::{
    Dashboard,
    components::Ui,
    pages::DevicesPane,
};
use paxri_ui::components::{ListState, ListView};
use gpui::{prelude::*, *};
use paxri_control::{DevicePlatform, Status};
use paxri_ui::connections::presence_detail;

impl Dashboard {
    pub(super) fn paired_devices(&self, ui: Ui, status: &Status, cx: &Context<Self>) -> Div {
        let filters = &self.device_filters;
        let query = filters.query(cx);
        let card = div().min_h_0().flex_1().flex().flex_col().gap_2()
            .child(filters.controls(|v| &mut v.device_filters, cx));
        let now = paxri_control::unix_time_ms();
        let rows = status
            .peers_by_last_change()
            .into_iter()
            .filter_map(|(name, id)| {
                let platform = status.peer_platform(name);
                let state = status.peer_state(name);
                let display_name = status.peer_display_name(name);
                if !filters.matches(&query, display_name, name, id, platform, state) {
                    return None;
                }
                let mut detail = presence_detail(state, now, status.peer_last_status_change_ms.get(name).copied());
                if platform != DevicePlatform::Android && display_name != name {
                    detail.push_str(" · ");
                    detail.push_str(name);
                }
                let peer = SharedString::from(name.clone());
                let selected =
                    matches!(&self.devices_pane, Some(DevicesPane::Peer(p)) if p == name);
                Some(
                    ui.device_row(
                        SharedString::from(format!("device-{name}")),
                        platform,
                        SharedString::from(display_name.to_owned()),
                        status.peer_online(name),
                        detail,
                    )
                    .pressed(selected)
                    .on_click(cx.listener(move |v, _, _, cx| {
                        v.show_devices_pane(DevicesPane::Peer(peer.to_string()), cx)
                    })),
                )
            });
        let mut rows = rows.peekable();
        if rows.peek().is_none() {
            return card.child(ListState::filtered("No matching devices").render(*ui));
        }
        card.child(ListView::scrollable("paired-list").children(rows))
    }
}
