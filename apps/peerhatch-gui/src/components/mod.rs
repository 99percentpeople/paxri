//! Desktop workspace controls and shared GPUI components.
mod dialog;
pub mod dock;
mod focus;
pub(crate) use dialog::{Dialog, DialogContent, DialogMode, DialogSuccess};
use peerhatch_ui::components::input;
mod scrollbar;
mod settings;
use gpui::{prelude::*, *};
pub use input::Input;
#[expect(dead_code, reason = "Reusable multiline control for form pages")]
pub type Textarea = input::TextInput<true>;
pub use scrollbar::{ScrollMetrics, ScrollSource, Scrollbar};
pub(crate) use peerhatch_ui::settings::{SettingsGroup, SettingsSection, SettingsTable};
pub(crate) use settings::{TextSetting, setting_field};

pub use peerhatch_ui::components::{
    Button, ButtonVariant, Dropdown, DropdownContent, DropdownState, DropdownTrigger, Icon, ListItem, Select, actions, form_row, heading, row, single_line,
    tooltip,
};

#[derive(Clone, Copy)]
pub struct Ui(peerhatch_ui::components::Ui);

impl Ui {
    pub fn new(window: &Window, cx: &App) -> Self {
        Self(peerhatch_ui::components::Ui::new(window, cx))
    }
}
impl std::ops::Deref for Ui {
    type Target = peerhatch_ui::components::Ui;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

pub(crate) fn observe_window_focus<V: 'static>(
    window: &mut Window,
    fallback: &FocusHandle,
    cx: &mut Context<V>,
) {
    #[cfg(windows)]
    crate::ime::observe(window);
    focus::observe_window_focus(window, fallback, cx);
}
