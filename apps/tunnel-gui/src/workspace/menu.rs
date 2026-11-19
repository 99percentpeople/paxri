use super::*;

#[derive(Clone, Copy)]
enum MenuAction {
    Home,
    Settings,
    TabsOnLeft(bool),
}

impl MenuAction {
    fn label(self) -> (Icon, &'static str) {
        match self {
            Self::Home => (Icon::Home, "Open Home"),
            Self::Settings => (Icon::Settings, "Settings"),
            Self::TabsOnLeft(true) => (Icon::TabsOnLeft, "Tabs on left"),
            Self::TabsOnLeft(false) => (Icon::TabsOnTop, "Tabs on top"),
        }
    }
}

impl Workspace {
    fn menu_actions(&self) -> Vec<MenuAction> {
        let mut actions = vec![MenuAction::Settings];
        if !self.tabs.contains(&Page::Home) {
            actions.push(MenuAction::Home);
        }
        actions.push(MenuAction::TabsOnLeft(!self.sidebar));
        actions
    }

    pub(super) fn dismiss_menu(&mut self, cx: &mut Context<Self>) {
        if self.menu_index.take().is_some() {
            cx.notify();
        }
    }

    fn run_menu_action(&mut self, action: MenuAction, w: &mut Window, cx: &mut Context<Self>) {
        self.dismiss_menu(cx);
        match action {
            MenuAction::Home => self.show_page(Page::Home, w, cx),
            MenuAction::Settings => self.show_page(Page::Settings, w, cx),
            MenuAction::TabsOnLeft(left) => self.sidebar = left,
        }
    }

    fn menu_key(&mut self, event: &KeyDownEvent, w: &mut Window, cx: &mut Context<Self>) {
        let key = &event.keystroke;
        if key.modifiers.modified() && !(key.key == "tab" && key.modifiers == Modifiers::shift()) {
            return;
        }
        let actions = self.menu_actions();
        let last = actions.len() - 1; // Tab placement is always available.
        match (self.menu_index.map(|i| i.min(last)), key.key.as_str()) {
            (_, "tab") => {
                self.dismiss_menu(cx);
                if key.modifiers.shift {
                    w.focus_prev()
                } else {
                    w.focus_next()
                }
            }
            (None, "enter" | "space" | "down") if !event.is_held => self.menu_index = Some(0),
            (None, "up") => self.menu_index = Some(last),
            (Some(_), "escape") => self.dismiss_menu(cx),
            (Some(i), "down") => self.menu_index = Some((i + 1) % actions.len()),
            (Some(i), "up") => self.menu_index = Some(if i == 0 { last } else { i - 1 }),
            (Some(i), "enter" | "space") if !event.is_held => {
                self.run_menu_action(actions[i], w, cx)
            }
            _ => return,
        }
        w.prevent_default();
        cx.stop_propagation();
        cx.notify();
    }

    fn dropdown(&self, index: usize, w: &Window, cx: &Context<Self>) -> Deferred {
        let viewport = w.viewport_size();
        let actions = self.menu_actions();
        let selected = index.min(actions.len() - 1);
        let mut list = div()
            .id("workspace-menu-popup")
            .occlude()
            .flex()
            .flex_col()
            .w(px(234.).min((viewport.width - px(12.)).max(px(1.))))
            .max_h((viewport.height - px(48.)).max(px(32.)))
            .overflow_y_scroll()
            .p(px(5.))
            .rounded_lg()
            .border_1()
            .border_color(rgb(BORDER))
            .bg(rgb(0xffffff))
            .shadow_lg()
            .text_sm()
            .text_color(rgb(INK))
            .on_mouse_down_out(cx.listener(|v, _, _, cx| v.dismiss_menu(cx)));
        for (i, action) in actions.into_iter().enumerate() {
            let (icon, label) = action.label();
            list = list.child(
                div()
                    .id(label)
                    .flex()
                    .flex_none()
                    .items_center()
                    .w_full()
                    .h(px(34.))
                    .px_2()
                    .rounded_md()
                    .cursor_pointer()
                    .when(i == selected, |row| {
                        row.bg(rgb(0xe2f1ec)).text_color(rgb(0x165c4d))
                    })
                    .on_mouse_move(cx.listener(move |v, _, _, cx| {
                        if v.menu_index.is_some() && v.menu_index != Some(i) {
                            v.menu_index = Some(i);
                            cx.notify();
                        }
                    }))
                    .on_click(cx.listener(move |v, event: &ClickEvent, w, cx| {
                        if event.standard_click() {
                            v.run_menu_action(action, w, cx);
                        }
                    }))
                    .child(icon.label(label)),
            );
        }
        deferred(
            anchored()
                .anchor(Corner::TopRight)
                .offset(point(px(0.), px(4.)))
                .snap_to_window_with_margin(px(6.))
                .child(list),
        )
    }

    pub(super) fn menu_button(&self, w: &Window, cx: &Context<Self>) -> Div {
        let open = self.menu_index.is_some();
        let trigger = button_base("workspace-menu", "Menu")
            .track_focus(&self.menu_focus)
            .flex()
            .items_center()
            .gap_1()
            .h(px(30.))
            .border_1()
            .border_color(gpui::transparent_black())
            .focus(|s| s.border_color(rgb(0x85afa4)))
            .text_color(rgb(0x205e53))
            .bg(rgb(if open { 0xcde2da } else { 0xe7efed }))
            .hover(|s| s.bg(rgb(0xd3e4df)))
            .child(
                if open {
                    Icon::ChevronUp
                } else {
                    Icon::ChevronDown
                }
                .draw(13.),
            )
            .on_key_down(cx.listener(Self::menu_key))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |v, _, w, cx| {
                    // Outside dismissal and the trigger receive the same press.
                    // Use the state of the button the user actually pressed.
                    v.menu_index = if open { None } else { Some(0) };
                    v.menu_focus.focus(w);
                    cx.notify();
                }),
            );
        div()
            .relative()
            .flex()
            .flex_none()
            .items_center()
            .h_full()
            .px_1()
            .child(trigger)
            .when_some(self.menu_index, |d, i| {
                d.child(
                    div()
                        .absolute()
                        .bottom_0()
                        .right_1()
                        .size(px(0.))
                        .child(self.dropdown(i, w, cx)),
                )
            })
    }
}
