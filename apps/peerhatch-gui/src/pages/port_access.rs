use super::settings_catalog as catalog;
use crate::{
    Dashboard, GatewayRequest, Input, Operation,
    components::{
        ButtonVariant::{Ghost, Secondary},
        Icon, SettingsGroup, Ui, row, setting_field,
    },
};
use gpui::{prelude::*, *};
use peerhatch_control::{Command, PortAccess, listen_address, tcp_target};

#[derive(Clone, Copy)]
enum AccessList {
    Connect,
    Listen,
}

impl AccessList {
    fn key(self) -> &'static str {
        match self {
            Self::Connect => "connect",
            Self::Listen => "listen",
        }
    }

    fn update(self, ports: &mut PortAccess, text: &str, add: bool) -> Result<(), String> {
        fn update_entry<T: PartialEq>(
            entries: &mut Vec<T>,
            value: T,
            add: bool,
        ) -> Result<(), String> {
            if add {
                if entries.contains(&value) {
                    return Err("This address is already allowed.".into());
                }
                entries.push(value);
            } else {
                entries.retain(|entry| entry != &value);
            }
            Ok(())
        }
        match self {
            Self::Connect => update_entry(&mut ports.connect, tcp_target(text)?, add),
            Self::Listen => update_entry(&mut ports.listen, listen_address(text)?, add),
        }
    }
}

impl Dashboard {
    fn access_field(&self, list: AccessList) -> &Entity<Input> {
        match list {
            AccessList::Connect => &self.connect_field,
            AccessList::Listen => &self.listen_field,
        }
    }

    fn change_access(&mut self, list: AccessList, remove: Option<&str>, cx: &mut Context<Self>) {
        if self.saving {
            return;
        }
        let Some(current) = self
            .status
            .as_ref()
            .and_then(|s| s.gateway_port_access.clone())
        else {
            return;
        };
        let field = self.access_field(list).clone();
        let draft = field.read(cx).text().to_owned();
        let mut ports = current.clone();
        if let Err(error) =
            list.update(&mut ports, remove.unwrap_or(draft.trim()), remove.is_none())
        {
            self.action_error = Some(error);
            cx.notify();
            return;
        }
        self.action_error = None;
        let add = remove.is_none();
        let dashboard = cx.weak_entity();
        self.request(
            GatewayRequest {
                command: Operation::Api(Command::SetGatewayPortAccess {
                    expected: Some(current),
                    ports,
                }),
                on_complete: Some(Box::new(move |result, cx| match result {
                    Ok(()) if add && field.read(cx).text() == draft => {
                        field.update(cx, |v, cx| v.set_text("", cx));
                    }
                    Err(error) => cx.defer(move |cx| {
                        let _ = dashboard.update(cx, |v, cx| {
                            v.action_error = Some(error);
                            v.send(Command::Status, cx);
                        });
                    }),
                    _ => {}
                })),
            },
            cx,
        );
    }

    pub(super) fn port_access_settings(&self, ui: Ui, cx: &Context<Self>) -> SettingsGroup {
        let mut group = SettingsGroup::new(catalog::PORT_ACCESS);
        let Some(ports) = self
            .status
            .as_ref()
            .and_then(|s| s.gateway_port_access.as_ref())
        else {
            group.notice = Some(
                ui.muted(if self.status.is_some() {
                    "Update the local gateway to edit port access."
                } else {
                    "Start the local gateway to edit port access."
                })
                .into_any_element(),
            );
            return group;
        };
        group.notice = Some(ui.muted("These permissions apply to all paired devices connecting to this gateway. Published services keep their own access lists.").into_any_element());
        for (list, title, description, entries) in [
            (
                AccessList::Connect,
                "Allowed connection targets",
                "Addresses paired devices may connect to through this gateway.",
                ports.connect.clone(),
            ),
            (
                AccessList::Listen,
                "Allowed listen addresses",
                "Addresses paired devices may listen on here for reverse forwarding.",
                ports.listen.iter().map(ToString::to_string).collect(),
            ),
        ] {
            let key = list.key();
            let control = div()
                .max_w_full()
                .min_w_0()
                .flex()
                .items_center()
                .gap_2()
                .child(setting_field(self.access_field(list).clone()))
                .child(
                    ui.button(Secondary, SharedString::from(format!("add-{key}")), "Add")
                        .on_click(cx.listener(move |v, _, _, cx| v.change_access(list, None, cx))),
                );
            let mut details = div().w_full().flex().flex_col();
            if entries.is_empty() {
                details = details.child(ui.muted(match list {
                    AccessList::Connect => "No additional targets allowed.",
                    AccessList::Listen => "No listen addresses allowed.",
                }));
            }
            for address in entries {
                details =
                    details.child(
                        row().child(address.clone()).child(
                            ui.icon_button(
                                Ghost,
                                SharedString::from(format!("{key}-{address}")),
                                Icon::Close,
                                "Remove address",
                            )
                            .on_click(cx.listener(
                                move |v, _, _, cx| v.change_access(list, Some(&address), cx),
                            )),
                        ),
                    );
            }
            group
                .add(key, title, description, "tcp gateway paired ports", control)
                .details = Some(details.into_any_element());
        }
        group
    }
}
