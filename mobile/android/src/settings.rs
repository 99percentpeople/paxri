use super::*;
use paxri_config::{NativeChange, file, gui::GuiChange};
use paxri_ui::settings::controls::theme_cards;
use paxri_ui::{
    settings::{SettingsGroup, SettingsLayout, SettingsPage, SettingsSection, catalog},
    theme::Appearance,
};
use std::path::PathBuf;

const CONFIG_FILE: &str = "node.toml";
const LEGACY_GUI_FILE: &str = "gui.toml";

fn path(android: &AndroidApp) -> anyhow::Result<PathBuf> {
    android
        .internal_data_path()
        .map(|dir| dir.join(CONFIG_FILE))
        .ok_or_else(|| anyhow::anyhow!("App storage unavailable"))
}

pub(super) fn load(android: &AndroidApp) -> anyhow::Result<file::LocalSettings> {
    let path = path(android)?;
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
        match path(&self.android).and_then(|path| file::set_native(&path, change, true)) {
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
                        "Automatic public relay saved. Restart the app to apply."
                    } else {
                        "Relay URL saved. Restart the app to apply."
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

    pub(super) fn change_gui(&mut self, change: GuiChange, cx: &mut Context<Self>) -> bool {
        match path(&self.android).and_then(|path| file::set_gui(&path, change, true)) {
            Ok(settings) => {
                self.font = cx.global::<theme::fonts::MonospaceFonts>().resolve(&settings.terminal);
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
                    Self::settings_column()
                        .child(paxri_ui::settings::shortcuts::group(ui, cx, change).render_rows(ui)),
                )
                .into_any_element();
        }
        let device = SettingsGroup::device(paxri_ui::settings::controls::setting_field(
            self.name_input.clone(),
        ));
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
            terminal = self
                .terminal_settings
                .group(ui, cx, move |change, _, cx| {
                    let _ = view.update(cx, |view, cx| view.change_gui(change, cx));
                });
        }

        let mut network = SettingsGroup::new(catalog::NETWORK);
        if selected == Some(catalog::NETWORK) {
            network.setting(
                catalog::RELAY,
                paxri_ui::settings::controls::setting_field(self.relay_input.clone()),
            );
            let note = if self.saved_relay != self.active_relay {
                "Restart the app to apply saved changes."
            } else {
                "Network changes take effect at the next app start."
            };
            network.footer = Some(ui.caption(note).pt(theme::SPACE_1).into_any_element());
        }
        let mut groups = vec![device, network, appearance, terminal];
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
