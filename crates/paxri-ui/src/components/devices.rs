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
                    .when_some(status, |labels, status| labels.child(self.muted(status).text_xs())),
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
            Some((id, display_id)) => {
                let text = single_line(self.muted(format!("{} · {display_id}", platform.label())))
                    .w_full()
                    .id("device-identity")
                    .tooltip(tooltip(id.to_owned().into()));
                #[cfg(target_os = "android")]
                let text = text.text_ellipsis();
                text.into_any_element()
            }
            None => self.muted(platform.label()).into_any_element(),
        };
        let title = single_line(div())
            .text_2xl()
            .font_weight(FontWeight::SEMIBOLD)
            .id("device-heading")
            .tooltip(tooltip(name.clone()))
            .child(name);
        #[cfg(target_os = "android")]
        let title = title.text_ellipsis();
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
            .trailing(div().flex().items_center().gap_2().text_sm()
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
        unpair: impl Fn(&mut Window, &mut App) + 'static,
    ) -> ActionsMenu {
        let menu =
            DropdownContent::new().item("unpair-device", Icon::Trash.label("Unpair device"), unpair);
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
        ListItem::new(title.clone())
            .leading(Icon::Terminal.draw(18.))
            .detail(format!(
                "{name} · Session {} · {}",
                key.number,
                location.label()
            ))
            .when_some(actions, |item, actions| item.trailing(actions))
            .button(self, id, title)
            .on_click(open)
    }
}

impl DropdownContent {
    pub fn end_session(self, end: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.item("end-session", Icon::Stop.label("End session"), end)
    }
}
