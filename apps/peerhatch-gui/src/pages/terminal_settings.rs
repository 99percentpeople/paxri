use crate::{
    Dashboard,
    components::{SettingsGroup, Ui},
};
use gpui::{App, Context};
use peerhatch_control::{Command, GuiChange};

impl Dashboard {
    pub(super) fn change_terminal_setting(&mut self, change: GuiChange, cx: &mut Context<Self>) {
        self.send(Command::SetGui { change }, cx);
    }

    pub(super) fn terminal_settings(&self, ui: Ui, cx: &Context<Self>) -> SettingsGroup {
        let view = cx.entity().downgrade();
        self.settings_editor
            .terminal
            .group(*ui, cx, None, move |change, _, cx: &mut App| {
                let _ = view.update(cx, |view, cx| view.change_terminal_setting(change, cx));
            })
    }
}
