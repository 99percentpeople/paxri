use super::settings_catalog as catalog;
use crate::components::SettingsGroup;
use crate::{
    Dashboard,
    components::{ButtonVariant::Secondary, Ui, row},
    theme,
};
use gpui::{prelude::*, *};
use paxri_cli::gateway::Action;
use paxri_config::NativeChange;
use paxri_control::Command;

impl Dashboard {
    pub(crate) fn commit_relay_url(&mut self, cx: &mut Context<Self>) {
        if self.status.as_ref().is_some_and(|s| s.native.is_none()) {
            return;
        }
        if let Some(url) = self.relay_field.changed_value(cx) {
            self.send(
                Command::SetNative {
                    change: NativeChange::RelayUrl((!url.is_empty()).then_some(url)),
                },
                cx,
            );
        }
    }

    pub(super) fn network_settings(&self, ui: Ui, cx: &Context<Self>) -> SettingsGroup {
        let mut group = SettingsGroup::new(catalog::NETWORK);
        let mut notice = div().flex().flex_col().items_start().gap_2();
        let legacy = self.status.as_ref().filter(|s| s.native.is_none());
        if let Some(status) = legacy {
            notice = notice.child(
                ui.muted("This gateway must be updated before network settings can be changed."),
            );
            notice = if status.user_managed {
                notice.child(
                    ui.button(
                        Secondary,
                        "upgrade-network-gateway",
                        "Restart with this version",
                    )
                    .on_click(cx.listener(|v, _, _, cx| v.manage(Action::Restart, cx))),
                )
            } else {
                notice.child(ui.muted("Update it through the service manager."))
            };
        }
        let settings = self
            .status
            .as_ref()
            .and_then(|s| s.native.as_ref())
            .or(self.profile_native.as_ref());
        let Some(settings) = settings else {
            group.notice = Some(
                notice
                    .child(
                        ui.muted("Network settings appear when a node configuration is available."),
                    )
                    .into_any_element(),
            );
            return group;
        };

        if legacy.is_some() {
            group.notice = Some(notice.into_any_element());
        }
        for (spec, enabled, change) in [
            (
                catalog::UPNP,
                settings.upnp,
                NativeChange::Upnp(!settings.upnp),
            ),
            (
                catalog::LAN_MAPPING,
                settings.lan_port_mapping,
                NativeChange::LanPortMapping(!settings.lan_port_mapping),
            ),
        ] {
            let control = ui.setting_toggle(
                spec.key,
                enabled,
                legacy.is_none(),
                cx.listener(move |v, _, _, cx| {
                    v.send(
                        Command::SetNative {
                            change: change.clone(),
                        },
                        cx,
                    );
                }),
            );
            group.setting(spec, control);
        }

        let relay = if legacy.is_some() {
            let value = settings
                .relay_url
                .as_ref()
                .map_or("Automatic public relay", |url| url.as_str());
            ui.muted(value.to_owned()).text_right()
        } else {
            self.relay_field.render()
        };
        group.setting(catalog::RELAY, relay);
        if let Some(status) = &self.status
            && status.network_restart_required
        {
            let message = if status.user_managed {
                "Restart the gateway to apply saved changes."
            } else {
                "Restart the gateway through its service manager to apply saved changes."
            };
            let mut note = row().w_full().child(ui.muted(message));
            if status.user_managed {
                note = note.child(
                    ui.button(Secondary, "apply-network", "Restart gateway")
                        .on_click(cx.listener(|v, _, _, cx| v.manage(Action::Restart, cx))),
                );
            }
            group.footer = Some(note.into_any_element());
        } else {
            group.footer = Some(
                ui.caption("Network changes take effect at the next gateway start.")
                    .pt(theme::SPACE_1)
                    .into_any_element(),
            );
        }
        group
    }
}
