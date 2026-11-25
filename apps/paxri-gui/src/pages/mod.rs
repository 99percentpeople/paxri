mod device;
mod device_list;
mod sessions;
pub(crate) use paxri_ui::components::forwarding::ForwardFilters;
mod home;
pub(crate) use home::DevicesPane;
mod network_settings;
mod port_actions;
mod rules;
mod settings;
use paxri_ui::settings::catalog as settings_catalog;
mod settings_editor;
pub(crate) use settings_editor::SettingsEditor;
mod terminal_settings;

use crate::{Dashboard, GatewayOffline, components::{Icon, Ui}, theme::SPACE_4};
use gpui::{prelude::*, *};
use paxri_cli::gateway::Action;
use paxri_ui::components::ListState;

fn overview() -> Div {
    div()
        .w_full()
        .max_w(px(1040.))
        .mx_auto()
        .min_w_0()
        .flex()
        .flex_col()
        .gap(SPACE_4)
}


impl Dashboard {
    fn gateway_transition(&self) -> Option<&'static str> {
        match self.gateway_action {
            Some(Action::Start) => Some("Starting gateway…"),
            Some(Action::Stop) => Some("Stopping gateway…"),
            Some(Action::Restart) => Some("Restarting gateway…"),
            _ => None,
        }
    }

    fn gateway_notice(&self, transition: Option<&'static str>, ui: Ui, cx: &Context<Self>) -> (Div, Option<AnyElement>) {
        let state = match (transition, &self.offline) {
            (Some(label), _) => ListState::loading(label),
            (None, None) => ListState::loading("Connecting to gateway…"),
            (None, Some(GatewayOffline::Stopped)) => ListState::empty(Icon::Server, "Gateway stopped"),
            (None, Some(GatewayOffline::Failed(error))) => ListState::empty(Icon::Server, error.clone()),
        };
        let start = (self.offline.is_some() && !self.saving).then(|| {
            ui.floating_action("start-gateway", Icon::Play, "Start gateway")
                .on_click(cx.listener(|v, _, _, cx| v.manage(Action::Start, cx)))
                .into_any_element()
        });
        (state.render(ui), start)
    }
}
