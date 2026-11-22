use super::{port_actions::PortEntry, settings_catalog};
use crate::{
    Dashboard, Input,
    components::{
        ButtonVariant::{Ghost, Primary},
        Dialog, DialogContent, DialogMode, DialogSuccess, Icon, SettingsGroup, Ui, form_row,
        single_line,
    },
};
use gpui::{prelude::*, *};
use peerhatch_control::{Command, Service, tcp_target};
use std::collections::BTreeSet;

struct ServiceForm {
    dashboard: Entity<Dashboard>,
    original: Option<(String, Service)>,
    name: Entity<Input>,
    target: Entity<Input>,
    selected: BTreeSet<String>,
}

impl Dashboard {
    pub(super) fn published_services(&self, ui: Ui, cx: &Context<Self>) -> SettingsGroup {
        let mut group = SettingsGroup::new(settings_catalog::SERVICES);
        let Some(status) = &self.status else {
            group.notice = Some(
                ui.muted("Start the local gateway to manage published services.")
                    .into_any_element(),
            );
            return group;
        };
        let Some(grants) = &status.service_allow_peers else {
            group.notice = Some(
                ui.muted("Update the local gateway to manage published services.")
                    .into_any_element(),
            );
            for (name, target) in &status.services {
                let key = format!("{}.target", toml_edit::Key::new(name));
                group.value(&key, name, target, ui);
            }
            return group;
        };
        group.notice = Some(ui.muted("Publish a TCP target on this gateway and choose which paired devices can open it by name.").into_any_element());
        group.add(
            "",
            "Local services",
            if status.services.is_empty() {
                "No services published."
            } else {
                ""
            },
            "create publish tcp",
            ui.button(Primary, "add-service", "Add service")
                .icon(Icon::Plus)
                .on_click(cx.listener(|v, _, w, cx| v.open_service(None, w, cx))),
        );
        for (name, target) in &status.services {
            let key = toml_edit::Key::new(name).to_string();
            let Some(peers) = grants.get(name) else {
                group.value(&key, name, target, ui);
                continue;
            };
            let access = if peers.is_empty() {
                "No devices allowed".into()
            } else {
                peers.join(", ")
            };
            let service = Service {
                target: target.clone(),
                allow_peers: peers.clone(),
            };
            let mut description = format!("{target} · {access}");
            if status
                .gateway_port_access
                .as_ref()
                .is_some_and(|ports| ports.connect.contains(target))
            {
                description
                    .push_str(" · Port access allows all paired devices to connect by address.");
            }
            group.add(
                &key,
                name.clone(),
                description,
                "target allow_peers",
                self.port_actions(name, PortEntry::Service(service), cx),
            );
        }
        group
    }

    pub(super) fn open_service(
        &mut self,
        name: Option<&str>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(status) = self.status.as_ref().filter(|_| !self.saving) else {
            return;
        };
        let Some(grants) = status.service_allow_peers.as_ref() else {
            return;
        };
        let original = match name {
            Some(name) => {
                let (Some(target), Some(peers)) = (status.services.get(name), grants.get(name))
                else {
                    return;
                };
                Some((
                    name.to_owned(),
                    Service {
                        target: target.clone(),
                        allow_peers: peers.clone(),
                    },
                ))
            }
            None => None,
        };
        let form = ServiceForm {
            dashboard: cx.entity(),
            name: Input::new(name.unwrap_or_default(), "dev-web", cx),
            target: Input::new(
                original.as_ref().map_or("", |(_, s)| s.target.as_str()),
                "127.0.0.1:3000",
                cx,
            ),
            selected: original
                .as_ref()
                .map(|(_, s)| s.allow_peers.iter().cloned().collect())
                .unwrap_or_default(),
            original,
        };
        let focus = form.name.read(cx).focus_handle(cx);
        let model = cx.new(|cx| {
            cx.observe(&form.dashboard, |_, _, cx| cx.notify()).detach();
            form
        });
        Dialog::open(
            window,
            cx,
            DialogMode::Modal,
            if name.is_some() {
                "Edit published service"
            } else {
                "New published service"
            },
            size(px(500.), px(380.)),
            model,
            Some(focus),
            ServiceForm::render_form,
        );
    }
}

impl ServiceForm {
    fn save(&self, w: &Window, cx: &mut Context<Self>) {
        let draft = self.name.read(cx).text();
        let original_name = self.original.as_ref().map(|(name, _)| name.as_str());
        let name = if Some(draft) == original_name {
            draft
        } else {
            draft.trim()
        };
        if name.is_empty() {
            Dialog::show_error(w, "Enter a service name.", cx);
            return;
        }
        let target = match tcp_target(self.target.read(cx).text().trim()) {
            Ok(target) => target,
            Err(error) => {
                Dialog::show_error(w, error, cx);
                return;
            }
        };
        let command = Command::SaveService {
            name: original_name.unwrap_or(name).to_owned(),
            expected: self.original.as_ref().map(|(_, service)| service.clone()),
            rename_to: original_name
                .filter(|old| *old != name)
                .map(|_| name.to_owned()),
            service: Service {
                target,
                allow_peers: self.selected.iter().cloned().collect(),
            },
        };
        self.dashboard.update(cx, |v, cx| {
            v.send_from_dialog(command, w, DialogSuccess::Close, cx)
        });
    }

    fn render_form(&self, ui: Ui, cx: &Context<Self>) -> DialogContent {
        let dashboard = self.dashboard.read(cx);
        let mut devices = div().flex().flex_col().gap_1();
        if let Some(s) = &dashboard.status {
            if s.peers.is_empty() {
                devices = devices.child(ui.muted("No paired devices."));
            }
            for alias in s.peers.keys() {
                let selected = self.selected.contains(alias);
                let check = div()
                    .size(px(16.))
                    .flex_none()
                    .rounded(px(3.))
                    .border_1()
                    .border_color(rgb(ui.colors.input_border))
                    .flex()
                    .items_center()
                    .justify_center()
                    .when(selected, |d| {
                        d.bg(rgb(ui.colors.primary.background))
                            .text_color(rgb(ui.colors.primary.foreground))
                            .child(Icon::Check.draw(13.))
                    });
                let row = div()
                    .w_full()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(check)
                    .child(Icon::device(s.peer_platform(alias)).draw(16.))
                    .child(single_line(div()).child(alias.clone()));
                let id = (
                    ElementId::from("service-peer"),
                    SharedString::from(alias.clone()),
                );
                let alias = alias.clone();
                devices = devices.child(ui.button(Ghost, id, row).row().on_click(cx.listener(
                    move |v, _, _, cx| {
                        if !v.dashboard.read(cx).saving {
                            if !v.selected.remove(&alias) {
                                v.selected.insert(alias.clone());
                            }
                            cx.notify();
                        }
                    },
                )));
            }
        }
        let body = div()
            .flex()
            .flex_col()
            .gap_3()
            .child(
                form_row()
                    .child(ui.labeled("Service name", self.name.clone()))
                    .child(ui.labeled("TCP target", self.target.clone())),
            )
            .when(self.original.is_some(), |d| {
                d.child(ui.muted("If you rename this service, update its name in forwarding rules on other devices."))
            })
            .child(ui.labeled("Allowed devices", devices))
            .child(ui.muted("Leave all devices unchecked to deny new connections by name."));
        DialogContent::new(body, "Cancel").action(
            ui.button(
                Primary,
                "save-service",
                if dashboard.saving {
                    "Saving…"
                } else {
                    "Save service"
                },
            )
            .on_click(cx.listener(|v, _, w, cx| v.save(w, cx))),
        )
    }
}
