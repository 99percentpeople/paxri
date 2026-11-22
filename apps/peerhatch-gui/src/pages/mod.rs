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
mod settings_catalog;
mod settings_editor;
pub(crate) use settings_editor::SettingsEditor;
mod terminal_settings;

use crate::{
    Dashboard,
    components::{ButtonVariant::Secondary, Icon, Ui, actions, heading, row, single_line, tooltip},
    theme::{SPACE_2, SPACE_3, SPACE_4},
};
use gpui::{prelude::*, *};
use peerhatch_cli::gateway::Action;
use peerhatch_control::Command;
use peerhatch_control::DevicePlatform;

fn short_id(id: &str) -> String {
    if id.len() > 20 && id.is_ascii() {
        format!("{}…{}", &id[..10], &id[id.len() - 8..])
    } else {
        id.into()
    }
}

fn peer_detail(platform: peerhatch_control::DevicePlatform, id: &str) -> String {
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

fn device_heading(
    ui: Ui,
    badge: Div,
    name: SharedString,
    platform: DevicePlatform,
    id: Option<(&str, String)>,
) -> Div {
    let detail = match id {
        Some((id, display_id)) => {
            single_line(ui.muted(format!("{} · {display_id}", platform.label())))
                .w_full()
                .id("device-identity")
                .tooltip(tooltip(id.to_owned().into()))
                .into_any_element()
        }
        None => ui.muted(platform.label()).into_any_element(),
    };
    div()
        .min_w_0()
        .flex()
        .items_center()
        .gap(SPACE_3)
        .child(ui.icon_tile(Icon::device(platform), 48., 25.))
        .child(
            div()
                .min_w_0()
                .flex_1()
                .flex()
                .flex_col()
                .child(
                    div()
                        .min_w_0()
                        .flex()
                        .items_center()
                        .gap(SPACE_2)
                        .child(
                            single_line(div())
                                .text_2xl()
                                .font_weight(FontWeight::SEMIBOLD)
                                .id("device-heading")
                                .tooltip(tooltip(name.clone()))
                                .child(name),
                        )
                        .child(badge),
                )
                .child(detail),
        )
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
                    .unwrap_or_else(peerhatch_control::default_address)
                    .display()
                    .to_string()
            })))
    }
}
