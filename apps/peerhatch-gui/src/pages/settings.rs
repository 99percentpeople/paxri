use crate::{
    Dashboard, Input,
    components::{
        Button, ButtonVariant::Secondary, Icon, ListItem, Select, Ui, row, heading, actions,
    },
    theme::Appearance,
};
use gpui::{prelude::*, *};
use peerhatch_cli::gateway::Action;
use peerhatch_config::startup::Startup;
use peerhatch_control::{Command, GuiChange};

fn copy_button(ui: Ui, id: &'static str, label: &'static str, text: String) -> Button {
    ui.button(Secondary, id, label)
        .on_click(move |_, _, cx| cx.write_to_clipboard(ClipboardItem::new_string(text.clone())))
}

impl Dashboard {
    fn appearance_settings(&self, ui: Ui, cx: &Context<Self>) -> Div {
        let chosen = crate::theme::current(cx).appearance;
        let icon = |mode| match mode {
            Appearance::System => Icon::Monitor,
            Appearance::Light => Icon::Sun,
            Appearance::Dark => Icon::Moon,
        };
        let mut modes = Select::new(
            "theme",
            chosen,
            cx.listener(|view, mode: &Appearance, _, cx| {
                view.send(
                    Command::SetGui {
                        change: GuiChange::Appearance(*mode),
                    },
                    cx,
                );
            }),
        )
        .icon_trigger(icon(chosen), format!("Theme: {}", chosen.label()));
        for mode in Appearance::ALL {
            modes = modes.option_with_icon(mode, mode.label(), icon(mode));
        }
        ui.section("Appearance")
            .child(row().child(ui.muted("Theme")).child(modes))
    }

    fn gateway_settings(&self, ui: Ui, cx: &Context<Self>) -> Div {
        let online = self.status.is_some();
        let external = self.status.as_ref().is_some_and(|s| !s.user_managed);
        let (state, color) = match self.gateway_action {
            Some(Action::Start) => ("Starting…", ui.colors.muted_foreground),
            Some(Action::Stop) => ("Stopping…", ui.colors.muted_foreground),
            Some(Action::Restart) => ("Restarting…", ui.colors.muted_foreground),
            _ if external => ("Service-managed", ui.colors.success),
            _ if online => ("Running", ui.colors.success),
            _ if self.error.is_some() => ("Offline", ui.colors.warning),
            _ => ("Connecting", ui.colors.muted_foreground),
        };
        let mut section = ui.card().child(
            row()
                .child(heading("Gateway"))
                .child(ui.status_badge(state, color)),
        );
        let mut policy = Select::new(
            "gateway-startup",
            self.startup,
            cx.listener(|view, choice: &Startup, _, cx| {
                view.manage(
                    Action::Startup {
                        mode: Some(*choice),
                    },
                    cx,
                )
            }),
        );
        for choice in Startup::ALL {
            policy = policy.option(choice, choice.label());
        }
        if self
            .status
            .as_ref()
            .and_then(peerhatch_cli::gateway::other_installation)
            .is_some()
        {
            section = section.child(ui.muted("Running from a different installation.").text_xs());
        }
        if external {
            section = section
                .child(ui.muted("Startup and shutdown are controlled by the service manager."));
        } else {
            let mut controls = actions();
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
            section = section
                .child(ui.labeled("Startup behavior", policy))
                .child(controls);
        }
        section = section.child(ui.separator()).child(
            ui.button(
                crate::components::ButtonVariant::Ghost,
                "gateway-details",
                "Gateway details",
            )
            .icon(if self.gateway_details {
                Icon::ChevronDown
            } else {
                Icon::ChevronRight
            })
            .on_click(cx.listener(|v, _, _, cx| {
                v.gateway_details = !v.gateway_details;
                cx.notify();
            })),
        );
        if self.gateway_details {
            if let Some(image) = self
                .status
                .as_ref()
                .and_then(peerhatch_cli::gateway::running_image)
            {
                section = section.child(ui.muted(image).text_xs());
            }
            #[cfg(windows)]
            for line in &self.startup_report {
                section = section.child(ui.muted(line.clone()).text_xs());
            }
            section = section.child(
                ui.muted(format!(
                    "Local IPC · {}",
                    self.active_address()
                        .unwrap_or_else(peerhatch_control::default_address)
                        .display()
                ))
                .text_xs(),
            );
            if let Some(path) = &self.config_path {
                section = section.child(
                    ui.muted(format!("Configuration · {}", path.display()))
                        .text_xs(),
                );
            }
        }
        section
    }

    pub(crate) fn settings(&self, ui: Ui, cx: &mut Context<Self>) -> Div {
        let mut header = row().child(
            div()
                .text_3xl()
                .font_weight(FontWeight::SEMIBOLD)
                .child("Settings"),
        );
        if let Some(path) = self.config_path.clone() {
            header = header.child(
                ui.button(Secondary, "open-config", "Open config file")
                    .icon(Icon::File)
                    .tooltip(crate::components::tooltip(
                        path.display().to_string().into(),
                    ))
                    .on_click(cx.listener(move |v, _, _, cx| v.open_config_file(path.clone(), cx))),
            );
        }
        let column = || div().min_w_0().flex().flex_col().gap(crate::theme::SPACE_4);
        let mut primary = column()
            .child(self.gateway_settings(ui, cx))
            .child(self.network_settings(ui, cx));
        let mut secondary = column()
            .child(self.appearance_settings(ui, cx))
            .child(self.terminal_settings(ui, cx));
        if let Some(s) = &self.status {
            secondary = secondary.child(self.device_settings(ui, s, cx));
            if !s.services.is_empty() {
                let mut services = ui.section("Published services");
                for (name, target) in &s.services {
                    services = services
                        .child(ui.separator())
                        .child(ListItem::new(name.clone()).detail(ui.muted(target.clone())));
                }
                primary = primary.child(services);
            }
        }
        super::overview()
            .child(header)
            .child(super::overview_columns(primary, secondary))
    }

    fn open_config_file(&self, path: std::path::PathBuf, cx: &mut Context<Self>) {
        let launch = self.runtime.spawn(async move {
            let path = tokio::task::spawn_blocking(move || -> anyhow::Result<_> {
                if !path.try_exists()? {
                    peerhatch_config::file::init(&path)?;
                }
                Ok(path)
            })
            .await??;
            #[cfg(windows)]
            let mut command = {
                let root = std::env::var_os("SystemRoot")
                    .ok_or_else(|| anyhow::anyhow!("Windows directory unavailable"))?;
                tokio::process::Command::new(
                    std::path::Path::new(&root).join("System32/notepad.exe"),
                )
            };
            #[cfg(target_os = "macos")]
            let mut command = {
                let mut c = tokio::process::Command::new("open");
                c.arg("-t");
                c
            };
            #[cfg(all(unix, not(target_os = "macos")))]
            let mut command = tokio::process::Command::new("xdg-open");
            let result = command.arg(&path).status().await?;
            anyhow::ensure!(
                result.success(),
                "Could not open {} ({result})",
                path.display()
            );
            Ok::<_, anyhow::Error>(())
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

    fn device_settings(&self, ui: Ui, s: &peerhatch_control::Status, cx: &Context<Self>) -> Div {
        let mut device = ui.section("This device").child(ui.separator());
        if let Some(field) = &self.name_field {
            device = device
                .child(ui.labeled("Device name", field.clone()))
                .child(ui.muted(format!(
                    "Leave blank to use the computer name ({})",
                    s.computer_name
                )))
                .child(ui.form_actions(
                    "name",
                    "Save name",
                    cx.listener(|v, _, _, cx| {
                        if !v.saving
                            && let Some(field) = &v.name_field
                        {
                            let text = field.read(cx).text().trim();
                            v.send(
                                Command::SetDeviceName {
                                    name: (!text.is_empty()).then(|| text.to_owned()),
                                },
                                cx,
                            );
                        }
                    }),
                    cx.listener(|v, _, _, cx| {
                        if !v.saving {
                            v.name_field = None;
                            v.action_error = None;
                            cx.notify();
                        }
                    }),
                ));
        } else {
            let name = s.device_name.clone().unwrap_or_default();
            let system = s.computer_name.clone();
            device = device.child(
                row()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(ui.muted("Device name"))
                            .child(s.display_name().to_owned())
                            .child(ui.muted(if s.device_name.is_none() {
                                "Using computer name"
                            } else {
                                "Custom name"
                            })),
                    )
                    .child(
                        ui.button(Secondary, "edit-name", "Change name")
                            .on_click(cx.listener(move |v, _, w, cx| {
                                let field = Input::new(&name, &system, cx);
                                cx.focus_view(&field, w);
                                v.name_field = Some(field);
                                v.action_error = None;
                                cx.notify();
                            })),
                    ),
            );
        }
        device.child(ui.separator()).child(
            ListItem::new("Device ID")
                .detail(
                    ui.muted(peerhatch_config::peer_id::default_prefix(&s.device_id).to_owned()),
                )
                .trailing(copy_button(ui, "copy-id", "Copy ID", s.device_id.clone())),
        )
    }
}
