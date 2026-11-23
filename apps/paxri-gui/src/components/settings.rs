//! Desktop-specific input controls for the shared settings rows.
use super::{Input, Select, Ui, input::EditingFinished};
use crate::theme::SPACE_1;
use gpui::{prelude::*, *};
use std::collections::VecDeque;

pub(crate) use paxri_ui::settings::controls::setting_field;

impl Ui {
    pub fn setting_choice<T: Clone + PartialEq + 'static, L: Into<SharedString>>(
        self,
        id: &'static str,
        selected: T,
        values: impl IntoIterator<Item = T>,
        label: impl Fn(T) -> L,
        change: impl Fn(&T, &mut Window, &mut App) + 'static,
    ) -> Div {
        let mut select = Select::new(id, selected, change);
        for value in values {
            let name = label(value.clone());
            select = select.option(value, name);
        }
        setting_field(select)
    }

    pub fn setting_toggle(
        self,
        id: &'static str,
        enabled: bool,
        editable: bool,
        change: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Div {
        if editable {
            div()
                .flex()
                .child(self.switch(id, enabled).on_click(change))
        } else {
            self.muted(if enabled { "On" } else { "Off" })
                .whitespace_nowrap()
        }
    }
}

/// An editable preference that commits on Enter or blur and reconciles gateway replies.
pub(crate) struct TextSetting {
    input: Entity<Input>,
    saved: String,
    pending: VecDeque<String>,
    error: Option<String>,
}

impl TextSetting {
    pub fn new<V: 'static>(
        placeholder: &str,
        commit: impl Fn(&mut V, &mut Context<V>) + 'static,
        cx: &mut Context<V>,
    ) -> Self {
        let input = Input::setting(placeholder, cx);
        cx.subscribe(&input, move |view, _, _: &EditingFinished, cx| {
            commit(view, cx);
            cx.notify();
        })
        .detach();
        Self {
            input,
            saved: String::new(),
            pending: VecDeque::new(),
            error: None,
        }
    }

    pub fn render(&self, ui: Ui) -> Div {
        setting_field(
            div()
                .flex()
                .flex_col()
                .gap(SPACE_1)
                .child(self.input.clone())
                .when_some(self.error.clone(), |field, error| {
                    field.child(ui.alert(error))
                }),
        )
    }

    pub fn set_placeholder(&self, placeholder: &str, cx: &mut App) {
        self.input
            .update(cx, |field, cx| field.set_placeholder(placeholder, cx));
    }

    pub fn sync(&mut self, value: &str, cx: &mut App) {
        if self.saved == value {
            return;
        }
        self.input.update(cx, |field, cx| {
            if self.pending.is_empty() && field.text() == self.saved && !field.is_composing() {
                field.set_text(value, cx);
            }
        });
        self.saved = value.to_owned();
    }

    pub fn changed_value(&mut self, cx: &mut App) -> Option<String> {
        let value = self.input.read(cx).text().trim().to_owned();
        self.error = None;
        if value == *self.pending.back().unwrap_or(&self.saved) {
            self.input
                .update(cx, |field, cx| field.set_text(&value, cx));
            return None;
        }
        self.pending.push_back(value.clone());
        Some(value)
    }

    pub fn finish(&mut self, outcome: &Result<(), String>, cx: &mut App) {
        let Some(sent) = self.pending.pop_front() else {
            return;
        };
        let field = self.input.read(cx);
        if !self.pending.is_empty() || field.is_composing() || field.text().trim() != sent {
            return;
        }
        match outcome {
            Ok(()) => {
                self.input
                    .update(cx, |field, cx| field.set_text(&self.saved, cx));
            }
            Err(error) => self.error = Some(error.clone()),
        }
    }
}
