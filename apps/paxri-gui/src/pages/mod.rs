mod device;
mod device_list;
mod forward_filters;
pub(crate) use forward_filters::ForwardFilters;
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

use crate::{
    Dashboard,
    components::{ButtonVariant::Secondary, Ui, actions, heading, row},
    theme::SPACE_4,
};
use gpui::{prelude::*, *};
use paxri_cli::gateway::Action;
use paxri_control::Command;

fn short_id(id: &str) -> String {
    if id.len() > 20 && id.is_ascii() {
        format!("{}…{}", &id[..10], &id[id.len() - 8..])
    } else {
        id.into()
    }
}

fn peer_detail(platform: paxri_control::DevicePlatform, id: &str) -> String {
    format!("{} · {}", platform.label(), short_id(id))
}

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
    fn gateway_notice(&self, ui: Ui, cx: &mut Context<Self>) -> Div {
        ui.section()
            .child(
                row().child(heading("Local gateway")).child(
                    actions()
                        .when(self.error.is_some() && !self.saving, |r| {
                            r.child(
                                ui.button(Secondary, "start-gateway", "Start gateway")
                                    .on_click(
                                        cx.listener(|v, _, _, cx| v.manage(Action::Start, cx)),
                                    ),
                            )
                        })
                        .child(
                            ui.button(Secondary, "retry", "Refresh")
                                .on_click(cx.listener(|v, _, _, cx| v.send(Command::Status, cx))),
                        ),
                ),
            )
            .child(ui.muted(self.error.clone().unwrap_or_else(|| {
                self.address
                    .clone()
                    .unwrap_or_else(paxri_control::default_address)
                    .display()
                    .to_string()
            })))
    }
}
