//! Buttons put their label, optional icons and GPUI event handlers on one focusable control.
use super::*;
use crate::theme::BUTTON_HEIGHT;

#[derive(Clone, Copy)]
pub enum ButtonVariant {
    Primary,
    Secondary,
    Ghost,
    Outline,
    Destructive,
}

enum ButtonLayout {
    Label,
    Icon,
    Field,
}

#[derive(IntoElement)]
pub struct Button {
    root: Stateful<Div>,
    colors: &'static Palette,
    variant: ButtonVariant,
    label: AnyElement,
    leading: Option<Icon>,
    trailing: Option<Icon>,
    pressed: bool,
    layout: ButtonLayout,
}

impl Ui {
    pub fn button(
        self,
        variant: ButtonVariant,
        id: impl Into<ElementId>,
        label: impl IntoElement,
    ) -> Button {
        Button {
            root: div().id(id).focusable().tab_stop(true),
            colors: self.colors,
            variant,
            label: label.into_any_element(),
            leading: None,
            trailing: None,
            pressed: false,
            layout: ButtonLayout::Label,
        }
    }

    pub fn icon_button(
        self,
        variant: ButtonVariant,
        id: impl Into<ElementId>,
        icon: Icon,
        tooltip: impl Into<SharedString>,
    ) -> Button {
        let mut button = self.button(variant, id, icon.draw(16.));
        button.layout = ButtonLayout::Icon;
        button.tooltip(super::tooltip(tooltip.into()))
    }
}

impl Button {
    pub fn icon(mut self, icon: Icon) -> Self {
        self.leading = Some(icon);
        self
    }

    pub(super) fn field(mut self) -> Self {
        self.layout = ButtonLayout::Field;
        self
    }

    /// Use the shared button's pressed appearance and trailing chevron for a disclosure.
    pub fn disclosure(mut self, expanded: bool) -> Self {
        self.trailing = Some(if expanded {
            Icon::ChevronUp
        } else {
            Icon::ChevronDown
        });
        self.pressed = expanded;
        self
    }
}

impl InteractiveElement for Button {
    fn interactivity(&mut self) -> &mut Interactivity {
        self.root.interactivity()
    }
}

impl StatefulInteractiveElement for Button {}

impl RenderOnce for Button {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let colors = self.colors;
        let (pair, hover) = match self.variant {
            ButtonVariant::Primary => (&colors.primary, colors.primary_hover),
            ButtonVariant::Secondary | ButtonVariant::Ghost => {
                (&colors.secondary, colors.secondary_hover)
            }
            ButtonVariant::Outline => (&colors.canvas, colors.accent.background),
            ButtonVariant::Destructive => (&colors.destructive, colors.destructive_hover),
        };
        let focus_border = match self.variant {
            ButtonVariant::Secondary | ButtonVariant::Ghost | ButtonVariant::Outline => colors.ring,
            _ => pair.foreground,
        };
        let background = match self.variant {
            _ if self.pressed => Some(hover),
            ButtonVariant::Ghost => None,
            ButtonVariant::Outline => Some(colors.card),
            _ => Some(pair.background),
        };
        let root = self
            .root
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .gap(SPACE_2)
            .h(BUTTON_HEIGHT);
        let root = match self.layout {
            ButtonLayout::Label => root.px(SPACE_3),
            ButtonLayout::Icon => root.w(BUTTON_HEIGHT),
            ButtonLayout::Field => root.w_full().h(px(36.)).px(SPACE_3).justify_between(),
        };
        root.rounded(CONTROL_RADIUS)
            .border_1()
            .border_color(transparent_black())
            .when(matches!(self.variant, ButtonVariant::Outline), |button| {
                button.border_color(rgb(colors.input_border))
            })
            .cursor_pointer()
            .whitespace_nowrap()
            .text_sm()
            .font_weight(FontWeight::MEDIUM)
            .when_some(background, |button, color| button.bg(rgb(color)))
            .text_color(rgb(pair.foreground))
            .hover(|s| s.bg(rgb(hover)))
            .when(!matches!(self.variant, ButtonVariant::Ghost), |button| {
                button.active(|s| s.bg(rgb(hover)))
            })
            .when(
                KeyboardFocus::window(cx) == Some(window.window_handle()),
                |button| button.focus(|s| s.border_color(rgb(focus_border))),
            )
            .when_some(self.leading, |button, icon| button.child(icon.draw(16.)))
            .child(self.label)
            .when_some(self.trailing, |button, icon| button.child(icon.draw(16.)))
    }
}

// GPUI's focus style also applies to initial and mouse focus. Keep the keyboard
// indicator on the window where the user actually navigated with the keyboard.
struct KeyboardFocus(Option<AnyWindowHandle>);
impl Global for KeyboardFocus {}

impl KeyboardFocus {
    fn window(cx: &App) -> Option<AnyWindowHandle> {
        cx.try_global::<Self>().and_then(|focus| focus.0)
    }

    fn set(window: Option<AnyWindowHandle>, cx: &mut App) {
        if Self::window(cx) != window {
            cx.set_global(Self(window));
            cx.refresh_windows();
        }
    }
}

pub fn track_keyboard_focus(root: Div) -> Div {
    root.capture_key_down(|event, window, cx| {
        if matches!(
            event.keystroke.key.as_str(),
            "tab" | "up" | "down" | "left" | "right" | "enter" | "space" | "escape"
        ) {
            KeyboardFocus::set(Some(window.window_handle()), cx);
        }
    })
    .capture_any_mouse_down(|_, _, cx| KeyboardFocus::set(None, cx))
}
