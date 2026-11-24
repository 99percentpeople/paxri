use super::*;
use paxri_config::DevicePlatform;

impl Ui {
    pub fn local_device_footer(self, name: SharedString, status: Option<&'static str>) -> Div {
        div()
            .flex_none()
            .flex()
            .items_center()
            .gap_2()
            .child(Icon::device(DevicePlatform::current()).draw(18.))
            .child(
                div()
                    .min_w_0()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .child(single_line(div()).child(name))
                    .when_some(status, |labels, status| labels.child(self.muted(status).text_xs())),
            )
    }

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

    pub fn device_item(
        self,
        platform: DevicePlatform,
        name: SharedString,
        online: bool,
        detail: String,
    ) -> ListItem {
        let dot = div()
            .size(px(7.))
            .rounded_full()
            .flex_none()
            .border_1()
            .border_color(rgb(self.colors.muted_foreground))
            .when(online, |dot| {
                dot.bg(rgb(self.colors.success))
                    .border_color(rgb(self.colors.success))
            });
        ListItem::new(name)
            .leading(self.device_icon(platform))
            .detail(detail)
            .detail_icon(dot)
    }

    pub fn device_row(
        self,
        id: impl Into<ElementId>,
        platform: DevicePlatform,
        name: SharedString,
        online: bool,
        detail: String,
    ) -> Button {
        let label = SharedString::from(format!("{name} · {} · {detail}", platform.label()));
        let item = self.device_item(platform, name, online, detail);
        #[cfg(target_os = "android")]
        let item = item.chevron(self);
        item.button(self, id, label)
    }
}

impl Ui {
    pub fn new_session_button(self) -> Button {
        self.floating_action("new-terminal-session", Icon::Terminal, "New session")
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

    pub fn session_row(
        self,
        peer: &str,
        name: &str,
        number: usize,
        nonce: u64,
        title: SharedString,
        location: directory::SessionLocation,
    ) -> Button {
        self.navigation_row(
            SharedString::from(format!("show-terminal-{peer}-{number}-{nonce}")),
            Icon::Terminal.draw(18.),
            title,
            format!("{name} · Session {number} · {}", location.label()),
        )
    }
}
