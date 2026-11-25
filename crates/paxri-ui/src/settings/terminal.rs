//! Terminal preferences shared by the desktop and Android settings pages.
use super::{
    SettingsGroup, catalog,
    controls::{number_field, preview_label, scheme_select, setting_field},
};
use crate::{
    components::{
        Select, Ui,
        input::{EditingFinished, Input, NumberFormat, NumberStep},
    },
    terminal::preview::TerminalPreview,
    theme::{self, CursorShape, TerminalAppearance, fonts::MonospaceFonts},
};
use gpui::{prelude::*, *};
use paxri_config::gui::GuiChange;
use std::rc::Rc;

#[derive(Clone, Copy)]
enum Metric {
    Size,
    Height,
    Spacing,
}

impl Metric {
    fn format(self) -> NumberFormat {
        match self {
            Self::Size => NumberFormat::Decimal,
            Self::Height => NumberFormat::Integer,
            Self::Spacing => NumberFormat::SignedInteger,
        }
    }

    fn value(self, style: &TerminalAppearance) -> String {
        match self {
            Self::Size => style.font_size.map(|v| v.to_string()).unwrap_or_default(),
            Self::Height => style.line_height.map(|v| v.to_string()).unwrap_or_default(),
            Self::Spacing => style.letter_spacing.to_string(),
        }
    }

    fn placeholder(self, style: &TerminalAppearance) -> String {
        match self {
            Self::Size => TerminalAppearance::DEFAULT_FONT_SIZE.to_string(),
            Self::Height => {
                let automatic = TerminalAppearance::default().row_height(style.size());
                format!("{automatic:.1}").trim_end_matches(".0").to_owned()
            }
            Self::Spacing => "0".into(),
        }
    }

    fn step(self, draft: &str, count: i32, style: &TerminalAppearance) -> String {
        let (base, min, max) = match self {
            Self::Size => (
                style.size(),
                *TerminalAppearance::FONT_SIZES.start(),
                *TerminalAppearance::FONT_SIZES.end(),
            ),
            Self::Height => (
                style.row_height(style.size()).round(),
                f32::from(*TerminalAppearance::LINE_HEIGHTS.start()),
                f32::from(*TerminalAppearance::LINE_HEIGHTS.end()),
            ),
            Self::Spacing => (
                f32::from(style.letter_spacing),
                f32::from(*TerminalAppearance::LETTER_SPACINGS.start()),
                f32::from(*TerminalAppearance::LETTER_SPACINGS.end()),
            ),
        };
        let current = draft
            .parse::<f32>()
            .ok()
            .filter(|v| v.is_finite() && self.change(draft).is_ok())
            .unwrap_or(base);
        (current + count as f32).clamp(min, max).to_string()
    }

    fn change(self, value: &str) -> Result<GuiChange, &'static str> {
        match self {
            Self::Size if value.is_empty() => Ok(GuiChange::FontSize(None)),
            Self::Height if value.is_empty() => Ok(GuiChange::LineHeight(None)),
            Self::Spacing if value.is_empty() => Ok(GuiChange::LetterSpacing(0)),
            Self::Size => value
                .parse()
                .ok()
                .filter(|v| TerminalAppearance::FONT_SIZES.contains(v))
                .map(|v| GuiChange::FontSize(Some(v)))
                .ok_or("Enter a size from 8 to 30 px."),
            Self::Height => value
                .parse()
                .ok()
                .filter(|v| TerminalAppearance::LINE_HEIGHTS.contains(v))
                .map(|v| GuiChange::LineHeight(Some(v)))
                .ok_or("Enter a whole number from 8 to 40 px."),
            Self::Spacing => value
                .parse()
                .ok()
                .filter(|v| TerminalAppearance::LETTER_SPACINGS.contains(v))
                .map(GuiChange::LetterSpacing)
                .ok_or("Enter a whole number from −1 to 4 px."),
        }
    }
}

pub struct NumberField {
    pub input: Entity<Input>,
    metric: Metric,
    saved: String,
    error: Option<&'static str>,
}

impl EventEmitter<GuiChange> for NumberField {}

impl NumberField {
    fn new(metric: Metric, cx: &mut Context<Self>) -> Self {
        let input = Input::number(metric.format(), "px", cx);
        let saved = metric.value(&theme::current(cx).terminal);
        input.update(cx, |input, cx| input.set_text(&saved, cx));
        cx.subscribe(&input, |field, _, _: &EditingFinished, cx| field.commit(cx))
            .detach();
        cx.subscribe(&input, |field, input, step: &NumberStep, cx| {
            let value =
                field
                    .metric
                    .step(input.read(cx).text(), step.0, &theme::current(cx).terminal);
            if input.read(cx).text() != value {
                input.update(cx, |input, cx| input.set_text(&value, cx));
                field.error = None;
                if let Ok(change) = field.metric.change(&value) {
                    cx.emit(change);
                }
                cx.notify();
            }
        })
        .detach();
        Self {
            input,
            metric,
            saved,
            error: None,
        }
    }

    pub fn commit(&mut self, cx: &mut Context<Self>) {
        let input = self.input.read(cx);
        if input.is_composing() {
            return;
        }
        let text = input.text().trim();
        let saved = self.metric.value(&theme::current(cx).terminal);
        match self.metric.change(text) {
            Ok(change) => {
                self.error = None;
                if self.metric.change(&saved) != Ok(change.clone()) {
                    cx.emit(change);
                } else {
                    self.input
                        .update(cx, |input, cx| input.set_text(&saved, cx));
                }
            }
            Err(message) => self.error = Some(message),
        }
        cx.notify();
    }
}

impl Render for NumberField {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let style = theme::current(cx).terminal;
        let saved = self.metric.value(&style);
        let placeholder = self.metric.placeholder(&style);
        self.input.update(cx, |input, cx| {
            input.set_placeholder(&placeholder, cx);
            if saved != self.saved
                && !input.is_composing()
                && (input.text() == self.saved
                    || self.metric.change(input.text()) == self.metric.change(&saved))
            {
                input.set_text(&saved, cx);
                self.error = None;
            }
        });
        self.saved = saved;
        let ui = Ui::new(window, cx);
        div()
            .max_w_full()
            .flex()
            .flex_col()
            .items_end()
            .gap_1()
            .child(number_field(self.input.clone()))
            .when_some(self.error, |field, message| {
                field.child(div().w(px(168.)).max_w_full().child(ui.alert(message)))
            })
    }
}

pub struct TerminalSettings {
    pub numbers: [Entity<NumberField>; 3],
}

impl TerminalSettings {
    pub fn new<V: 'static>(
        change: fn(&mut V, GuiChange, &mut Context<V>),
        cx: &mut Context<V>,
    ) -> Self {
        let numbers = [Metric::Size, Metric::Height, Metric::Spacing].map(|metric| {
            let field = cx.new(|cx| NumberField::new(metric, cx));
            cx.subscribe(&field, move |view, _, event: &GuiChange, cx| {
                change(view, event.clone(), cx)
            })
            .detach();
            field
        });
        Self { numbers }
    }

    pub fn group(
        &self,
        ui: Ui,
        cx: &App,
        change: impl Fn(GuiChange, &mut Window, &mut App) + 'static,
    ) -> SettingsGroup {
        let change = Rc::new(change);
        let style = theme::current(cx).terminal;
        let fonts = cx.global::<MonospaceFonts>();
        let mut group = SettingsGroup::new(catalog::TERMINAL);
        group.notice =
            Some(TerminalPreview(style.clone(), fonts.resolve(&style)).into_any_element());

        let on_scheme = change.clone();
        let schemes = scheme_select(ui, style.scheme, move |v, w, cx| {
            on_scheme(GuiChange::Scheme(*v), w, cx)
        });
        group.grouped_setting("Text", catalog::SCHEME, setting_field(schemes));

        let selected = fonts
            .custom_face(&style.font_family)
            .map(|face| face.font.family.clone())
            .unwrap_or_default();
        let on_font = change.clone();
        let mut families = Select::new(
            "terminal-font-family",
            selected,
            move |v: &SharedString, w, cx| on_font(GuiChange::FontFamily(v.to_string()), w, cx),
        );
        for (value, font) in fonts.choices() {
            let font = font.clone();
            families = families.option_with_content(value, move || {
                div()
                    .min_w_0()
                    .truncate()
                    .font(font.clone())
                    .child(font.family.clone())
            });
        }
        group.grouped_setting("Text", catalog::FONT_FAMILY, setting_field(families));
        for (spec, field) in [
            catalog::FONT_SIZE,
            catalog::LINE_HEIGHT,
            catalog::LETTER_SPACING,
        ]
        .into_iter()
        .zip(&self.numbers)
        {
            group.grouped_setting("Text", spec, field.clone());
        }

        let on_shape = change.clone();
        let mut shapes = Select::new(
            "terminal-cursor-shape",
            style.cursor_shape,
            move |v: &CursorShape, w, cx| on_shape(GuiChange::CursorShape(*v), w, cx),
        );
        for shape in CursorShape::ALL {
            shapes = shapes.option_with_content(shape, move || {
                let mark = div()
                    .absolute()
                    .left_0()
                    .bg(rgb(ui.colors.canvas.foreground));
                let mark = match shape {
                    CursorShape::Underline => mark.bottom_0().w_full().h(px(2.)),
                    CursorShape::Bar => mark.top_0().w(px(2.)).h_full(),
                    CursorShape::Block => mark.top_0().size_full(),
                };
                preview_label(
                    div()
                        .relative()
                        .w(px(10.))
                        .h(px(14.))
                        .flex_none()
                        .child(mark),
                    shape.label(),
                )
            });
        }
        group.grouped_setting("Cursor", catalog::CURSOR_SHAPE, setting_field(shapes));
        group.grouped_setting(
            "Cursor",
            catalog::CURSOR_BLINK,
            div().flex().child(
                ui.switch("terminal-cursor-blink", style.blink_cursor)
                    .on_click(move |_, w, cx| {
                        change(GuiChange::BlinkCursor(!style.blink_cursor), w, cx)
                    }),
            ),
        );
        group
    }
}
