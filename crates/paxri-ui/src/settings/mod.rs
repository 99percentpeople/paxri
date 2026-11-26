//! Configuration sections and rows shared by the desktop and mobile settings screens.
pub mod catalog;
pub mod controls;
pub mod terminal;
pub mod shortcuts;
use crate::components::{Icon, Ui, actions, row};
use crate::theme::{SPACE_1, SPACE_2, SPACE_3, SPACE_4, TextRole, TypographyExt};
use catalog::page_for;
use gpui::{prelude::*, *};

pub const MAX_WIDTH: Pixels = px(840.);

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SettingsLayout {
    Desktop,
    Mobile,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SettingsTable {
    Root,
    Named(&'static str),
}

impl SettingsTable {
    pub fn heading_label(self) -> Option<SharedString> {
        matches!(self, Self::Named(_)).then(|| self.label())
    }

    pub fn label(self) -> SharedString {
        match self {
            Self::Root => "Top level".into(),
            Self::Named(name) => format!("[{name}]").into(),
        }
    }

    pub fn top_level(self) -> Self {
        match self {
            Self::Named(path) => Self::Named(path.split_once('.').map_or(path, |(name, _)| name)),
            table => table,
        }
    }

    fn path(self, key: &str) -> Option<String> {
        if key.is_empty() {
            return None;
        }
        match self {
            Self::Root => Some(key.to_owned()),
            Self::Named(table) => Some(format!("{table}.{key}")),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct SettingsSection {
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
pub struct SettingSpec {
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

pub struct SettingRow {
    title: SharedString,
    description: SharedString,
    path: Option<String>,
    search: String,
    control: AnyElement,
    pub details: Option<AnyElement>,
    subsection: Option<&'static str>,
}

impl SettingRow {
    fn render(self, ui: Ui, separator: bool) -> Div {
        let title = actions()
            .child(
                div()
                    .text_role(TextRole::Subtitle)
                    .whitespace_nowrap()
                    .child(self.title),
            )
            .when_some(self.path, |d, path| d.child(ui.caption(path)));
        let copy = div()
            .min_w_0()
            .max_w_full()
            .flex_1()
            .flex_basis(px(220.))
            .flex()
            .flex_col()
            .gap(SPACE_1)
            .child(title)
            .when(!self.description.is_empty(), |d| {
                d.child(ui.muted(self.description))
            });

        let control = div()
            .min_w_0()
            .max_w(px(440.))
            .flex_initial()
            .flex()
            .justify_end()
            .ml_auto();
        let header = row()
            .w_full()
            .gap_x(SPACE_3)
            .gap_y(SPACE_2)
            .child(copy)
            .child(control.child(self.control));
        div()
            .w_full()
            .min_w_0()
            .flex_none()
            .flex()
            .flex_col()
            .py(if cfg!(target_os = "android") {
                SPACE_3
            } else {
                SPACE_4
            })
            .when(separator, |row| {
                row.border_t_1().border_color(rgb(ui.colors.border))
            })
            .child(header)
            .when_some(self.details, |row, details| {
                row.child(
                    div()
                        .w_full()
                        .min_w_0()
                        .mt(SPACE_3)
                        .flex()
                        .flex_col()
                        .child(details),
                )
            })
    }
}

pub struct SettingsGroup {
    pub section: SettingsSection,
    rows: Vec<SettingRow>,
    pub notice: Option<AnyElement>,
    pub footer: Option<AnyElement>,
}

impl SettingsGroup {
    pub fn device(name_field: impl IntoElement) -> Self {
        let mut group = Self::new(catalog::DEVICE);
        group.setting(catalog::DEVICE_NAME, name_field);
        group
    }

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

    /// A form field displayed like a preference, without a configuration path.
    pub fn field(&mut self, title: impl Into<SharedString>, control: impl IntoElement) {
        self.add("", title, "", "", controls::setting_field(control));
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

    pub fn render_rows(self, ui: Ui) -> impl IntoElement {
        let section = self.section;
        self.render(ui, section)
    }

    fn render(self, ui: Ui, page: SettingsSection) -> impl IntoElement {
        let section_needs_title =
            self.section.title != page.title || self.section.table != page.table;
        let has_intro = section_needs_title || self.notice.is_some();
        let mut section = div()
            .id((
                ElementId::from("settings-table"),
                self.section.table.label(),
            ))
            .w_full()
            .min_w_0()
            .flex()
            .flex_col()
            .when(section_needs_title, |body| {
                body.child(
                    actions()
                        .pb(SPACE_2)
                        .child(
                            div()
                                .text_role(TextRole::Section)
                                .child(self.section.title),
                        )
                        .when(self.section.table != page.table, |heading| {
                            heading.child(ui.caption(self.section.table.label()))
                        }),
                )
            })
            .children(self.notice);
        let mut last_subsection = None;
        for (index, row) in self.rows.into_iter().enumerate() {
            if let Some(heading) = row.subsection
                && last_subsection != Some(heading)
            {
                section = section.child(
                    div()
                        .when(index > 0 || has_intro, |d| d.pt(SPACE_4))
                        .pb(SPACE_2)
                        .text_role(TextRole::Subtitle)
                        .child(heading),
                );
                last_subsection = Some(heading);
            }
            let separator = index > 0 || has_intro || row.subsection.is_some();
            section = section.child(row.render(ui, separator));
        }
        section.children(self.footer)
    }
}

pub struct SettingsPage {
    pub section: SettingsSection,
    pub groups: Vec<SettingsGroup>,
}

impl SettingsPage {
    pub fn collect(groups: Vec<SettingsGroup>) -> Vec<Self> {
        let mut pages: Vec<Self> = Vec::new();
        for group in groups {
            let section = page_for(group.section);
            if let Some(page) = pages
                .iter_mut()
                .find(|page| page.section.table == section.table)
            {
                page.groups.push(group);
            } else {
                pages.push(Self {
                    section,
                    groups: vec![group],
                });
            }
        }
        pages
    }

    pub fn filter(&mut self, query: &str) {
        let heading =
            format!("{} {}", self.section.title, self.section.table.label()).to_lowercase();
        let words: Vec<_> = query
            .split_whitespace()
            .filter(|w| !heading.contains(w))
            .collect();
        for group in &mut self.groups {
            group.filter(&words);
        }
        self.groups.retain(|group| group.len() > 0);
    }

    pub fn count(&self) -> usize {
        self.groups.iter().map(SettingsGroup::len).sum()
    }

    pub fn render(self, ui: Ui, layout: SettingsLayout) -> Div {
        let section = self.section;
        let title = actions()
            .child(
                div()
                    .text_role(TextRole::PageTitle)
                    .child(section.title),
            )
            .when_some(section.table.heading_label(), |heading, label| {
                heading.child(ui.caption(label))
            });
        div()
            .w_full()
            .min_w_0()
            .flex()
            .flex_col()
            .gap(SPACE_3)
            .when(layout == SettingsLayout::Desktop, |body| body.child(title))
            .children(
                self.groups
                    .into_iter()
                    .map(|group| group.render(ui, section)),
            )
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
        assert!(super::catalog::GATEWAY.table.path("").is_none());
    }
}
