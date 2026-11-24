//! Desktop workspace controls and shared GPUI components.
mod dialog;
pub mod dock;
mod focus;
pub(crate) use dialog::{Dialog, DialogContent, DialogMode, DialogSuccess};
use paxri_ui::components::input;
mod scrollbar;
mod settings;
use gpui::{prelude::*, *};
pub use input::Input;
#[expect(dead_code, reason = "Reusable multiline control for form pages")]
pub type Textarea = input::TextInput<true>;
pub use scrollbar::{ScrollMetrics, ScrollSource, Scrollbar};
pub(crate) use paxri_ui::settings::{SettingsGroup, SettingsSection, SettingsTable};
pub(crate) use settings::{TextSetting, setting_field};
pub(crate) use paxri_ui::components::Toast;

pub(crate) fn notify_active(message: impl Into<SharedString>, cx: &mut App) {
    if let Some(window) = cx.active_window().or_else(|| cx.windows().first().copied()) {
        Toast::show(message, window, cx);
    }
}

pub(crate) fn copy_with_feedback(text: String, message: &'static str, window: &mut Window, cx: &mut App) {
    cx.write_to_clipboard(ClipboardItem::new_string(text.clone()));
    // Some platforms transfer clipboard ownership on the next event loop iteration.
    window.on_next_frame(move |window, cx| {
        if cx.read_from_clipboard().and_then(|item| item.text()).as_deref() == Some(text.as_str()) {
            Toast::show(message, window.window_handle(), cx);
        }
    });
    window.refresh();
}

pub use paxri_ui::components::{
    Button, ButtonVariant, Icon, ListItem, Select, actions, form_row, heading, row, single_line,
    tooltip,
};

#[derive(Clone, Copy)]
pub struct Ui(paxri_ui::components::Ui);

impl Ui {
    pub fn new(window: &Window, cx: &App) -> Self {
        Self(paxri_ui::components::Ui::new(window, cx))
    }
}
impl std::ops::Deref for Ui {
    type Target = paxri_ui::components::Ui;
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
