use super::*;
use paxri_config::DevicePlatform;

impl Ui {
    pub fn device_heading(
        self,
        badge: Div,
        name: SharedString,
        platform: DevicePlatform,
        id: Option<(&str, String)>,
    ) -> Div {
        let detail = match id {
            Some((id, display_id)) => {
                single_line(self.muted(format!("{} · {display_id}", platform.label())))
                    .w_full()
                    .id("device-identity")
                    .tooltip(tooltip(id.to_owned().into()))
                    .into_any_element()
            }
            None => self.muted(platform.label()).into_any_element(),
        };
        div()
            .min_w_0()
            .flex()
            .items_center()
            .gap(SPACE_3)
            .child(self.icon_tile(Icon::device(platform), 48., 25.))
            .child(
                div()
                    .min_w_0()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .min_w_0()
                            .flex()
                            .items_center()
                            .gap(SPACE_2)
                            .child(
                                single_line(div())
                                    .text_2xl()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .id("device-heading")
                                    .tooltip(tooltip(name.clone()))
                                    .child(name),
                            )
                            .child(badge),
                    )
                    .child(detail),
            )
    }

    pub fn device_row(
        self,
        id: impl Into<ElementId>,
        platform: DevicePlatform,
        name: SharedString,
        detail: String,
    ) -> Button {
        self.button(
            ButtonVariant::Ghost,
            id,
            div()
                .w_full()
                .min_w_0()
                .flex()
                .items_center()
                .gap_2()
                .py_1()
                .child(self.icon_tile(Icon::device(platform), 30., 18.))
                .child(
                    div()
                        .min_w_0()
                        .flex_1()
                        .flex()
                        .flex_col()
                        .child(div().truncate().child(name))
                        .child(self.muted(detail).text_xs().truncate()),
                ),
        )
        .row()
    }
}

impl Ui {
    pub fn new_session_button(self) -> Button {
        self.button(
            ButtonVariant::Primary,
            "new-terminal-session",
            "New session",
        )
        .icon(Icon::Terminal)
    }

    pub fn end_session_button(self) -> Button {
        // Desktop toolbar icons leave a one-pixel inset on their 16 px grid.
        let size = if cfg!(target_os = "android") { 22. } else { 14. };
        let button = self.button(ButtonVariant::Ghost, "end-session", Icon::Stop.draw(size));
        #[cfg(not(target_os = "android"))]
        let button = button.icon_only();
        #[cfg(target_os = "android")]
        let button = button.circular().role(Role::Button).aria_label("End session");
        button.tooltip(tooltip("End session".into()))
    }

    pub fn session_row(self, number: usize, title: SharedString, detail: String) -> Button {
        self.navigation_row(
            ("show-terminal", number),
            Icon::Terminal.draw(18.),
            title,
            detail,
        )
    }

    pub fn terminal_sessions(
        self,
        action: Option<Button>,
        rows: impl Iterator<Item = Button>,
    ) -> Div {
        let section = self
            .section()
            .child(row().child(heading("Terminal sessions")).children(action));
        let mut rows = rows.peekable();
        if rows.peek().is_none() {
            section.child(self.muted("No open sessions."))
        } else {
            section.child(div().flex().flex_col().children(rows))
        }
    }
}
