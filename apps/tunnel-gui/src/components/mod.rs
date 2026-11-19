//! Reusable GPUI controls, layouts and icons for workspace pages.
mod button;
mod controls;
pub mod dock;
mod dropdown;
mod icons;
mod input;
mod list_item;
mod scrollbar;
mod select;
mod tooltip;
use crate::theme::{
    self, CONTROL_RADIUS, PANEL_RADIUS, Palette, SPACE_1, SPACE_2, SPACE_3, SPACE_4,
};
pub use button::{Button, ButtonVariant, track_keyboard_focus};
pub use dropdown::{Dropdown, DropdownContent, DropdownState, DropdownTrigger};
use gpui::{prelude::*, *};
pub use icons::Icon;
pub use input::{Input, Textarea};
pub use list_item::ListItem;
pub use scrollbar::{ScrollMetrics, ScrollSource, Scrollbar};
pub use select::Select;
pub use tooltip::tooltip;

#[derive(Clone, Copy)]
pub struct Ui {
    pub colors: &'static Palette,
}

impl Ui {
    pub fn new(window: &Window, cx: &App) -> Self {
        Self {
            colors: theme::palette(window, cx),
        }
    }

    pub fn card(self) -> Div {
        div()
            .flex_none()
            .flex()
            .flex_col()
            .gap(SPACE_3)
            .p(SPACE_4)
            .rounded(PANEL_RADIUS)
            .border_1()
            .border_color(rgb(self.colors.border))
            .bg(rgb(self.colors.card))
            .text_color(rgb(self.colors.canvas.foreground))
    }

    pub fn section(self, title: &'static str) -> Div {
        self.card().child(heading(title))
    }

    pub fn separator(self) -> Div {
        div()
            .w_full()
            .h(px(1.))
            .flex_none()
            .bg(rgb(self.colors.border))
    }

    pub fn muted(self, text: impl Into<SharedString>) -> Div {
        div()
            .text_sm()
            .text_color(rgb(self.colors.muted_foreground))
            .child(text.into())
    }

    pub fn labeled(self, label: &'static str, control: impl IntoElement) -> Div {
        div()
            .flex()
            .flex_col()
            .gap(SPACE_1)
            .flex_1()
            .min_w(px(165.))
            .child(self.muted(label))
            .child(control)
    }

    pub fn device_icon(self, platform: tunnel_control::DevicePlatform) -> Div {
        div()
            .size(px(34.))
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .rounded(CONTROL_RADIUS)
            .bg(rgb(self.colors.accent.background))
            .text_color(rgb(self.colors.accent.foreground))
            .child(Icon::device(platform).draw(20.))
    }

    pub fn alert(self, message: impl Into<SharedString>) -> Div {
        div()
            .px(SPACE_4)
            .py(SPACE_2)
            .bg(rgb(self.colors.alert.background))
            .text_color(rgb(self.colors.alert.foreground))
            .text_sm()
            .child(message.into())
    }

    pub fn status_dot(self, connected: bool) -> Div {
        div()
            .flex_none()
            .size(px(7.))
            .rounded_full()
            .bg(rgb(if connected {
                self.colors.success
            } else {
                self.colors.warning
            }))
    }
}

pub fn page(title: impl Into<SharedString>) -> Div {
    div().flex().flex_col().gap(SPACE_4).child(
        div()
            .text_3xl()
            .font_weight(FontWeight::SEMIBOLD)
            .child(title.into()),
    )
}

pub fn heading(text: impl Into<SharedString>) -> Div {
    div()
        .text_lg()
        .font_weight(FontWeight::SEMIBOLD)
        .child(text.into())
}

pub fn row() -> Div {
    div()
        .flex()
        .flex_wrap()
        .gap(SPACE_2)
        .items_center()
        .justify_between()
}

pub fn actions() -> Div {
    div().flex().flex_wrap().items_center().gap(SPACE_2)
}

pub fn form_row() -> Div {
    div().flex().flex_wrap().gap(SPACE_3)
}
