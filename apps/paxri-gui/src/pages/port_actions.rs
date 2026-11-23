use crate::{
    Dashboard,
    components::{
        ButtonVariant::{Destructive, Ghost},
        Dialog, DialogContent, DialogMode, DialogSuccess, Dropdown, DropdownContent, DropdownState,
        DropdownTrigger, Icon, Ui,
    },
};
use gpui::{prelude::*, *};
use paxri_control::{Command, ForwardStatus, PeerForward};

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
        owner: Option<&str>,
        rule: &ForwardStatus,
        display_name: &str,
        cx: &Context<Self>,
    ) -> PortActions {
        let dashboard = cx.entity();
        let edit = name.to_owned();
        let edit_owner = owner.map(str::to_owned);
        let content =
            DropdownContent::new().item("edit", Icon::Pencil.label("Edit"), move |w, cx| {
                dashboard.update(cx, |v, cx| {
                    v.open_rule(edit_owner.as_deref(), Some(&edit), w, cx)
                });
            });
        let dashboard = cx.entity();
        let enabled = rule.enabled();
        let toggle_owner = owner.map(str::to_owned);
        let toggle_name = name.to_owned();
        let toggle_peer = rule.peer.clone();
        let content = content.item(
            "toggle",
            if enabled {
                Icon::Pause.label("Pause")
            } else {
                Icon::Play.label("Resume")
            },
            move |_, cx| {
                dashboard.update(cx, |v, cx| {
                    if v.saving {
                        return;
                    }
                    let command = match &toggle_owner {
                        Some(owner) => Command::ManagePeerForward {
                            owner: owner.clone(),
                            action: PeerForward::SetEnabled {
                                name: toggle_name.clone(),
                                enabled: !enabled,
                            },
                        },
                        None => Command::SetForward {
                            name: toggle_name.clone(),
                            expected_peer: Some(toggle_peer.clone()),
                            enabled: !enabled,
                        },
                    };
                    v.send(command, cx);
                });
            },
        );
        let dashboard = cx.entity();
        let remove = name.to_owned();
        let peer = rule.peer.clone();
        let remote = owner.map(str::to_owned);
        let prompt = format!("Remove the forwarding rule for {}?", rule.route_for(display_name));
        let content = content.item("remove", Icon::Trash.label("Remove"), move |w, cx| {
            let name = remove.clone();
            let peer = peer.clone();
            let remote = remote.clone();
            let prompt = prompt.clone();
            Dialog::open(
                w,
                cx,
                DialogMode::Modal,
                "Remove forwarding rule?",
                size(px(400.), px(180.)),
                dashboard.clone(),
                None,
                move |_, ui, cx| {
                    let name = name.clone();
                    let peer = peer.clone();
                    let remote = remote.clone();
                    DialogContent::new(prompt.clone(), "Cancel").action(
                        ui.button(Destructive, "confirm-remove", "Remove rule")
                            .on_click(cx.listener(move |v, _, w, cx| {
                                let command = match &remote {
                                    Some(owner) => Command::ManagePeerForward {
                                        owner: owner.clone(),
                                        action: PeerForward::Remove { name: name.clone() },
                                    },
                                    None => Command::RemoveForward {
                                        name: name.clone(),
                                        expected_peer: Some(peer.clone()),
                                    },
                                };
                                v.send_from_dialog(command, w, DialogSuccess::Close, cx);
                            })),
                    )
                },
            );
        });
        PortActions {
            id: format!("rule-actions-{owner:?}/{name}").into(),
            name: name.to_owned(),
            content,
        }
    }
}
