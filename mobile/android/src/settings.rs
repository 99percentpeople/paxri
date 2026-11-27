use super::*;
use paxri_config::{NativeChange, file, gui::GuiChange, startup::GatewayChange};
use paxri_ui::components::{ButtonVariant, Icon, actions};
use paxri_ui::settings::controls::theme_cards;
use paxri_ui::{
    settings::{SettingsGroup, SettingsLayout, SettingsPage, SettingsSection, catalog},
    theme::{self, Appearance, SPACE_1, SPACE_2, SPACE_3},
};
use std::path::{Path, PathBuf};

const CONFIG_FILE: &str = "node.toml";
const LEGACY_GUI_FILE: &str = "gui.toml";

fn config_path(dir: Option<&Path>) -> anyhow::Result<PathBuf> {
    dir.map(|dir| dir.join(CONFIG_FILE))
        .ok_or_else(|| anyhow::anyhow!("App storage unavailable"))
}

pub(super) fn load_dir(dir: &Path) -> anyhow::Result<file::LocalSettings> {
    let path = dir.join(CONFIG_FILE);
    let legacy_gui = path.with_file_name(LEGACY_GUI_FILE);
    if !path.try_exists()? && legacy_gui.try_exists()? {
        std::fs::rename(&legacy_gui, &path)?;
    }
    let mut settings = if path.try_exists()? {
        file::read_local(&path)?
    } else {
        file::LocalSettings::default()
    };
    // Import the previous pinch/stepper preference once. The shared config owns subsequent edits.
    let legacy = path.with_file_name("terminal-font-size");
    if settings.gui.terminal.font_size.is_none()
        && let Ok(text) = std::fs::read_to_string(&legacy)
        && let Some(size) = text
            .trim()
            .parse::<f32>()
            .ok()
            .filter(|v| theme::TerminalAppearance::FONT_SIZES.contains(v))
    {
        settings.gui = file::set_gui(&path, GuiChange::FontSize(Some(size)), true)?;
    }
    if legacy.exists()
        && let Err(error) = std::fs::remove_file(legacy)
    {
        log::warn!("Retiring old font size preference: {error}");
    }
    if settings.gui.keybar.is_none() {
        settings.gui = file::set_gui(&path, GuiChange::Keybar(KeybarSettings::default()), true)?;
    }
    Ok(settings)
}

impl Mobile {
    pub(super) fn settings_column() -> Div {
        div()
            .flex_1()
            .min_w_0()
            .max_w(paxri_ui::settings::MAX_WIDTH)
            .flex()
            .flex_col()
            .gap_2()
    }

    pub(super) fn settings_body(
        &self,
        id: impl Into<ElementId>,
        content: impl IntoElement,
    ) -> paxri_ui::components::ScrollArea {
        paxri_ui::components::ScrollArea::new(
            id,
            div()
                .p_4()
                .child(div().w_full().flex().justify_center().child(content)),
        )
        .with_handle(self.settings_scroll.clone())
    }

    pub(super) fn commit_device_name(&mut self, cx: &mut App) {
        let input = self.name_input.read(cx);
        if input.is_composing() {
            return;
        }
        let value = input.text().trim().to_owned();
        if value != DEVICE_NAME.borrow().custom {
            if let Err(error) =
                activity_action(&self.android, ActivityAction::SaveDeviceName(&value))
            {
                Toast::show(error, self.window, cx);
                return;
            }
            Toast::show("Device name saved", self.window, cx);
        }
        self.name_input
            .update(cx, |input, cx| input.set_text(&value, cx));
    }

    pub(super) fn commit_relay_url(&mut self, cx: &mut Context<Self>) {
        let input = self.relay_input.read(cx);
        if input.is_composing() {
            return;
        }
        let value = input.text().trim().to_owned();
        if value == self.saved_relay {
            self.relay_input
                .update(cx, |input, cx| input.set_text(&value, cx));
            return;
        }
        let change = NativeChange::RelayUrl((!value.is_empty()).then_some(value));
        match config_path(self.data_dir.as_deref())
            .and_then(|path| file::set_native(&path, change, true))
        {
            Ok(native) => {
                let saved = native
                    .relay_url
                    .as_ref()
                    .map(|url| url.as_str().to_owned())
                    .unwrap_or_default();
                let changed = saved != self.saved_relay;
                self.saved_relay = saved.clone();
                self.relay_input
                    .update(cx, |input, cx| input.set_text(&saved, cx));
                if changed {
                    let message = if self.saved_relay == self.active_relay {
                        "Relay URL restored to active setting"
                    } else if self.saved_relay.is_empty() {
                        "Automatic public relay saved. Restart the gateway to apply."
                    } else {
                        "Relay URL saved. Restart the gateway to apply."
                    };
                    Toast::show(message, self.window, cx);
                }
                cx.notify();
            }
            Err(error) => {
                Toast::show(format!("{error:#}"), self.window, cx);
            }
        }
    }

    pub(super) fn change_gateway(
        &mut self,
        change: GatewayChange,
        cx: &mut Context<Self>,
    ) -> bool {
        match config_path(self.data_dir.as_deref())
            .and_then(|path| file::set_gateway(&path, change, true))
        {
            Ok(settings) => {
                self.gateway_policy = GatewayPolicy::from(&settings);
                GATEWAY.set_policy(self.gateway_policy);
                cx.notify();
                true
            }
            Err(error) => {
                Toast::show(format!("Saving gateway settings: {error:#}"), self.window, cx);
                false
            }
        }
    }

    pub(super) fn change_gui(&mut self, change: GuiChange, cx: &mut Context<Self>) -> bool {
        match config_path(self.data_dir.as_deref())
            .and_then(|path| file::set_gui(&path, change, true))
        {
            Ok(settings) => {
                self.font = cx
                    .global::<theme::fonts::MonospaceFonts>()
                    .resolve(&settings.terminal);
                self.update_session_fonts();
                self.keybar = settings.keybar.clone().unwrap_or_default();
                *HARDWARE_CONTROLS.lock().unwrap() = settings.controls.clone();
                theme::apply(settings, cx);
                cx.notify();
                true
            }
            Err(error) => {
                Toast::show(format!("Saving settings: {error:#}"), self.window, cx);
                false
            }
        }
    }

    fn gateway_settings(&self, ui: Ui, cx: &Context<Self>) -> SettingsGroup {
        let mut group = SettingsGroup::new(catalog::GATEWAY);
        let state = self.gateway_status.label();
        let color = match &self.gateway_status {
            GatewayStatus::Running { .. } => ui.colors.success,
            GatewayStatus::Failed(_) => ui.colors.warning,
            GatewayStatus::Stopped
            | GatewayStatus::Starting
            | GatewayStatus::Stopping
            | GatewayStatus::Restarting => ui.colors.muted_foreground,
        };
        let status_hint = match &self.gateway_status {
            GatewayStatus::Running {
                connected_count,
                paired_count,
                ..
            } => {
                if *paired_count == 0 {
                    "No paired devices yet.".to_owned()
                } else {
                    format!("{connected_count} of {paired_count} paired devices connected.")
                }
            }
            GatewayStatus::Failed(error) => error.clone(),
            GatewayStatus::Stopped => {
                "Connections and background keep-alive are paused.".to_owned()
            }
            GatewayStatus::Starting => "Starting embedded gateway…".to_owned(),
            GatewayStatus::Restarting => "Restarting embedded gateway…".to_owned(),
            GatewayStatus::Stopping => "Stopping embedded gateway…".to_owned(),
        };
        group.add(
            "",
            "Gateway status",
            &status_hint,
            &format!("runtime process service {state}"),
            actions().justify_end().child(ui.status_badge(state, color)),
        );

        let mut controls = actions().justify_end();
        if self.gateway_status.is_transitioning() {
            controls = controls.child(
                div()
                    .h(theme::BUTTON_HEIGHT)
                    .flex()
                    .items_center()
                    .child(ui.muted(format!("{state}…")).whitespace_nowrap()),
            );
        } else if self.gateway_status.is_running() {
            controls = controls
                .child(
                    ui.button(ButtonVariant::Secondary, "gateway-restart", "Restart")
                        .on_click(cx.listener(|_, _, _, _| GATEWAY.restart())),
                )
                .child(
                    ui.button(ButtonVariant::Secondary, "gateway-stop", "Stop")
                        .on_click(cx.listener(|_, _, _, _| GATEWAY.stop())),
                );
        } else {
            controls = controls.child(
                ui.button(ButtonVariant::Secondary, "gateway-start", "Start gateway")
                    .on_click(cx.listener(|_, _, _, _| GATEWAY.start())),
            );
        }
        group.add(
            "",
            "Gateway actions",
            "Start, stop, or restart the local background gateway.",
            "runtime process start stop restart",
            controls,
        );

        let bg_enabled = self.gateway_policy.background_service;
        group.setting(
            catalog::BACKGROUND_SERVICE,
            ui.setting_toggle(
                "gateway-background-service",
                bg_enabled,
                true,
                cx.listener(move |view, _, _, cx| {
                    let next = !view.gateway_policy.background_service;
                    view.change_gateway(GatewayChange::BackgroundService(next), cx);
                }),
            ),
        );

        let keep_alive = self.gateway_policy.screen_off_keep_alive;
        group.setting(
            catalog::SCREEN_OFF_KEEP_ALIVE,
            ui.setting_toggle(
                "gateway-screen-off-keep-alive",
                keep_alive,
                bg_enabled,
                cx.listener(move |view, _, _, cx| {
                    let next = !view.gateway_policy.screen_off_keep_alive;
                    view.change_gateway(GatewayChange::ScreenOffKeepAlive(next), cx);
                }),
            ),
        );

        let unrestricted = self.battery_unrestricted;
        let (badge, badge_color, action_label) = if unrestricted {
            ("Unrestricted", ui.colors.success, "Manage")
        } else {
            ("Optimized", ui.colors.warning, "Allow")
        };
        group.add(
            "",
            "Battery optimization",
            "Current Android background power policy for paxri.",
            "background doze power battery unrestricted keep alive",
            actions()
                .justify_end()
                .child(ui.status_badge(badge, badge_color)),
        );
        group.add(
            "",
            "Battery settings",
            "Allow unrestricted background activity so Android Doze does not pause the gateway.",
            "background doze power battery unrestricted keep alive settings",
            actions().justify_end().child(
                ui.button(
                    ButtonVariant::Secondary,
                    "battery-optimization",
                    action_label,
                )
                .on_click(cx.listener(|view, _, _, cx| {
                    if let Err(error) =
                        activity_action(&view.android, ActivityAction::OpenBatterySettings)
                    {
                        Toast::show(error, view.window, cx);
                    }
                })),
            ),
        );

        let toggle = ui
            .button(
                ButtonVariant::Ghost,
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
            .on_click(cx.listener(|view, _, _, cx| {
                view.gateway_details = !view.gateway_details;
                cx.notify();
            }));
        let diagnostics = group.add(
            "",
            "Gateway details",
            "Inspect the active relay, connection state, and configuration path.",
            "runtime diagnostics relay config path",
            toggle,
        );
        if self.gateway_details {
            let mut lines = Vec::new();
            let active_relay = if self.active_relay.is_empty() {
                "Automatic public relay"
            } else {
                self.active_relay.as_str()
            };
            lines.push(format!("Relay · {active_relay}"));
            if self.saved_relay != self.active_relay {
                let pending = if self.saved_relay.is_empty() {
                    "Automatic public relay"
                } else {
                    self.saved_relay.as_str()
                };
                lines.push(format!("Pending relay (restart to apply) · {pending}"));
            }
            lines.push(format!(
                "Foreground service · {}",
                if self.gateway_status.is_running() && self.gateway_policy.background_service {
                    "Active"
                } else {
                    "Inactive"
                }
            ));
            if let Ok(path) = config_path(self.data_dir.as_deref()) {
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

        if let Some(id) = self.gateway_status.device_id() {
            let full_id = id.to_owned();
            let prefix = paxri_config::peer_id::default_prefix(id).to_owned();
            group.add(
                "",
                "Device ID",
                "Copy the full identifier for this device.",
                "runtime identity",
                actions()
                    .justify_end()
                    .child(ui.muted(prefix).flex_none().whitespace_nowrap())
                    .child(
                        ui.button(ButtonVariant::Secondary, "copy-device-id", "Copy ID")
                            .on_click(cx.listener(move |view, _, _, cx| {
                                match gpui_mobile::packages::clipboard::set_text(&full_id) {
                                    Ok(()) => Toast::show("Device ID copied", view.window, cx),
                                    Err(error) => Toast::show(error, view.window, cx),
                                }
                            })),
                    ),
            );
        }

        group
    }

    pub(super) fn settings(
        &self,
        selected: Option<SettingsSection>,
        ui: Ui,
        cx: &Context<Self>,
    ) -> AnyElement {
        if selected == Some(keybar::SECTION) {
            return self.keybar_editor(ui, cx).into_any_element();
        }
        if selected == Some(catalog::SHORTCUTS) {
            let view = cx.entity().downgrade();
            let change = move |change, _: &mut Window, cx: &mut App| {
                let _ = view.update(cx, |view, cx| view.change_gui(change, cx));
            };
            return self
                .settings_body(
                    (
                        ElementId::from("settings"),
                        SharedString::from(catalog::SHORTCUTS.title),
                    ),
                    Self::settings_column().child(
                        paxri_ui::settings::shortcuts::group(ui, cx, change).render_rows(ui),
                    ),
                )
                .into_any_element();
        }
        let device = SettingsGroup::device(paxri_ui::settings::controls::setting_field(
            self.name_input.clone(),
        ));
        let gateway = if selected == Some(catalog::GATEWAY) {
            self.gateway_settings(ui, cx)
        } else {
            SettingsGroup::new(catalog::GATEWAY)
        };
        let mut appearance = SettingsGroup::new(catalog::APPEARANCE);
        let mut terminal = SettingsGroup::new(catalog::TERMINAL);
        if selected == Some(catalog::page_for(catalog::TERMINAL)) {
            let settings = theme::current(cx);
            appearance.setting(
                catalog::THEME,
                theme_cards(
                    ui,
                    settings.appearance,
                    cx.listener(|view, mode: &Appearance, _, cx| {
                        view.change_gui(GuiChange::Appearance(*mode), cx);
                    }),
                ),
            );
            let view = cx.entity().downgrade();
            terminal = self.terminal_settings.group(ui, cx, move |change, _, cx| {
                let _ = view.update(cx, |view, cx| view.change_gui(change, cx));
            });
        }

        let mut network = SettingsGroup::new(catalog::NETWORK);
        if selected == Some(catalog::NETWORK) {
            network.setting(
                catalog::RELAY,
                paxri_ui::settings::controls::setting_field(self.relay_input.clone()),
            );
            if self.saved_relay != self.active_relay {
                network.footer = Some(
                    paxri_ui::components::row()
                        .w_full()
                        .pt(SPACE_1)
                        .gap_x(SPACE_3)
                        .gap_y(SPACE_2)
                        .child(
                            ui.caption("Restart the gateway to apply saved relay changes.")
                                .min_w_0()
                                .flex_1()
                                .flex_basis(px(220.)),
                        )
                        .child(
                            div().ml_auto().flex_none().child(
                                ui.button(
                                    ButtonVariant::Secondary,
                                    "network-restart-gateway",
                                    "Restart gateway",
                                )
                                .on_click(cx.listener(|_, _, _, _| GATEWAY.restart())),
                            ),
                        )
                        .into_any_element(),
                );
            } else {
                network.footer = Some(
                    ui.caption("Relay URL changes take effect after restarting the gateway.")
                        .pt(SPACE_1)
                        .into_any_element(),
                );
            }
        }
        let mut groups = vec![device, gateway, network, appearance, terminal];
        if selected == Some(catalog::TERMINAL_CONTROLS) {
            let view = cx.entity().downgrade();
            let change = move |change, _: &mut Window, cx: &mut App| {
                let _ = view.update(cx, |view, cx| view.change_gui(change, cx));
            };
            groups.push(paxri_ui::settings::shortcuts::features_group(ui, cx, change));
        } else {
            groups.push(SettingsGroup::new(catalog::TERMINAL_CONTROLS));
        }
        let pages = SettingsPage::collect(groups);
        let mut body = Self::settings_column();
        for page in pages {
            let section = page.section;
            if selected.is_none() {
                body = body.child(
                    paxri_ui::settings::controls::category_button(
                        ui,
                        Some(section),
                        false,
                        None,
                        SettingsLayout::Mobile,
                    )
                    .role(Role::Button)
                    .aria_label(section.title)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.navigate(Page::SettingsCategory(section), cx);
                        cx.notify();
                    })),
                );
            } else if selected == Some(section) {
                body = body.child(page.render(ui, SettingsLayout::Mobile));
            }
        }
        if selected == Some(catalog::TERMINAL_CONTROLS) {
            body = body
                .child(ui.separator())
                .child(
                    paxri_ui::settings::controls::category_button(
                        ui,
                        Some(keybar::SECTION),
                        false,
                        None,
                        SettingsLayout::Mobile,
                    )
                    .role(Role::Button)
                    .aria_label("Terminal keys")
                    .on_click(cx.listener(|v, _, _, cx| v.open_keybar(cx))),
                )
                .child(
                    paxri_ui::settings::controls::category_button(
                        ui,
                        Some(catalog::SHORTCUTS),
                        false,
                        None,
                        SettingsLayout::Mobile,
                    )
                    .role(Role::Button)
                    .aria_label(catalog::SHORTCUTS.title)
                    .on_click(cx.listener(|v, _, _, cx| {
                        v.navigate(Page::SettingsCategory(catalog::SHORTCUTS), cx);
                        cx.notify();
                    })),
                );
        }
        self.settings_body(
            (
                ElementId::from("settings"),
                SharedString::from(selected.map(|section| section.title).unwrap_or("categories")),
            ),
            body,
        )
        .into_any_element()
    }
}
