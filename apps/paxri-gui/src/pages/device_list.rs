use crate::{
    Dashboard,
    components::{ButtonVariant::Ghost, Icon, Select, Ui},
    pages::DevicesPane,
};
use gpui::{prelude::*, *};
use paxri_control::{DevicePlatform, Status};
use paxri_ui::connections::presence_detail;

impl Dashboard {
    fn system_filter(&self, cx: &Context<Self>) -> Select<Option<DevicePlatform>> {
        let mut select = Select::new(
            "peer-system-filter",
            Some(self.peer_system),
            cx.listener(|view, system: &Option<DevicePlatform>, _, cx| {
                view.peer_system = *system;
                cx.notify();
            }),
        )
        .option_with_icon(None, "All systems", Icon::Filter);
        for system in [
            DevicePlatform::Android,
            DevicePlatform::Windows,
            DevicePlatform::Linux,
            DevicePlatform::MacOs,
            DevicePlatform::Web,
            DevicePlatform::Unknown,
        ] {
            select = select.option_with_icon(Some(system), system.label(), Icon::device(system));
        }
        select
    }

    pub(super) fn paired_devices(&self, ui: Ui, status: &Status, cx: &Context<Self>) -> Div {
        if status.peers.is_empty() {
            return div().child(ui.muted("No paired devices yet."));
        }
        let query = self.peer_search.read(cx).text().to_lowercase();
        let terms: Vec<_> = query.split_whitespace().collect();
        let active = self.peer_system.is_some();
        let controls = div()
            .flex_none()
            .w_full()
            .flex()
            .items_center()
            .gap_1()
            .child(div().min_w_0().flex_1().child(self.peer_search.clone()))
            .child(
                ui.icon_button(
                    Ghost,
                    "system-filter-toggle",
                    Icon::Filter,
                    "Filter devices by system",
                )
                .pressed(self.peer_filter_open || active)
                .on_click(cx.listener(|v, _, _, cx| {
                    v.peer_filter_open = !v.peer_filter_open;
                    cx.notify();
                })),
            );
        let card = div()
            .min_h_0()
            .flex()
            .flex_col()
            .gap_2()
            .child(controls)
            .when(self.peer_filter_open || active || !query.is_empty(), |d| {
                d.child(
                    div()
                        .flex_none()
                        .flex()
                        .gap_1()
                        .items_center()
                        .when(self.peer_filter_open || active, |r| {
                            r.child(div().min_w_0().flex_1().child(self.system_filter(cx)))
                        })
                        .when(active || !query.is_empty(), |r| {
                            r.child(
                                ui.icon_button(
                                    Ghost,
                                    "clear-device-filters",
                                    Icon::FilterX,
                                    "Clear filters",
                                )
                                .on_click(cx.listener(
                                    |v, _, _, cx| {
                                        v.peer_system = None;
                                        v.peer_filter_open = false;
                                        v.peer_search
                                            .update(cx, |field, cx| field.set_text("", cx));
                                        cx.notify();
                                    },
                                )),
                            )
                        }),
                )
            });
        let now = paxri_control::unix_time_ms();
        let rows = status
            .peers_by_last_change()
            .into_iter()
            .filter_map(|(name, id)| {
                let platform = status.peer_platform(name);
                if self.peer_system.is_some_and(|system| system != platform) {
                    return None;
                }
                let state = status.peer_state(name);
                let display_name = status.peer_display_name(name);
                if !terms.is_empty() {
                    let searchable =
                        format!("{display_name} {name} {id} {} {}", platform.label(), state).to_lowercase();
                    if !terms.iter().all(|term| searchable.contains(*term)) {
                        return None;
                    }
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
                    ui.device_row(SharedString::from(format!("device-{name}")), platform, SharedString::from(display_name.to_owned()), detail.clone())
                    .tooltip(crate::components::tooltip(
                        format!("{display_name} · {} · {detail}", platform.label()).into(),
                    ))
                    .pressed(selected)
                    .on_click(cx.listener(move |v, _, _, cx| {
                        v.show_devices_pane(DevicesPane::Peer(peer.to_string()), cx)
                    })),
                )
            });
        let mut rows = rows.peekable();
        if rows.peek().is_none() {
            return card.child(ui.muted("No devices match your filters."));
        }
        card.child(
            div()
                .id("paired-list")
                .min_h_0()
                .overflow_y_scroll()
                .flex()
                .flex_col()
                .gap_1()
                .children(rows),
        )
    }
}
