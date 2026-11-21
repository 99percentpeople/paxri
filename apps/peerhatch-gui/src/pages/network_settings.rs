use crate::{
    Dashboard, Input,
    components::{ButtonVariant::Secondary, ListItem, Ui},
    theme,
};
use gpui::{prelude::*, *};
use peerhatch_cli::gateway::Action;
use peerhatch_config::NativeChange;
use peerhatch_control::Command;

impl Dashboard {
    pub(super) fn network_settings(&self, ui: Ui, cx: &Context<Self>) -> Div {
        let mut section = ui.section("Network");
        let legacy = self.status.as_ref().filter(|s| s.native.is_none());
        if let Some(status) = legacy {
            section = section.child(
                ui.muted("This gateway must be updated before network settings can be changed."),
            );
            section = if status.user_managed {
                section.child(
                    ui.button(
                        Secondary,
                        "upgrade-network-gateway",
                        "Restart with this version",
                    )
                    .on_click(cx.listener(|v, _, _, cx| v.manage(Action::Restart, cx))),
                )
            } else {
                section.child(ui.muted("Update it through the service manager."))
            };
        }
        let settings = self
            .status
            .as_ref()
            .and_then(|s| s.native.as_ref())
            .or(self.profile_native.as_ref());
        let Some(settings) = settings else {
            return section.child(
                ui.muted("Network settings appear when a node configuration is available."),
            );
        };

        for (id, title, hint, enabled, change) in [
            (
                "allow-upnp",
                "Allow UPnP",
                "Allow UPnP router discovery and UDP port requests. PCP and NAT-PMP remain available.",
                settings.upnp,
                NativeChange::Upnp(!settings.upnp),
            ),
            (
                "lan-mapping",
                "Physical LAN mapping",
                "Try the physical IPv4 router using NAT-PMP when a VPN controls the default route.",
                settings.lan_port_mapping,
                NativeChange::LanPortMapping(!settings.lan_port_mapping),
            ),
        ] {
            let control = if legacy.is_some() {
                ui.muted(if enabled { "On" } else { "Off" })
                    .into_any_element()
            } else {
                ui.switch(id, enabled)
                    .on_click(cx.listener(move |v, _, _, cx| {
                        if !v.saving {
                            v.send(
                                Command::SetNative {
                                    change: change.clone(),
                                },
                                cx,
                            );
                        }
                    }))
                    .into_any_element()
            };
            section = section.child(ui.separator()).child(
                crate::components::row()
                    .child(
                        div()
                            .min_w_0()
                            .flex_grow()
                            .flex_basis(px(190.))
                            .child(title)
                            .child(ui.muted(hint).text_xs()),
                    )
                    .child(control),
            );
        }

        section = section.child(ui.separator());
        if legacy.is_none()
            && let Some(field) = &self.relay_field
        {
            section = section
                .child(ui.labeled("Relay URL", field.clone()))
                .child(
                    ui.muted("Leave blank to select a public relay automatically.")
                        .text_xs(),
                )
                .child(ui.form_actions(
                    "relay",
                    "Save relay",
                    cx.listener(|v, _, _, cx| {
                        if !v.saving
                            && let Some(field) = &v.relay_field
                        {
                            let value = field.read(cx).text().trim();
                            v.send(
                                Command::SetNative {
                                    change: NativeChange::RelayUrl(
                                        (!value.is_empty()).then(|| value.to_owned()),
                                    ),
                                },
                                cx,
                            );
                        }
                    }),
                    cx.listener(|v, _, _, cx| {
                        v.relay_field = None;
                        v.action_error = None;
                        cx.notify();
                    }),
                ));
        } else {
            let value = settings
                .relay_url
                .as_ref()
                .map(ToString::to_string)
                .unwrap_or_default();
            let mut item = ListItem::new("Relay").detail(ui.muted(if value.is_empty() {
                "Automatic public relay".to_owned()
            } else {
                value.clone()
            }));
            if legacy.is_none() {
                item = item.trailing(ui.button(Secondary, "edit-relay", "Edit").on_click(
                    cx.listener(move |v, _, w, cx| {
                        let field = Input::new(&value, "https://relay.example.org", cx);
                        cx.focus_view(&field, w);
                        v.relay_field = Some(field);
                        cx.notify();
                    }),
                ));
            }
            section = section.child(item);
        }
        if let Some(status) = &self.status
            && status.network_restart_required
        {
            let message = if status.user_managed {
                "Restart the gateway to apply saved changes."
            } else {
                "Restart the gateway through its service manager to apply saved changes."
            };
            let mut note = ListItem::new(ui.muted(message));
            if status.user_managed {
                note = note.trailing(
                    ui.button(Secondary, "apply-network", "Restart gateway")
                        .on_click(cx.listener(|v, _, _, cx| v.manage(Action::Restart, cx))),
                );
            }
            section = section.child(ui.separator()).child(note);
        } else {
            section = section.child(
                ui.muted("Network changes take effect at the next gateway start.")
                    .text_xs()
                    .pt(theme::SPACE_1),
            );
        }
        section
    }
}
