use super::settings_catalog as catalog;
use crate::{
    Dashboard,
    components::{
        Button, ButtonVariant::Secondary, Icon, Select, SettingsGroup, SettingsSection, SettingsTable,
        Ui, actions, setting_field,
    },
    theme::{self, Appearance, SPACE_1, SPACE_2, SPACE_3},
};
use anyhow::Context as _;
use gpui::{prelude::*, *};
use peerhatch_config::gui::TabPlacement;
use peerhatch_cli::gateway::Action;
use peerhatch_config::startup::Startup;
use peerhatch_control::{Command, GuiChange};

fn copy_button(ui: Ui, id: &'static str, label: &'static str, text: String) -> Button {
    ui.button(Secondary, id, label)
        .on_click(move |_, _, cx| cx.write_to_clipboard(ClipboardItem::new_string(text.clone())))
}

fn appearance_preview(mode: Appearance) -> Div {
    let (sidebar, page) = match mode {
        Appearance::System => (&theme::LIGHT, &theme::DARK),
        Appearance::Light => (&theme::LIGHT, &theme::LIGHT),
        Appearance::Dark => (&theme::DARK, &theme::DARK),
    };
    let line = |width, color| div().w(px(width)).h(px(3.)).rounded_full().bg(rgb(color));
    div()
        .w_full()
        .h(px(48.))
        .flex()
        .overflow_hidden()
        .rounded(SPACE_1)
        .border_1()
        .border_color(rgb(sidebar.input_border))
        .child(
            div()
                .w(px(35.))
                .flex_none()
                .flex()
                .flex_col()
                .gap(SPACE_1)
                .p(SPACE_1)
                .py(SPACE_2)
                .bg(rgb(sidebar.secondary.background))
                .child(line(21., sidebar.muted_foreground))
                .child(line(15., sidebar.input_border)),
        )
        .child(
            div()
                .flex_1()
                .flex()
                .flex_col()
                .gap(SPACE_1)
                .p(SPACE_2)
                .bg(rgb(page.canvas.background))
                .child(line(24., page.canvas.foreground))
                .child(line(36., page.input_border))
                .child(line(28., page.input_border)),
        )
}

impl Dashboard {
    fn appearance_settings(&self, ui: Ui, cx: &Context<Self>) -> SettingsGroup {
        let settings = theme::current(cx);
        let cards = actions()
            .justify_end()
            .children(Appearance::ALL.into_iter().map(|mode| {
                let icon = match mode {
                    Appearance::System => Icon::Monitor,
                    Appearance::Light => Icon::Sun,
                    Appearance::Dark => Icon::Moon,
                };
                ui.choice_card(
                    ("theme", mode as usize),
                    px(132.),
                    icon.label(mode.label()),
                    appearance_preview(mode),
                    settings.appearance == mode,
                )
                .on_click(cx.listener(move |view, _, _, cx| {
                    view.send(
                        Command::SetGui {
                            change: GuiChange::Appearance(mode),
                        },
                        cx,
                    );
                }))
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

    fn runtime_settings(&self, ui: Ui, cx: &Context<Self>) -> SettingsGroup {
        let online = self.status.is_some();
        let external = self.status.as_ref().is_some_and(|s| !s.user_managed);
        let (state, color) = match self.gateway_action {
            Some(Action::Start) => ("Starting…", ui.colors.muted_foreground),
            Some(Action::Stop) => ("Stopping…", ui.colors.muted_foreground),
            Some(Action::Restart) => ("Restarting…", ui.colors.muted_foreground),
            _ if online => ("Running", ui.colors.success),
            _ if self.error.is_some() => ("Offline", ui.colors.warning),
            _ => ("Connecting", ui.colors.muted_foreground),
        };
        let mut group = SettingsGroup::new(catalog::RUNTIME);
        let status_hint = if self
            .status
            .as_ref()
            .and_then(peerhatch_cli::gateway::other_installation)
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
            &format!("process {state}"),
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
            "process start stop restart",
            controls,
        );
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
            "diagnostics task scheduler socket",
            toggle,
        );
        if self.gateway_details {
            let mut lines = Vec::new();
            if let Some(image) = self
                .status
                .as_ref()
                .and_then(peerhatch_cli::gateway::running_image)
            {
                lines.push(image);
            }
            #[cfg(windows)]
            lines.extend(self.startup_report.iter().cloned());
            lines.push(format!(
                "Local IPC · {}",
                self.active_address()
                    .unwrap_or_else(peerhatch_control::default_address)
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
            group.add(
                "",
                "Device ID",
                "Copy the full identifier for this device.",
                "identity",
                actions()
                    .justify_end()
                    .child(
                        ui.muted(
                            peerhatch_config::peer_id::default_prefix(&s.device_id).to_owned(),
                        )
                        .flex_none()
                        .whitespace_nowrap(),
                    )
                    .child(copy_button(ui, "copy-id", "Copy ID", s.device_id.clone())),
            );
        }
        group
    }

    fn gateway_settings(&self, ui: Ui, cx: &Context<Self>) -> SettingsGroup {
        let mut gateway = SettingsGroup::new(catalog::GATEWAY);
        let policy = if self.status.as_ref().is_some_and(|s| !s.user_managed) {
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
                    view.manage(
                        Action::Startup {
                            mode: Some(*choice),
                        },
                        cx,
                    )
                }),
            )
        };
        gateway.setting(catalog::STARTUP, policy);
        gateway
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
            sources.push((catalog::DEVICE, Box::new(move || self.device_settings(ui))));
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
            catalog::PORT_ACCESS,
            Box::new(move || self.port_access_settings(ui, cx)),
        ));
        sources.push((
            catalog::APPEARANCE,
            Box::new(move || self.appearance_settings(ui, cx)),
        ));
        sources.push((
            catalog::TERMINAL,
            Box::new(move || self.terminal_settings(ui, cx)),
        ));
        sources.push((
            catalog::SERVICES,
            Box::new(move || self.published_services(ui, cx)),
        ));
        sources.push((
            catalog::RUNTIME,
            Box::new(move || self.runtime_settings(ui, cx)),
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

    pub(super) fn open_config_file(&self, path: std::path::PathBuf, cx: &mut Context<Self>) {
        let launch = self.runtime.spawn_blocking(move || -> anyhow::Result<()> {
            let path = std::path::absolute(path)?;
            if !path.try_exists()? {
                peerhatch_config::file::init(&path)?;
            }
            open::that(&path).with_context(|| format!("Could not open {}", path.display()))
        });
        cx.spawn(async move |this, cx| {
            if let Err(error) = launch.await.map_err(anyhow::Error::from).and_then(|r| r) {
                let _ = this.update(cx, |v, cx| {
                    v.action_error = Some(format!("Open config file: {error:#}"));
                    cx.notify();
                });
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

    fn device_settings(&self, ui: Ui) -> SettingsGroup {
        let mut group = SettingsGroup::new(catalog::DEVICE);
        group.setting(catalog::DEVICE_NAME, self.name_field.render(ui));
        group
    }
}
