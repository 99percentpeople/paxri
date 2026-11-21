//! Settings rows supply the content for both the form and its search index.
use super::{Icon, Input, Select, Ui, actions, input::EditingFinished, row};
use crate::theme::{SPACE_1, SPACE_2, SPACE_3, SPACE_4};
use gpui::{prelude::*, *};
use std::collections::VecDeque;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum SettingsTable {
    Root,
    Named(&'static str),
    Runtime,
}

impl SettingsTable {
    pub fn label(self) -> SharedString {
        match self {
            Self::Root => "Top level".into(),
            Self::Named(name) => format!("[{name}]").into(),
            Self::Runtime => "Runtime".into(),
        }
    }

    pub fn top_level(self) -> Self {
        match self {
            Self::Named(path) => Self::Named(path.split_once('.').map_or(path, |(name, _)| name)),
            table => table,
        }
    }

    fn path(self, key: &str) -> Option<String> {
        match self {
            Self::Root => Some(key.to_owned()),
            Self::Named(table) => Some(format!("{table}.{key}")),
            Self::Runtime => None,
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) struct SettingsSection {
    pub table: SettingsTable,
    pub title: &'static str,
    pub icon: Icon,
}

impl SettingsSection {
    pub const fn new(table: SettingsTable, title: &'static str, icon: Icon) -> Self {
        Self { table, title, icon }
    }
}

#[derive(Clone, Copy)]
pub(crate) struct SettingSpec {
    pub key: &'static str,
    title: &'static str,
    description: &'static str,
    keywords: &'static str,
}

impl SettingSpec {
    pub const fn new(
        key: &'static str,
        title: &'static str,
        description: &'static str,
        keywords: &'static str,
    ) -> Self {
        Self {
            key,
            title,
            description,
            keywords,
        }
    }
}

pub(crate) struct SettingRow {
    title: SharedString,
    description: SharedString,
    path: Option<String>,
    search: String,
    control: AnyElement,
    grow_value: bool,
    pub details: Option<AnyElement>,
    subsection: Option<&'static str>,
}

impl SettingRow {
    fn render(self, ui: Ui) -> Div {
        let title = actions()
            .child(
                div()
                    .text_sm()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(self.title),
            )
            .when_some(self.path, |d, path| d.child(ui.muted(path).text_xs()));
        let copy = div()
            .min_w_0()
            .flex_grow()
            .flex_basis(px(200.))
            .flex()
            .flex_col()
            .gap(SPACE_1)
            .child(title)
            .when(!self.description.is_empty(), |d| {
                d.child(ui.muted(self.description))
            });

        // Values share the available width with the label and fill a line after wrapping.
        // Interactive controls keep their own preferred width.
        let control = if self.grow_value {
            div().min_w_0().flex_grow().flex_basis(px(224.))
        } else {
            div().min_w_0().max_w(px(440.)).flex_initial()
        };
        let header = row()
            .w_full()
            .gap(SPACE_3)
            .child(copy)
            .child(control.ml_auto().child(self.control));
        div()
            .min_w_0()
            .flex_none()
            .py(SPACE_4)
            .border_t_1()
            .border_color(rgb(ui.colors.border))
            .child(header)
            .when_some(self.details, |row, details| {
                row.child(div().mt(SPACE_3).child(details))
            })
    }
}

pub(crate) struct SettingsGroup {
    pub section: SettingsSection,
    rows: Vec<SettingRow>,
    pub notice: Option<AnyElement>,
    pub footer: Option<AnyElement>,
}

impl SettingsGroup {
    pub fn new(section: SettingsSection) -> Self {
        Self {
            section,
            rows: Vec::new(),
            notice: None,
            footer: None,
        }
    }

    /// Configure one preference once; both the view and the search index use its descriptor.
    pub fn setting(&mut self, spec: SettingSpec, control: impl IntoElement) -> &mut SettingRow {
        self.add(
            spec.key,
            spec.title,
            spec.description,
            spec.keywords,
            control,
        )
    }

    pub fn grouped_setting(
        &mut self,
        heading: &'static str,
        spec: SettingSpec,
        control: impl IntoElement,
    ) {
        let row = self.setting(spec, control);
        row.subsection = Some(heading);
        row.search.push(' ');
        row.search.push_str(&heading.to_lowercase());
    }

    pub fn value(&mut self, key: &str, title: &str, value: &str, ui: Ui) {
        let row = self.add(
            key,
            title.to_owned(),
            "",
            value,
            ui.muted(value.to_owned()).text_right(),
        );
        row.grow_value = true;
    }

    pub fn add(
        &mut self,
        key: &str,
        title: impl Into<SharedString>,
        description: impl Into<SharedString>,
        keywords: &str,
        control: impl IntoElement,
    ) -> &mut SettingRow {
        let title = title.into();
        let description = description.into();
        let path = self.section.table.path(key);
        let search = format!(
            "{} {} {} {title} {description} {keywords}",
            self.section.title,
            self.section.table.label(),
            path.as_deref().unwrap_or_default()
        )
        .to_lowercase();
        self.rows.push_mut(SettingRow {
            title,
            description,
            path,
            search,
            control: control.into_any_element(),
            grow_value: false,
            details: None,
            subsection: None,
        })
    }

    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn filter(&mut self, words: &[&str]) {
        self.rows
            .retain(|row| words.iter().all(|word| row.search.contains(*word)));
    }

    pub fn render(self, ui: Ui, page: SettingsSection) -> impl IntoElement {
        let section_needs_title =
            self.section.title != page.title || self.section.table != page.table;
        let mut section = div()
            .id((
                ElementId::from("settings-table"),
                self.section.table.label(),
            ))
            .min_w_0()
            .flex()
            .flex_col()
            .when(section_needs_title, |body| {
                body.child(
                    actions()
                        .pb(SPACE_2)
                        .child(
                            div()
                                .text_lg()
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(self.section.title),
                        )
                        .when(self.section.table != page.table, |heading| {
                            heading.child(ui.muted(self.section.table.label()).text_xs())
                        }),
                )
            })
            .children(self.notice);
        let mut last_subsection = None;
        for row in self.rows {
            if let Some(heading) = row.subsection
                && last_subsection != Some(heading)
            {
                section = section.child(
                    div()
                        .pt(SPACE_4)
                        .pb(SPACE_2)
                        .text_sm()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(heading),
                );
                last_subsection = Some(heading);
            }
            section = section.child(row.render(ui));
        }
        section.children(self.footer)
    }
}

pub(crate) fn setting_field(control: impl IntoElement) -> Div {
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

#[cfg(test)]
mod tests {
    use super::{Icon, SettingSpec, SettingsGroup, SettingsSection, SettingsTable};
    #[test]
    fn settings_search_indexes_toml_paths_and_rendered_descriptions() {
        let mut group = SettingsGroup::new(SettingsSection::new(
            SettingsTable::Named("native"),
            "Network",
            Icon::Globe,
        ));
        group.add(
            "lan_port_mapping",
            "Physical LAN mapping",
            "Use the router when a VPN is active.",
            "nat-pmp",
            gpui::Empty,
        );
        group.setting(
            SettingSpec::new("upnp", "Allow UPnP", "Discover a router.", ""),
            gpui::Empty,
        );
        group.filter(&["network", "[native]", "vpn", "native.lan_port_mapping"]);
        assert_eq!(group.len(), 1);
        group.filter(&["blink"]);
        assert_eq!(group.len(), 0);
        assert_eq!(
            SettingsTable::Root.path("device_name").as_deref(),
            Some("device_name")
        );
        assert!(SettingsTable::Runtime.path("gateway").is_none());
    }
}
