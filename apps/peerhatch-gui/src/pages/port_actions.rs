use crate::{
    Dashboard,
    components::{
        ButtonVariant::{Destructive, Ghost},
        Dialog, DialogContent, DialogMode, DialogSuccess, Dropdown, DropdownContent, DropdownState,
        DropdownTrigger, Icon, Ui,
    },
};
use gpui::{prelude::*, *};
use peerhatch_control::{Command, Service};

#[derive(Clone)]
pub(super) enum PortEntry {
    Forward { peer: String, enabled: bool },
    Service(Service),
}

#[derive(IntoElement)]
pub(super) struct PortActions {
    id: SharedString,
    name: String,
    content: DropdownContent,
}

impl RenderOnce for PortActions {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let ui = Ui::new(window, cx);
        let state = DropdownState::local(self.id.clone().into(), window, cx);
        let trigger = ui
            .icon_button(
                Ghost,
                self.id,
                Icon::Ellipsis,
                format!("Actions for {}", self.name),
            )
            .pressed(state.read(cx).is_open());
        Dropdown::new(&state, DropdownTrigger::new(trigger), self.content)
    }
}

impl Dashboard {
    pub(super) fn port_actions(
        &self,
        name: &str,
        entry: PortEntry,
        cx: &Context<Self>,
    ) -> PortActions {
        let is_service = matches!(entry, PortEntry::Service(_));
        let dashboard = cx.entity();
        let edit = name.to_owned();
        let mut content =
            DropdownContent::new().item("edit", Icon::Pencil.label("Edit"), move |w, cx| {
                dashboard.update(cx, |v, cx| {
                    if is_service {
                        v.open_service(Some(&edit), w, cx);
                    } else {
                        v.open_rule(Some(&edit), w, cx);
                    }
                });
            });
        if let PortEntry::Forward { peer, enabled } = &entry {
            let dashboard = cx.entity();
            let name = name.to_owned();
            let peer = peer.clone();
            let enabled = *enabled;
            content = content.item(
                "toggle",
                if enabled {
                    Icon::Pause.label("Pause")
                } else {
                    Icon::Play.label("Resume")
                },
                move |_, cx| {
                    dashboard.update(cx, |v, cx| {
                        if !v.saving {
                            v.send(
                                Command::SetForward {
                                    name: name.clone(),
                                    expected_peer: Some(peer.clone()),
                                    enabled: !enabled,
                                },
                                cx,
                            );
                        }
                    });
                },
            );
        }
        let dashboard = cx.entity();
        let remove = name.to_owned();
        let (title, action, prompt) = match &entry {
            PortEntry::Forward { peer, .. } => (
                "Remove forwarding rule?",
                "Remove rule",
                format!("Remove “{name}” from {peer}?"),
            ),
            PortEntry::Service(_) => (
                "Remove published service?",
                "Remove service",
                format!("Stop accepting new connections to “{name}” by name?"),
            ),
        };
        let content = content.item("remove", Icon::Trash.label("Remove"), move |w, cx| {
            let name = remove.clone();
            let entry = entry.clone();
            let prompt = prompt.clone();
            Dialog::open(
                w,
                cx,
                DialogMode::Modal,
                title,
                size(px(400.), px(180.)),
                dashboard.clone(),
                None,
                move |_, ui, cx| {
                    let name = name.clone();
                    let entry = entry.clone();
                    DialogContent::new(prompt.clone(), "Cancel").action(
                        ui.button(Destructive, "confirm-remove", action)
                            .on_click(cx.listener(move |v, _, w, cx| {
                                let command = match &entry {
                                    PortEntry::Forward { peer, .. } => Command::RemoveForward {
                                        name: name.clone(),
                                        expected_peer: Some(peer.clone()),
                                    },
                                    PortEntry::Service(expected) => Command::RemoveService {
                                        name: name.clone(),
                                        expected: expected.clone(),
                                    },
                                };
                                v.send_from_dialog(command, w, DialogSuccess::Close, cx);
                            })),
                    )
                },
            );
        });
        let kind = if is_service { "service" } else { "rule" };
        PortActions {
            id: format!("{kind}-actions-{name}").into(),
            name: name.to_owned(),
            content,
        }
    }
}
