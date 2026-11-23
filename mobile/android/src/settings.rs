use super::*;
use paxri_config::{file, gui::GuiChange};
use paxri_ui::settings::controls::theme_cards;
use paxri_ui::{
    settings::{SettingsGroup, SettingsLayout, SettingsPage, SettingsSection, catalog},
    theme::{Appearance, GuiSettings},
};
use std::path::PathBuf;

fn path(android: &AndroidApp) -> anyhow::Result<PathBuf> {
    android
        .internal_data_path()
        .map(|dir| dir.join("gui.toml"))
        .ok_or_else(|| anyhow::anyhow!("App storage unavailable"))
}

pub(super) fn load(android: &AndroidApp) -> anyhow::Result<GuiSettings> {
    let path = path(android)?;
    let settings = if path.try_exists()? {
        file::read_gui(&path)?
    } else {
        // Keep the existing Android dark theme on first launch and on upgrade.
        file::set_gui(&path, GuiChange::Appearance(Appearance::Dark), true)?
    };
    // Import the previous pinch/stepper preference once. The shared GUI config owns subsequent edits.
    let legacy = path.with_file_name("terminal-font-size");
    let settings = if settings.terminal.font_size.is_none()
        && let Ok(text) = std::fs::read_to_string(&legacy)
        && let Some(size) = text
            .trim()
            .parse::<f32>()
            .ok()
            .filter(|v| theme::TerminalAppearance::FONT_SIZES.contains(v))
    {
        file::set_gui(&path, GuiChange::FontSize(Some(size)), true)?
    } else {
        settings
    };
    if legacy.exists()
        && let Err(error) = std::fs::remove_file(legacy)
    {
        log::warn!("Retiring old font size preference: {error}");
    }
    if settings.keybar.is_none() {
        return file::set_gui(&path, GuiChange::Keybar(KeybarSettings::default()), true);
    }
    Ok(settings)
}

impl Mobile {
    pub(super) fn settings_body(&self, id: impl Into<ElementId>) -> Stateful<Div> {
        div()
            .id(id)
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&self.settings_scroll)
            .restrict_scroll_to_axis()
            .flex()
            .flex_col()
            .p_4()
            .gap_2()
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

    pub(super) fn change_gui(&mut self, change: GuiChange, cx: &mut Context<Self>) -> bool {
        match path(&self.android).and_then(|path| file::set_gui(&path, change, true)) {
            Ok(settings) => {
                self.font = cx.global::<theme::fonts::MonospaceFonts>().resolve(&settings.terminal);
                self.update_session_fonts();
                self.keybar = settings.keybar.clone().unwrap_or_default();
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
        available_width: Pixels,
        cx: &Context<Self>,
    ) -> Stateful<Div> {
        if selected == Some(keybar::SECTION) {
            return self.keybar_editor(ui, cx);
        }
        let device = SettingsGroup::device(paxri_ui::settings::controls::setting_field(
            self.name_input.clone(),
        ));
        let mut appearance = SettingsGroup::new(catalog::APPEARANCE);
        let mut terminal = SettingsGroup::new(catalog::TERMINAL);
        if selected == Some(catalog::page_for(catalog::TERMINAL)) {
            let settings = theme::current(cx);
            let card_width = ((available_width - px(48.)) / 3.)
                .max(px(96.))
                .min(px(132.));
            appearance.setting(
                catalog::THEME,
                theme_cards(
                    ui,
                    settings.appearance,
                    card_width,
                    cx.listener(|view, mode: &Appearance, _, cx| {
                        view.change_gui(GuiChange::Appearance(*mode), cx);
                    }),
                ),
            );
            let view = cx.entity().downgrade();
            terminal = self.terminal_settings.group(
                ui,
                cx,
                Some(&self.terminal_menus),
                move |change, _, cx| {
                    let _ = view.update(cx, |view, cx| view.change_gui(change, cx));
                },
            );
        }

        let pages = SettingsPage::collect(vec![device, appearance, terminal]);
        let mut body = self.settings_body((
            ElementId::from("settings"),
            SharedString::from(selected.map(|section| section.title).unwrap_or("categories")),
        ));
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
        if selected.is_none() {
            body = body.child(paxri_ui::settings::controls::category_button(
                ui, Some(keybar::SECTION), false, None, SettingsLayout::Mobile,
            ).role(Role::Button).aria_label("Terminal keys")
                .on_click(cx.listener(|v, _, _, cx| v.open_keybar(cx))));
        }
        body
    }
}
