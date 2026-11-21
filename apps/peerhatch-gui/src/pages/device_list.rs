use crate::{
    Dashboard, Workspace,
    components::{ButtonVariant::Ghost, Icon, Select, Ui, heading, row},
    theme::{BUTTON_HEIGHT, SPACE_2},
};
use gpui::{prelude::*, *};
use peerhatch_control::{DevicePlatform, Status};

fn change_age(now: i64, changed: i64) -> String {
    let minutes = now.saturating_sub(changed).max(0) / 60_000;
    match minutes {
        0 => "just now".into(),
        1..60 => format!("{minutes}m ago"),
        60..1440 => format!("{}h ago", minutes / 60),
        _ => format!("{}d ago", minutes / 1440),
    }
}

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

    pub(super) fn paired_devices(
        &self,
        ui: Ui,
        status: &Status,
        workspace: WeakEntity<Workspace>,
        cx: &Context<Self>,
    ) -> Div {
        if status.peers.is_empty() {
            return ui
                .section("Paired devices")
                .child(ui.separator())
                .child(ui.muted("No paired devices yet."));
        }
        let query = self.peer_search.read(cx).text().to_lowercase();
        let terms: Vec<_> = query.split_whitespace().collect();
        let header = row()
            .min_h(BUTTON_HEIGHT)
            .child(heading("Paired devices"))
            .when(self.peer_system.is_some() || !query.is_empty(), |header| {
                header.child(
                    ui.icon_button(
                        Ghost,
                        "clear-device-filters",
                        Icon::FilterX,
                        "Clear filters",
                    )
                    .on_click(cx.listener(|view, _, _, cx| {
                        view.peer_system = None;
                        view.peer_search
                            .update(cx, |field, cx| field.set_text("", cx));
                        cx.notify();
                    })),
                )
            });
        let controls = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(SPACE_2)
            .child(
                div()
                    .flex_grow()
                    .min_w_0()
                    .flex_basis(px(148.))
                    .child(self.peer_search.clone()),
            )
            .child(div().w(px(142.)).max_w_full().child(self.system_filter(cx)));
        let card = ui
            .card()
            .child(header)
            .child(controls)
            .child(ui.separator());

        let now = peerhatch_control::unix_time_ms();
        let rows = status
            .peers_by_last_change()
            .into_iter()
            .filter_map(|(name, id)| {
                let platform = status.peer_platform(name);
                if self.peer_system.is_some_and(|system| system != platform) {
                    return None;
                }
                let state = status.peer_state(name);
                if !terms.is_empty() {
                    let searchable =
                        format!("{name} {id} {} {}", platform.label(), state).to_lowercase();
                    if !terms.iter().all(|term| searchable.contains(*term)) {
                        return None;
                    }
                }
                let mut detail = platform.label().to_owned();
                detail.push_str(" · ");
                detail.push_str(state);
                if let Some(&changed) = status.peer_last_status_change_ms.get(name) {
                    detail.push_str(" · ");
                    detail.push_str(&change_age(now, changed));
                }
                let peer = SharedString::from(name.clone());
                let workspace = workspace.clone();
                Some(
                    ui.navigation_row(
                        SharedString::from(format!("device-{name}")),
                        ui.device_icon(platform),
                        peer.clone(),
                        detail,
                    )
                    .on_click(move |_, w, cx| {
                        let _ = workspace.update(cx, |v, cx| v.open_device(&peer, w, cx));
                    }),
                )
            });
        let mut rows = rows.peekable();
        if rows.peek().is_none() {
            return card.child(ui.muted("No devices match your filters."));
        }
        card.child(div().flex().flex_col().children(rows))
    }
}
