mod button;
pub mod caret;
mod controls;
pub mod directory;
mod dropdown;
pub use devices::DeviceCounts;
mod filter_toolbar;
pub use filter_toolbar::FilterToolbar;
mod select;
mod shortcut_recorder;
pub use shortcut_recorder::ShortcutRecorder;
pub use dropdown::{ActionsMenu, ActiveDropdown, Dropdown, DropdownContent, DropdownState, DropdownTrigger};
pub use select::Select;
pub mod focus;
pub mod input;
mod icons;
mod list_item;
mod list_view;
pub use list_view::ListView;
mod scrollbar;
pub use scrollbar::{ScrollArea, ScrollMetrics, ScrollSource, Scrollbar};
mod list_state;
pub use list_state::ListState;
pub mod forwarding;
mod tooltip;
mod toast;
pub use toast::Toast;
use crate::theme::{self, CONTROL_RADIUS, Palette, SPACE_1, SPACE_2, SPACE_3, SPACE_4};
pub use button::{Button, ButtonVariant, FLOATING_ACTION_CLEARANCE, floating_corner};
use gpui::{prelude::*, *};
pub use icons::Icon;
pub use list_item::ListItem;
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

    pub fn section(self) -> Div {
        div()
            .w_full()
            .min_w_0()
            .flex_none()
            .flex()
            .flex_col()
            .gap(SPACE_3)
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

    pub fn labeled(self, label: impl Into<SharedString>, control: impl IntoElement) -> Div {
        div()
            .flex()
            .flex_col()
            .gap(SPACE_1)
            .flex_1()
            .min_w(px(165.))
            .child(self.muted(label))
            .child(control)
    }

    pub fn device_icon(self, platform: paxri_config::DevicePlatform) -> Div {
        self.icon_tile(Icon::device(platform), 34., 20.)
    }

    pub fn icon_tile(self, icon: Icon, size: f32, glyph: f32) -> Div {
        div()
            .size(px(size))
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .rounded(CONTROL_RADIUS)
            .bg(rgb(self.colors.accent.background))
            .text_color(rgb(self.colors.accent.foreground))
            .child(icon.draw(glyph))
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

    pub fn status_badge(self, label: &'static str, dot: u32) -> Div {
        div()
            .flex()
            .items_center()
            .gap(SPACE_2)
            .flex_none()
            .px(SPACE_2)
            .py(SPACE_1)
            .rounded_full()
            .border_1()
            .border_color(rgb(self.colors.border))
            .bg(rgb(self.colors.card))
            .text_xs()
            .font_weight(FontWeight::MEDIUM)
            .child(div().size(px(7.)).rounded_full().bg(rgb(dot)))
            .child(label)
    }
}

pub fn heading(text: impl Into<SharedString>) -> Div {
    div()
        .text_lg()
        .font_weight(FontWeight::SEMIBOLD)
        .child(text.into())
}

/// Clip painted glyphs without rewriting the UTF-8 text or its native font runs.
pub fn single_line<E: Styled>(element: E) -> E {
    element.min_w_0().whitespace_nowrap().overflow_hidden()
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

pub fn byte_at_utf16(text: &str, offset: usize) -> usize {
    let mut units = 0;
    for (byte, c) in text.char_indices() {
        if units + c.len_utf16() > offset {
            return byte;
        }
        units += c.len_utf16();
    }
    text.len()
}

mod devices;
