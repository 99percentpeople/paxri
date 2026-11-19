use super::*;
use crate::components::{ButtonVariant::Ghost, Dropdown, DropdownContent, DropdownTrigger};

impl Workspace {
    pub(super) fn workspace_menu(&self, ui: Ui, cx: &Context<Self>) -> Div {
        let trigger = DropdownTrigger::new(
            ui.button(Ghost, "workspace-menu", "Menu")
                .disclosure(self.menu.read(cx).is_open()),
        );
        let show = |page: Page| {
            let workspace = cx.weak_entity();
            move |window: &mut Window, cx: &mut App| {
                let _ = workspace.update(cx, |view, cx| view.show_page(page.clone(), window, cx));
            }
        };
        let mut content = DropdownContent::new().item(
            "settings",
            Icon::Settings.label("Settings"),
            show(Page::Settings),
        );
        if !self.tabs.contains(&Page::Home) {
            content = content.item("home", Icon::Home.label("Open Home"), show(Page::Home));
        }
        let placement = self.dock.read(cx).placement().other();
        let label = match placement {
            DockPlacement::Top => Icon::TabsOnTop.label("Tabs on top"),
            DockPlacement::Left => Icon::TabsOnLeft.label("Tabs on left"),
        };
        let dock = self.dock.clone();
        let content = content.item("tab-placement", label, move |_, cx| {
            dock.update(cx, |dock, cx| dock.set_placement(placement, cx));
        });
        div()
            .flex_none()
            .px_1()
            .child(Dropdown::new(&self.menu, trigger, content))
    }
}
