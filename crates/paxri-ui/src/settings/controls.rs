use crate::components::{Icon, Select, Ui};
use crate::theme::{self, Appearance, SPACE_1, SPACE_2, TerminalScheme};
use gpui::{prelude::*, *};

fn appearance_preview(mode: Appearance) -> Div {
    let (sidebar, page) = match mode {
        Appearance::System => (&theme::LIGHT, &theme::DARK),
        Appearance::Light => (&theme::LIGHT, &theme::LIGHT),
        Appearance::Dark => (&theme::DARK, &theme::DARK),
    };
    let line = |width, color| div().w(px(width)).h(px(3.)).rounded_full().bg(rgb(color));
    div()
        .w_full()
        .h(px(48.))
        .flex()
        .overflow_hidden()
        .rounded(SPACE_1)
        .border_1()
        .border_color(rgb(sidebar.input_border))
        .child(
            div()
                .w(px(35.))
                .flex_none()
                .flex()
                .flex_col()
                .gap(SPACE_1)
                .p(SPACE_1)
                .py(SPACE_2)
                .bg(rgb(sidebar.secondary.background))
                .child(line(21., sidebar.muted_foreground))
                .child(line(15., sidebar.input_border)),
        )
        .child(
            div()
                .flex_1()
                .flex()
                .flex_col()
                .gap(SPACE_1)
                .p(SPACE_2)
                .bg(rgb(page.canvas.background))
                .child(line(24., page.canvas.foreground))
                .child(line(36., page.input_border))
                .child(line(28., page.input_border)),
        )
}

pub fn theme_cards(
    ui: Ui,
    selected: Appearance,
    change: impl Fn(&Appearance, &mut Window, &mut App) + 'static,
) -> Div {
    let change = std::rc::Rc::new(change);
    div()
        .w(px(412.))
        .max_w_full()
        .grid()
        .grid_cols(Appearance::ALL.len() as u16)
        .gap(SPACE_2)
        .children(Appearance::ALL.into_iter().map(|mode| {
            let icon = match mode {
                Appearance::System => Icon::Monitor,
                Appearance::Light => Icon::Sun,
                Appearance::Dark => Icon::Moon,
            };
            ui.choice_card(
                ("theme", mode as usize),
                icon.label(mode.label()).gap(SPACE_1),
                appearance_preview(mode),
                selected == mode,
            )
            .on_click({
                let change = change.clone();
                move |_, window, cx| change(&mode, window, cx)
            })
        }))
}

pub fn preview_label(preview: Div, label: &'static str) -> Div {
    div()
        .min_w_0()
        .flex()
        .items_center()
        .gap(theme::SPACE_2)
        .child(preview)
        .child(div().min_w_0().truncate().child(label))
}

pub fn scheme_select(
    ui: Ui,
    selected: TerminalScheme,
    change: impl Fn(&TerminalScheme, &mut Window, &mut App) + 'static,
) -> Select<TerminalScheme> {
    let mut schemes = Select::new("terminal-scheme", selected, change);
    for scheme in TerminalScheme::ALL {
        let palette = theme::terminal_palette(scheme, ui.colors);
        schemes = schemes.option_with_content(scheme, move || {
            let swatch = div().flex_none().flex().flex_col().gap(px(1.)).children(
                palette.ansi.chunks_exact(8).map(|row| {
                    div().flex().gap(px(1.)).children(
                        row.iter()
                            .map(|&color| div().w(px(4.)).h(px(5.)).bg(rgb(color))),
                    )
                }),
            );
            preview_label(swatch, scheme.label())
        });
    }
    schemes
}

pub fn number_field(input: impl IntoElement) -> Div {
    div().w(px(108.)).max_w_full().child(input)
}

pub fn setting_field(control: impl IntoElement) -> Div {
    div().w(px(224.)).max_w_full().min_w_0().child(control)
}

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

pub fn category_button(
    ui: Ui,
    item: Option<super::SettingsSection>,
    selected: bool,
    count: Option<usize>,
    layout: super::SettingsLayout,
) -> crate::components::Button {
    use crate::components::{ButtonVariant::Ghost, single_line};
    let category = item.map(|section| section.table);
    let (label, icon) = item
        .map(|s| (s.title, s.icon))
        .unwrap_or(("All results", Icon::Search));
    let id = (
        ElementId::from("settings-category"),
        category
            .map(super::SettingsTable::label)
            .unwrap_or_else(|| "all".into()),
    );
    let text = div()
        .flex_1()
        .min_w_0()
        .flex()
        .items_center()
        .justify_between()
        .gap_1()
        .child(single_line(div()).child(label))
        .when(layout == super::SettingsLayout::Mobile, |row| {
            row.child(Icon::ChevronRight.draw(16.))
        })
        .when_some(count, |d, n| d.child(ui.caption(n.to_string())));
    ui.button(Ghost, id, text)
        .row()
        .icon(icon)
        .pressed(selected)
}
