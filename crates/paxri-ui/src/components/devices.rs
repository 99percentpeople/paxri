use super::*;
use paxri_config::DevicePlatform;

#[derive(Clone, Copy)]
pub struct DeviceCounts {
    pub sessions: Option<usize>,
    pub forwards: Option<usize>,
}

fn count_label(count: Option<usize>) -> String {
    count.map_or_else(|| "—".into(), |n| n.to_string())
}

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
                    .when_some(status, |labels, status| labels.child(self.caption(status))),
            )
    }

    pub fn page_heading(
        self,
        icon: Icon,
        title: impl Into<SharedString>,
        subtitle: impl Into<SharedString>,
    ) -> Div {
        let title = single_line(div())
            .text_role(TextRole::PageTitle)
            .child(title.into());
        let detail = single_line(self.muted(subtitle.into())).w_full();
        div()
            .min_w_0()
            .flex()
            .items_center()
            .gap(SPACE_3)
            .child(self.icon_tile(icon, 48., 25.))
            .child(
                div()
                    .min_w_0()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .child(title)
                    .child(detail),
            )
    }

    pub fn device_heading(
        self,
        badge: Option<Div>,
        name: SharedString,
        platform: DevicePlatform,
        id: Option<(&str, String)>,
    ) -> Div {
        let detail = match id {
            Some((id, display_id)) => single_line(self.muted(format!("{} · {display_id}", platform.label())))
                .w_full()
                .id("device-identity")
                .tooltip(tooltip(id.to_owned().into()))
                .into_any_element(),
            None => self.muted(platform.label()).into_any_element(),
        };
        let title = single_line(div())
            .text_role(TextRole::PageTitle)
            .id("device-heading")
            .tooltip(tooltip(name.clone()))
            .child(name);
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
                            .child(title)
                            .children(badge),
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
        let label = format!("{name} · {} · {detail}", platform.label());
        let item = self.device_item(platform, name, online, detail);
        #[cfg(target_os = "android")]
        let item = item.chevron(self);
        item.button(self, id, label.into())
    }

    pub fn counted_navigation_row(
        self, id: &'static str, icon: Icon, title: &'static str, count: Option<usize>,
    ) -> Button {
        let count = count_label(count);
        let label = format!("{title}: {count}").into();
        ListItem::new(title)
            .leading(icon.draw(18.))
            .trailing(div().flex().items_center().gap_2().text_role(TextRole::Body)
                .text_color(rgb(self.colors.muted_foreground)).child(count).child(Icon::ChevronRight.draw(14.)))
            .button(self, id, label)
    }
}

impl Ui {
    pub fn new_session_button(self) -> Button {
        self.floating_action("new-terminal-session", Icon::Terminal, "New session")
    }

    #[cfg(not(target_os = "android"))]
    pub fn end_session_button(self) -> Button {
        self.button(ButtonVariant::Ghost, "end-session", Icon::Stop.draw(14.))
            .icon_only()
            .tooltip(tooltip("End session".into()))
    }

    pub fn device_detail_sections(
        self,
        connection: Div,
        counts: DeviceCounts,
        show_forwards: bool,
        open_sessions: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
        open_forwards: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Div {
        div()
            .w_full()
            .min_w_0()
            .flex()
            .flex_col()
            .gap(SPACE_4)
            .child(self.separator())
            .child(connection)
            .child(self.separator())
            .child(
                self.counted_navigation_row(
                    "device-sessions",
                    Icon::Terminal,
                    "Sessions",
                    counts.sessions,
                )
                .on_click(open_sessions),
            )
            .when(show_forwards, |page| {
                page.child(self.separator()).child(
                    self.counted_navigation_row(
                        "device-forwards",
                        Icon::Forward,
                        "Port forwarding",
                        counts.forwards,
                    )
                    .on_click(open_forwards),
                )
            })
    }

    pub fn device_actions(
        self,
        id: impl Into<ElementId>,
        name: &str,
        reconnect: impl Fn(&mut Window, &mut App) + 'static,
        unpair: impl Fn(&mut Window, &mut App) + 'static,
    ) -> ActionsMenu {
        let menu = DropdownContent::new()
            .item("reconnect-device", Icon::Refresh.label("Reconnect"), reconnect)
            .separator()
            .item("unpair-device", Icon::Trash.label("Unpair device"), unpair);
        let menu = self.actions_menu(id, format!("Actions for {name}").into(), menu);
        #[cfg(target_os = "android")]
        let menu = menu.circular_trigger();
        menu
    }

    pub fn session_row(
        self,
        peer: &str,
        name: &str,
        key: paxri_control::terminal::managed::Key,
        title: SharedString,
        location: directory::SessionLocation,
        actions: Option<ActionsMenu>,
        open: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Button {
        let id = SharedString::from(format!("show-terminal-{peer}-{}-{}", key.number, key.nonce));
        let ended = location == directory::SessionLocation::Ended;
        let dot = div().size(px(7.)).rounded_full().flex_none();
        let dot = match location {
            directory::SessionLocation::Open => dot.bg(rgb(self.colors.success)),
            directory::SessionLocation::Gateway => {
                dot.border_1().border_color(rgb(self.colors.success))
            }
            directory::SessionLocation::Ended => {
                dot.border_1().border_color(rgb(self.colors.muted_foreground))
            }
        };
        let detail = session_detail(title.as_ref(), name, key.number, location);
        let heading = list_item::list_title(title.clone())
            .when(ended, |text| text.text_color(rgb(self.colors.muted_foreground)));
        ListItem::custom(heading)
            .detail_icon(dot)
            .detail(detail)
            .when_some(actions, |item, actions| item.trailing(actions))
            .button(self, id, title)
            .on_click(open)
    }
}

fn session_detail(
    title: &str,
    device_name: &str,
    number: usize,
    location: directory::SessionLocation,
) -> String {
    if title == device_name {
        format!("#{number} · {}", location.label())
    } else {
        format!("{device_name} · #{number} · {}", location.label())
    }
}

impl DropdownContent {
    pub fn end_session(self, end: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.item("end-session", Icon::Stop.label("End session"), end)
    }
}

#[cfg(test)]
mod tests {
    use super::{directory::SessionLocation, session_detail};

    #[test]
    fn session_detail_omits_duplicate_device_name_when_title_matches() {
        assert_eq!(
            session_detail("Studio", "Studio", 1, SessionLocation::Open),
            "#1 · Open"
        );
        assert_eq!(
            session_detail("vim", "Studio", 2, SessionLocation::Gateway),
            "Studio · #2 · Detached"
        );
        assert_eq!(
            session_detail("Studio 2", "Studio", 2, SessionLocation::Ended),
            "Studio · #2 · Ended"
        );
    }
}
