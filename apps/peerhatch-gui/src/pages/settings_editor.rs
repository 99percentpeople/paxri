//! The settings index and the editor use the same rows, including service names supplied by the gateway.
use crate::{
    Dashboard, Input,
    components::{
        ButtonVariant::Ghost, Icon, SettingsGroup, SettingsSection, SettingsTable, Ui, actions,
        dock::DockPanel,
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
        compact: bool,
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
        let button = if compact {
            ui.icon_button(Ghost, id, icon, label)
        } else {
            let text = div()
                .flex_1()
                .min_w_0()
                .flex()
                .items_center()
                .justify_between()
                .child(div().truncate().child(label))
                .when_some(count, |d, n| d.child(ui.muted(n.to_string()).text_xs()));
            ui.button(Ghost, id, text).row().icon(icon)
        };
        button
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

    pub(crate) fn settings(&self, ui: Ui, width: Pixels, cx: &mut Context<Self>) -> Div {
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
            .h_full()
            .overflow_y_scroll()
            .flex_none()
            .w(px(if compact { 48. } else { 188. }))
            .px(SPACE_2)
            .py(SPACE_3)
            .border_r_1()
            .border_color(rgb(ui.colors.border))
            .flex()
            .flex_col()
            .gap(SPACE_1);
        if searching {
            navigation = navigation.child(self.settings_nav_button(
                ui,
                None,
                selected.is_none(),
                compact,
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
                compact,
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

        let toolbar = div()
            .flex_none()
            .flex()
            .items_center()
            .gap(SPACE_2)
            .px(SPACE_4)
            .py(SPACE_3)
            .border_b_1()
            .border_color(rgb(ui.colors.border))
            .when(!compact, |t| {
                t.child(
                    div()
                        .flex_1()
                        .min_w(px(164.))
                        .text_xl()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child("Settings"),
                )
            })
            .child(
                div()
                    .min_w_0()
                    .w(px(760.))
                    .when(compact, |field| field.flex_1())
                    .child(editor.search.clone()),
            )
            .when(!compact || self.config_path.is_some(), |t| {
                t.child(
                    div()
                        .flex()
                        .justify_end()
                        .when(!compact, |side| side.flex_1())
                        .when_some(self.config_path.clone(), |side, path| {
                            side.child(
                                ui.icon_button(
                                    Ghost,
                                    "open-config",
                                    Icon::File,
                                    format!("Open config file · {}", path.display()),
                                )
                                .on_click(cx.listener(
                                    move |v, _, _, cx| v.open_config_file(path.clone(), cx),
                                )),
                            )
                        }),
                )
            });
        div()
            .size_full()
            .min_h_0()
            .flex()
            .flex_col()
            .child(toolbar)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .min_h_0()
                    .flex()
                    .child(navigation)
                    .child(DockPanel::scroll(
                        "settings-scroll",
                        editor.scroll.clone(),
                        content,
                    )),
            )
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
