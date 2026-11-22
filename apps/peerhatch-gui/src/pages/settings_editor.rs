//! The settings index and the editor use the same rows.
use crate::{
    Dashboard, Input,
    components::{
        ButtonVariant::Ghost,
        Icon, SettingsGroup, SettingsSection, SettingsTable, Ui, actions,
        dock::{DockNavigation, DockPanel},
        single_line,
    },
    theme::{SPACE_1, SPACE_2, SPACE_3, SPACE_4},
};
use gpui::{prelude::*, *};

use super::settings_catalog::{GATEWAY, page_for};

struct SettingsPage {
    section: SettingsSection,
    groups: Vec<SettingsGroup>,
}

impl SettingsPage {
    fn collect(groups: Vec<SettingsGroup>) -> Vec<Self> {
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

    fn filter(&mut self, query: &str) {
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

    fn count(&self) -> usize {
        self.groups.iter().map(SettingsGroup::len).sum()
    }

    fn render(self, ui: Ui) -> Div {
        let section = self.section;
        let title = actions()
            .child(
                div()
                    .text_2xl()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(section.title),
            )
            .when(section.table != SettingsTable::Runtime, |heading| {
                heading.child(ui.muted(section.table.label()).text_xs())
            });
        div()
            .min_w_0()
            .flex()
            .flex_col()
            .gap(SPACE_3)
            .child(title)
            .children(
                self.groups
                    .into_iter()
                    .map(|group| group.render(ui, section)),
            )
    }
}

pub(crate) struct SettingsEditor {
    pub(crate) search: Entity<Input>,
    query: String,
    category: SettingsTable,
    search_category: Option<SettingsTable>,
    scroll: ScrollHandle,
}

impl SettingsEditor {
    pub(crate) fn new(cx: &mut Context<Dashboard>) -> Self {
        let search = Input::search("Search settings", cx);
        cx.observe(&search, |dashboard, field, cx| {
            let query = field.read(cx).text().trim().to_lowercase();
            let editor = &mut dashboard.settings_editor;
            if editor.query != query {
                editor.query = query;
                editor.search_category = None;
                editor.scroll.set_offset(Point::default());
                cx.notify();
            }
        })
        .detach();
        Self {
            search,
            query: String::new(),
            category: GATEWAY.table,
            search_category: None,
            scroll: ScrollHandle::new(),
        }
    }

    pub(crate) fn reset(&mut self, cx: &mut App) {
        self.search.update(cx, |field, cx| field.set_text("", cx));
        self.query.clear();
        self.category = GATEWAY.table;
        self.search_category = None;
        self.scroll.set_offset(Point::default());
    }
}

impl Dashboard {
    fn settings_nav_button(
        &self,
        ui: Ui,
        item: Option<SettingsSection>,
        selected: bool,
        count: Option<usize>,
        cx: &Context<Self>,
    ) -> crate::components::Button {
        let category = item.map(|section| section.table);
        let (label, icon) = item
            .map(|s| (s.title, s.icon))
            .unwrap_or(("All results", Icon::Search));
        let id = (
            ElementId::from("settings-category"),
            category
                .map(SettingsTable::label)
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
            .when_some(count, |d, n| d.child(ui.muted(n.to_string()).text_xs()));
        ui.button(Ghost, id, text)
            .row()
            .icon(icon)
            .pressed(selected)
            .on_click(cx.listener(move |dashboard, _, _, cx| {
                let editor = &mut dashboard.settings_editor;
                if !editor.query.is_empty() {
                    editor.search_category = category;
                } else if let Some(category) = category {
                    editor.category = category;
                }
                editor.scroll.set_offset(Point::default());
                cx.notify();
            }))
    }

    pub(crate) fn settings(&self, ui: Ui, width: Pixels, cx: &mut Context<Self>) -> DockPanel {
        let editor = &self.settings_editor;
        let searching = !editor.query.is_empty();
        let (category, groups) =
            self.settings_groups(ui, cx, (!searching).then_some(editor.category));
        let mut pages = SettingsPage::collect(groups);
        if searching {
            for page in &mut pages {
                page.filter(&editor.query);
            }
            pages.retain(|page| !page.groups.is_empty());
        }
        let count = pages.iter().map(SettingsPage::count).sum();
        let selected = if searching {
            editor
                .search_category
                .filter(|c| pages.iter().any(|p| p.section.table == *c))
        } else {
            category
        };
        let compact = width < px(560.);
        let mut navigation = div()
            .id("settings-navigation")
            .min_w_0()
            .min_h_0()
            .flex_1()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap(SPACE_1);
        if searching {
            navigation = navigation.child(self.settings_nav_button(
                ui,
                None,
                selected.is_none(),
                Some(count),
                cx,
            ));
        }
        for page in &pages {
            if page.section.table == SettingsTable::Runtime && pages.len() > 1 {
                navigation = navigation.child(ui.separator());
            }
            navigation = navigation.child(self.settings_nav_button(
                ui,
                Some(page.section),
                selected == Some(page.section.table),
                searching.then(|| page.count()),
                cx,
            ));
        }
        let mut content = div()
            .w_full()
            .min_w_0()
            .max_w(px(840.))
            .mx_auto()
            .flex()
            .flex_col()
            .gap(SPACE_4)
            .when(!compact, |d| d.px(SPACE_4).py(SPACE_2));
        if pages.is_empty() {
            content = content
                .child(
                    div()
                        .text_lg()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child("No matching settings"),
                )
                .child(
                    div().flex().child(
                        ui.button(Ghost, "reset-empty-search", "Clear search")
                            .on_click(cx.listener(Self::clear_settings_search)),
                    ),
                );
        }
        for page in pages {
            if selected.is_none_or(|c| c == page.section.table) {
                content = content.child(page.render(ui));
            }
        }

        let mut navigation = DockNavigation::new("Settings", navigation);
        if let Some(path) = self.config_path.clone() {
            navigation = navigation.action(
                ui.icon_button(
                    Ghost,
                    "open-config",
                    Icon::File,
                    format!("Open config file · {}", path.display()),
                )
                .on_click(cx.listener(move |v, _, _, cx| v.open_config_file(path.clone(), cx))),
            );
        }
        let toolbar = div()
            .flex_none()
            .flex()
            .justify_center()
            .px(SPACE_4)
            .py(SPACE_3)
            .border_b_1()
            .border_color(rgb(ui.colors.border))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .max_w(px(760.))
                    .child(editor.search.clone()),
            );
        DockPanel::fill(
            div()
                .size_full()
                .min_h_0()
                .flex()
                .flex_col()
                .child(toolbar)
                .child(DockPanel::scroll(
                    "settings-scroll",
                    editor.scroll.clone(),
                    content,
                )),
        )
        .with_navigation(navigation)
    }

    fn clear_settings_search(
        &mut self,
        _: &ClickEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.settings_editor
            .search
            .update(cx, |s, cx| s.set_text("", cx));
        cx.focus_view(&self.settings_editor.search, window);
    }
}

#[cfg(test)]
mod tests {
    use super::SettingsPage;
    use crate::{components::SettingsGroup, pages::settings_catalog as catalog};

    #[test]
    fn interface_collects_sibling_settings_and_counts_only_matching_rows() {
        let mut appearance = SettingsGroup::new(catalog::APPEARANCE);
        appearance.setting(catalog::THEME, gpui::Empty);
        let mut terminal = SettingsGroup::new(catalog::TERMINAL);
        terminal.setting(catalog::CURSOR_SHAPE, gpui::Empty);
        let network = SettingsGroup::new(catalog::NETWORK);
        let mut pages = SettingsPage::collect(vec![appearance, network, terminal]);
        assert_eq!(pages.len(), 2);
        let interface = &mut pages[0];
        assert_eq!(interface.section.title, "Interface");
        assert_eq!(interface.count(), 2);
        interface.filter("interface gui.terminal.cursor_shape");
        assert_eq!(interface.count(), 1);
        assert_eq!(interface.groups[0].section.title, "Terminal");
    }
}
