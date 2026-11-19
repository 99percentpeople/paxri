mod home;
mod rules;
mod settings;

use crate::{
    Dashboard,
    components::{ButtonVariant::Secondary, Ui, heading, row},
};
use gpui::{prelude::*, *};
use tunnel_control::Command;

fn short_id(id: &str) -> String {
    if id.len() > 20 && id.is_ascii() {
        format!("{}…{}", &id[..10], &id[id.len() - 8..])
    } else {
        id.into()
    }
}

impl Dashboard {
    fn gateway_notice(&self, ui: Ui, cx: &mut Context<Self>) -> Div {
        ui.card()
            .child(
                row()
                    .child(heading(if self.error.is_some() {
                        "Gateway offline"
                    } else {
                        "Connecting to gateway…"
                    }))
                    .child(
                        ui.button(Secondary, "retry", "Refresh")
                            .on_click(cx.listener(|v, _, _, cx| v.send(Command::Status, cx))),
                    ),
            )
            .child(ui.muted(self.error.clone().unwrap_or_else(|| {
                self.address
                    .clone()
                    .unwrap_or_else(tunnel_control::default_address)
                    .display()
                    .to_string()
            })))
    }
}
