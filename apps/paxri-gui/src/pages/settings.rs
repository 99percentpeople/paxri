use super::settings_catalog as catalog;
use crate::{
    Dashboard, GatewayOffline,
    components::{
        ButtonVariant::Secondary, Icon, Select, SettingsGroup, SettingsSection, SettingsTable,
        Ui, actions, setting_field,
    },
    theme::{self, Appearance, SPACE_2, SPACE_3},
};
use anyhow::Context as _;
use paxri_ui::settings::controls::theme_cards;
use gpui::{prelude::*, *};
use paxri_config::gui::TabPlacement;
use paxri_cli::gateway::Action;
use paxri_config::startup::Startup;
use paxri_control::{Command, GuiChange};

impl Dashboard {
    fn appearance_settings(&self, ui: Ui, cx: &Context<Self>) -> SettingsGroup {
        let settings = theme::current(cx);
        let cards = theme_cards(*ui, settings.appearance, px(132.), cx.listener(|view, mode: &Appearance, _, cx| {
            view.send(Command::SetGui { change: GuiChange::Appearance(*mode) }, cx);
        }));
        let mut group = SettingsGroup::new(catalog::APPEARANCE);
        group.setting(catalog::THEME, cards);
        let position = Select::new(
            "tab-placement",
            settings.tab_placement,
            cx.listener(|view, placement: &TabPlacement, _, cx| {
                view.send(
                    Command::SetGui {
                        change: GuiChange::TabPlacement(*placement),
                    },
                    cx,
                );
            }),
        )
        .option_with_icon(TabPlacement::Top, "Top", Icon::TabsOnTop)
        .option_with_icon(TabPlacement::Left, "Left", Icon::TabsOnLeft);
        group.setting(catalog::TAB_PLACEMENT, setting_field(position));
        group
    }

    fn gateway_settings(&self, ui: Ui, cx: &Context<Self>) -> SettingsGroup {
        let online = self.status.is_some();
        let external = self.status.as_ref().is_some_and(|s| !s.user_managed);
        let (state, color) = match (self.gateway_transition(), &self.offline) {
            (Some(label), _) => (label, ui.colors.muted_foreground),
            _ if online => ("Running", ui.colors.success),
            (_, Some(GatewayOffline::Stopped)) => ("Stopped", ui.colors.muted_foreground),
            (_, Some(GatewayOffline::Failed(_))) => ("Offline", ui.colors.warning),
            _ => ("Connecting", ui.colors.muted_foreground),
        };
        let mut group = SettingsGroup::new(catalog::GATEWAY);
        let status_hint = if self
            .status
            .as_ref()
            .and_then(paxri_cli::gateway::other_installation)
            .is_some()
        {
            "Running from a different installation."
        } else {
            ""
        };
        group.add(
            "",
            "Gateway status",
            status_hint,
            &format!("runtime process {state}"),
            actions().child(ui.status_badge(state, color)),
        );

        let actions_hint = if external {
            "Startup and shutdown are controlled by the service manager."
        } else {
            "Start, stop or restart the local gateway."
        };
        let mut controls = actions().justify_end();
        if external {
            controls = controls.child(ui.muted("Service-managed").whitespace_nowrap());
        } else {
            for (id, text, action) in if online {
                vec![
                    ("gateway-restart", "Restart", Action::Restart),
                    ("gateway-stop", "Stop", Action::Stop),
                ]
            } else {
                vec![("gateway-start", "Start gateway", Action::Start)]
            } {
                controls = controls.child(
                    ui.button(Secondary, id, text)
                        .on_click(cx.listener(move |v, _, _, cx| v.manage(action, cx))),
                );
            }
        }
        group.add(
            "",
            "Gateway actions",
            actions_hint,
            "runtime process start stop restart",
            controls,
        );
        let policy = if external {
            setting_field(
                ui.muted(format!("{} · Service-managed", self.startup.label()))
                    .text_right(),
            )
        } else {
            ui.setting_choice(
                "gateway-startup",
                self.startup,
                Startup::ALL,
                Startup::label,
                cx.listener(|view, choice: &Startup, _, cx| {
                    view.manage(Action::Startup { mode: Some(*choice) }, cx)
                }),
            )
        };
        group.setting(catalog::STARTUP, policy);

        let toggle = ui
            .button(
                crate::components::ButtonVariant::Ghost,
                "gateway-details",
                if self.gateway_details {
                    "Hide details"
                } else {
                    "Show details"
                },
            )
            .icon(if self.gateway_details {
                Icon::ChevronDown
            } else {
                Icon::ChevronRight
            })
            .on_click(cx.listener(|v, _, _, cx| {
                v.gateway_details = !v.gateway_details;
                cx.notify();
            }));
        let diagnostics = group.add(
            "",
            "Gateway details",
            "Inspect the executable, startup registration, local IPC and configuration path.",
            "runtime diagnostics task scheduler socket",
            toggle,
        );
        if self.gateway_details {
            let mut lines = Vec::new();
            if let Some(image) = self
                .status
                .as_ref()
                .and_then(paxri_cli::gateway::running_image)
            {
                lines.push(image);
            }
            #[cfg(windows)]
            lines.extend(self.startup_report.iter().cloned());
            lines.push(format!(
                "Local IPC · {}",
                self.active_address()
                    .unwrap_or_else(paxri_control::default_address)
                    .display()
            ));
            if let Some(path) = &self.config_path {
                lines.push(format!("Configuration · {}", path.display()));
            }
            diagnostics.details = Some(
                div()
                    .w_full()
                    .flex()
                    .flex_col()
                    .gap(SPACE_2)
                    .p(SPACE_3)
                    .rounded(SPACE_2)
                    .bg(rgb(ui.colors.card))
                    .children(lines.into_iter().map(|line| ui.muted(line)))
                    .into_any_element(),
            );
        }
        if let Some(s) = &self.status {
            let id = s.device_id.clone();
            group.add(
                "",
                "Device ID",
                "Copy the full identifier for this device.",
                "runtime identity",
                actions()
                    .justify_end()
                    .child(
                        ui.muted(
                            paxri_config::peer_id::default_prefix(&s.device_id).to_owned(),
                        )
                        .flex_none()
                        .whitespace_nowrap(),
                    )
                    .child(ui.button(Secondary, "copy-id", "Copy ID").on_click(move |_, w, cx| {
                        crate::components::copy_with_feedback(id.clone(), "Device ID copied", w, cx);
                    })),
            );
        }
        group
    }

    pub(super) fn settings_groups(
        &self,
        ui: Ui,
        cx: &Context<Self>,
        requested: Option<SettingsTable>,
    ) -> (Option<SettingsTable>, Vec<SettingsGroup>) {
        type Builder<'a> = Box<dyn FnOnce() -> SettingsGroup + 'a>;
        let mut sources: Vec<(SettingsSection, Builder<'_>)> = Vec::new();
        if self.status.is_some() {
            sources.push((catalog::DEVICE, Box::new(move || self.device_settings())));
        }
        sources.push((
            catalog::GATEWAY,
            Box::new(move || self.gateway_settings(ui, cx)),
        ));
        sources.push((
            catalog::NETWORK,
            Box::new(move || self.network_settings(ui, cx)),
        ));
        sources.push((
            catalog::APPEARANCE,
            Box::new(move || self.appearance_settings(ui, cx)),
        ));
        sources.push((
            catalog::TERMINAL,
            Box::new(move || self.terminal_settings(ui, cx)),
        ));
        let selected = requested.map(|page| {
            if sources.iter().any(|(s, _)| s.table.top_level() == page) {
                page
            } else {
                catalog::GATEWAY.table
            }
        });
        let groups = sources
            .into_iter()
            .map(|(section, build)| {
                if selected.is_none_or(|page| section.table.top_level() == page) {
                    build()
                } else {
                    SettingsGroup::new(section)
                }
            })
            .collect();
        (selected, groups)
    }

    pub(super) fn open_config_file(&self, path: std::path::PathBuf, window: AnyWindowHandle, cx: &mut Context<Self>) {
        let launch = self.runtime.spawn_blocking(move || -> anyhow::Result<()> {
            let path = std::path::absolute(path)?;
            if !path.try_exists()? {
                paxri_config::file::init(&path)?;
            }
            open::that(&path).with_context(|| format!("Could not open {}", path.display()))
        });
        cx.spawn(async move |_, cx| {
            if let Err(error) = launch.await.map_err(anyhow::Error::from).and_then(|r| r) {
                let _ = cx.update(|cx| crate::components::Toast::show(format!("Open config file: {error:#}"), window, cx));
            }
        })
        .detach();
    }

    pub(crate) fn commit_device_name(&mut self, cx: &mut Context<Self>) {
        if self.status.is_some()
            && let Some(name) = self.name_field.changed_value(cx)
        {
            self.send(
                Command::SetDeviceName {
                    name: (!name.is_empty()).then_some(name),
                },
                cx,
            );
        }
    }

    fn device_settings(&self) -> SettingsGroup {
        SettingsGroup::device(self.name_field.render())
    }
}
