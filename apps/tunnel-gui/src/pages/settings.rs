use super::short_id;
use crate::{
    Dashboard, Input,
    components::{Button, ButtonVariant::Secondary, Icon, ListItem, Select, Ui, page, row},
    theme::Appearance,
};
use gpui::{prelude::*, *};
use tunnel_control::Command;

fn copy_button(ui: Ui, id: &'static str, label: &'static str, text: String) -> Button {
    ui.button(Secondary, id, label)
        .on_click(move |_, _, cx| cx.write_to_clipboard(ClipboardItem::new_string(text.clone())))
}

impl Dashboard {
    fn appearance_settings(&self, ui: Ui, cx: &Context<Self>) -> Div {
        let chosen = *cx.global::<Appearance>();
        let icon = |mode| match mode {
            Appearance::System => Icon::Monitor,
            Appearance::Light => Icon::Sun,
            Appearance::Dark => Icon::Moon,
        };
        let mut modes = Select::new(
            "theme",
            chosen,
            cx.listener(|view, mode: &Appearance, _, cx| {
                view.action_error = mode
                    .apply(cx)
                    .err()
                    .map(|err| format!("Save appearance: {err}"));
                cx.notify();
            }),
        )
        .icon_trigger(icon(chosen), format!("Theme: {}", chosen.label()));
        for mode in Appearance::ALL {
            modes = modes.option_with_icon(mode, mode.label(), icon(mode));
        }
        ui.section("Appearance")
            .child(row().child(ui.muted("Theme")).child(modes))
    }

    pub(crate) fn settings(&self, ui: Ui, cx: &mut Context<Self>) -> Div {
        let page = page("Settings")
            .child(self.appearance_settings(ui, cx))
            .child(self.terminal_settings.clone());
        let Some(s) = &self.status else {
            return page.child(self.gateway_notice(ui, cx));
        };
        let mut device = ui.section("This device").child(ui.separator());
        if let Some(field) = &self.name_field {
            device = device
                .child(ui.labeled("Device name", field.clone()))
                .child(ui.muted(format!(
                    "Leave blank to use the computer name ({})",
                    s.computer_name
                )))
                .child(ui.form_actions(
                    "name",
                    if self.saving {
                        "Saving…"
                    } else {
                        "Save name"
                    },
                    cx.listener(|v, _, _, cx| {
                        if !v.saving
                            && let Some(field) = &v.name_field
                        {
                            let text = field.read(cx).text().trim();
                            v.send(
                                Command::SetDeviceName {
                                    name: (!text.is_empty()).then(|| text.to_owned()),
                                },
                                cx,
                            );
                        }
                    }),
                    cx.listener(|v, _, _, cx| {
                        if !v.saving {
                            v.name_field = None;
                            v.action_error = None;
                            cx.notify();
                        }
                    }),
                ));
        } else {
            let name = s.device_name.clone().unwrap_or_default();
            let system = s.computer_name.clone();
            device = device.child(
                row()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(ui.muted("Device name"))
                            .child(s.display_name().to_owned())
                            .child(ui.muted(if s.device_name.is_none() {
                                "Using computer name"
                            } else {
                                "Custom name"
                            })),
                    )
                    .child(
                        ui.button(Secondary, "edit-name", "Change name")
                            .on_click(cx.listener(move |v, _, w, cx| {
                                let field = Input::new(&name, &system, cx);
                                field.read(cx).focus.focus(w);
                                v.name_field = Some(field);
                                v.action_error = None;
                                cx.notify();
                            })),
                    ),
            );
        }
        let identity = ui
            .section("Gateway")
            .child(ui.separator())
            .child(
                ListItem::new(ui.muted("Device ID"))
                    .detail(short_id(&s.device_id))
                    .trailing(copy_button(ui, "copy-id", "Copy ID", s.device_id.clone())),
            )
            .child(ui.muted(format!(
                "Local IPC · {}",
                self.address
                    .clone()
                    .unwrap_or_else(tunnel_control::default_address)
                    .display()
            )));
        let mut services = ui.section("Published services");
        if s.services.is_empty() {
            services = services.child(ui.muted("No services published."));
        }
        for (name, target) in &s.services {
            services = services
                .child(ui.separator())
                .child(ListItem::new(name.clone()).trailing(ui.muted(target.clone())));
        }
        page.child(device).child(identity).child(services)
    }
}
