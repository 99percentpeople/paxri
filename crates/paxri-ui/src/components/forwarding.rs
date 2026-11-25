use super::{
    directory::{DeviceOption, ListFilters, contains_terms},
    *,
};
use crate::theme::BUTTON_HEIGHT;
use paxri_control::{Direction, ForwardStatus};
use std::net::SocketAddr;

pub const NARROW_WIDTH: Pixels = px(650.);

#[derive(Default)]
pub struct ForwardScope {
    pub device: Option<String>,
    pub direction: Option<Direction>,
    pub state: Option<&'static str>,
}

impl ForwardScope {
    pub fn active_count(&self) -> usize {
        usize::from(self.device.is_some())
            + usize::from(self.direction.is_some())
            + usize::from(self.state.is_some())
    }
}

pub type ForwardFilters = ListFilters<ForwardScope>;

impl ForwardFilters {
    pub fn for_forwards<V: 'static>(cx: &mut Context<V>) -> Self {
        Self::new("Search device or port…", cx)
    }

    pub fn for_device(&mut self, device: Option<String>, cx: &mut App) {
        self.reset(cx);
        self.scope.device = device;
    }

    pub fn has_selections(&self) -> bool {
        self.scope.active_count() > 0
    }

    pub fn matches(
        &self,
        device: &str,
        name: &str,
        rule: &ForwardStatus,
        display_name: &str,
        query: &str,
    ) -> bool {
        let state = if rule.enabled() && rule.error.is_some() {
            "Error"
        } else {
            rule.state().trim_end_matches('…')
        };
        if self.scope.device.as_ref().is_some_and(|peer| peer != device)
            || self
                .scope
                .direction
                .is_some_and(|direction| direction != rule.direction)
            || self.scope.state.is_some_and(|selected| selected != state)
        {
            return false;
        }
        if query.is_empty() {
            return true;
        }
        let direction = match rule.direction {
            Direction::Local => "forward",
            Direction::Remote => "reverse",
        };
        contains_terms(
            query,
            format!(
                "{name} {display_name} {} {} {} {} {direction} {state} {}",
                rule.peer,
                rule.listen,
                rule.bound.as_deref().unwrap_or(""),
                rule.target,
                rule.state()
            ),
        )
    }

    pub fn toolbar<V: 'static>(
        &self,
        options: &[DeviceOption],
        access: fn(&mut V) -> &mut Self,
        cx: &Context<V>,
    ) -> FilterToolbar {
        let mut devices = Select::new(
            "forward-device-filter",
            Some(self.scope.device.clone()),
            cx.listener(move |v, peer: &Option<String>, _, cx| {
                access(v).scope.device = peer.clone();
                cx.notify();
            }),
        )
        .option(None, "All devices");
        for (id, name, platform) in options {
            devices =
                devices.option_with_icon(Some(id.clone()), name.clone(), Icon::device(*platform));
        }
        let directions = Select::new(
            "forward-direction-filter",
            Some(self.scope.direction),
            cx.listener(move |v, direction: &Option<Direction>, _, cx| {
                access(v).scope.direction = *direction;
                cx.notify();
            }),
        )
        .option(None, "Any direction")
        .option(Some(Direction::Local), "Forward")
        .option(Some(Direction::Remote), "Reverse");
        let mut states = Select::new(
            "forward-state-filter",
            Some(self.scope.state),
            cx.listener(move |v, state: &Option<&'static str>, _, cx| {
                access(v).scope.state = *state;
                cx.notify();
            }),
        )
        .option(None, "Any state");
        for state in ["Listening", "Connecting", "Paused", "Error"] {
            states = states.option(Some(state), state);
        }

        let owner = cx.weak_entity();
        FilterToolbar::new(
            "forward-filters",
            "Filter forwarding rules",
            &self.search,
            self.scope.active_count(),
        )
        .group("Device", devices)
        .group("Direction", directions)
        .group("State", states)
        .clear(move |_, cx| {
            let _ = owner.update(cx, |v, cx| {
                access(v).scope = ForwardScope::default();
                cx.notify();
            });
        })
    }
}

pub fn endpoint_labels(direction: Direction) -> (&'static str, &'static str) {
    match direction {
        Direction::Local => ("Listen on this device", "Target on peer"),
        Direction::Remote => ("Listen on peer", "Target on this device"),
    }
}

pub fn direction_select(
    id: impl Into<ElementId>,
    direction: Direction,
    change: impl Fn(&Direction, &mut Window, &mut App) + 'static,
) -> Select<Direction> {
    Select::new(id, direction, change)
        .option(Direction::Local, "Forward")
        .option(Direction::Remote, "Reverse")
}

pub fn device_select(
    id: impl Into<ElementId>,
    selected: impl Into<Option<String>>,
    placeholder: impl Into<SharedString>,
    options: &[DeviceOption],
    change: impl Fn(&String, &mut Window, &mut App) + 'static,
) -> Select<String> {
    let mut devices = Select::new(id, selected, change).placeholder(placeholder);
    for (id, name, platform) in options {
        devices = devices.option_with_icon(id.clone(), name.clone(), Icon::device(*platform));
    }
    devices
}

pub fn parse_endpoints(listen: &str, target: &str) -> Result<(SocketAddr, String), String> {
    Ok((
        paxri_control::listen_address(listen.trim())?,
        paxri_control::tcp_target(target.trim())?,
    ))
}

fn columns() -> Div {
    div().w_full().min_w_0().flex().gap_2().items_center()
}
fn cell() -> Div {
    div().min_w_0().flex_1().flex().flex_col()
}

impl Ui {
    pub fn forwarding_table(self, list: ListView, narrow: bool, rows: Vec<Div>) -> ListView {
        list.divided()
            .when(!narrow, |list| list.child(self.forwarding_header()))
            .children(rows)
    }

    pub fn forwarding_header(self) -> Div {
        let mut header = columns();
        for label in ["Rule / Direction", "Device", "Listen", "Target", "State"] {
            header = header.child(cell().child(self.muted(label)));
        }
        ListItem::custom(header)
            .trailing(div().w(BUTTON_HEIGHT))
            .inset_row()
    }

    pub fn forwarding_row(
        self,
        key: String,
        name: &str,
        f: &ForwardStatus,
        display_name: &str,
        narrow: bool,
        actions: impl IntoElement,
    ) -> Div {
        let ui = self;
        let local = f.direction == Direction::Local;
        let direction = if local { "Forward" } else { "Reverse" };
        let (source, destination) = if local {
            ("This device", display_name)
        } else {
            (display_name, "This device")
        };
        let line = |column: &'static str, value: &str| {
            single_line(div())
                .w_full()
                .id((ElementId::from(column), SharedString::from(key.clone())))
                .tooltip(tooltip(value.to_owned().into()))
                .child(value.to_owned())
        };
        let endpoint = |column, host: &str, address: &str| {
            cell()
                .child(ui.muted(host.to_owned()).text_xs())
                .child(line(column, address))
        };
        let listen = f.bound.as_deref().unwrap_or(&f.listen);
        let title = super::list_item::list_title(name.to_owned())
            .id((ElementId::from("rule"), SharedString::from(key.clone())))
            .tooltip(tooltip(name.to_owned().into()));
        let item = if narrow {
            ListItem::custom(title)
                .detail(format!("{direction} · {}", f.state()))
                .below_detail(
                    div()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .child(ui.muted(format!("{source} / {listen}")))
                        .child(ui.muted(format!("→ {destination} / {}", f.target))),
                )
        } else {
            let fields = columns()
                .child(cell().child(title).child(ui.muted(direction).text_xs()))
                .child(cell().child(line("peer", display_name)))
                .child(endpoint("listen", source, listen))
                .child(endpoint("target", destination, &f.target))
                .child(cell().child(line("state", f.state())));
            ListItem::custom(fields)
        };
        item.trailing(actions)
            .inset_row()
            .rounded(CONTROL_RADIUS)
            .hover(|s| s.bg(rgb(ui.colors.accent.background)))
    }
}

#[derive(IntoElement)]
pub struct RuleMenu {
    id: SharedString,
    name: String,
    content: DropdownContent,
}

#[derive(Clone, Copy)]
pub enum RuleAction {
    Edit,
    SetEnabled(bool),
    Remove,
}

impl RuleMenu {
    pub fn new(
        id: String,
        name: &str,
        enabled: bool,
        action: impl Fn(RuleAction, &mut Window, &mut App) + 'static,
    ) -> Self {
        let action = std::rc::Rc::new(action);
        let mut content = DropdownContent::new();
        for (id, icon, label, item) in [
            ("edit", Icon::Pencil, "Edit", RuleAction::Edit),
            (
                "toggle",
                if enabled { Icon::Pause } else { Icon::Play },
                if enabled { "Pause" } else { "Resume" },
                RuleAction::SetEnabled(!enabled),
            ),
            ("remove", Icon::Trash, "Remove", RuleAction::Remove),
        ] {
            let call = action.clone();
            content = content.item(id, icon.label(label), move |w, cx| call(item, w, cx));
        }
        Self {
            id: id.into(),
            name: name.to_owned(),
            content,
        }
    }
}

impl RenderOnce for RuleMenu {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let label = SharedString::from(format!("Actions for {}", self.name));
        Ui::new(window, cx).actions_menu(self.id, label, self.content)
    }
}

#[cfg(test)]
mod tests {
    use super::{Direction, endpoint_labels, parse_endpoints};

    #[test]
    fn endpoint_labels_flip_with_direction_and_parse_defaults() {
        assert_eq!(
            endpoint_labels(Direction::Local),
            ("Listen on this device", "Target on peer")
        );
        assert_eq!(
            endpoint_labels(Direction::Remote),
            ("Listen on peer", "Target on this device")
        );
        let (listen, target) = parse_endpoints(" 8080 ", " 3000 ").unwrap();
        assert_eq!(listen.port(), 8080);
        assert_eq!(target, "127.0.0.1:3000");
        assert!(parse_endpoints("bad", "3000").is_err());
    }
}
