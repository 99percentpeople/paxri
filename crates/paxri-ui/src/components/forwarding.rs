use super::{directory::DeviceOption, input::Input, *};
use crate::theme::BUTTON_HEIGHT;
use paxri_control::{Direction, ForwardStatus};

pub struct ForwardFilters {
    pub device: Option<String>,
    pub direction: Option<Direction>,
    state: Option<&'static str>,
    pub search: Entity<Input>,
}

impl ForwardFilters {
    pub fn new<V: 'static>(cx: &mut Context<V>) -> Self {
        let search = Input::search("Search device or port…", cx);
        cx.observe(&search, |_, _, cx| cx.notify()).detach();
        Self {
            search,
            device: None,
            direction: None,
            state: None,
        }
    }

    pub fn reset(&mut self, device: Option<String>, cx: &mut App) {
        self.search.update(cx, |input, cx| input.set_text("", cx));
        self.device = device;
        self.direction = None;
        self.state = None;
    }

    pub fn query(&self, cx: &App) -> String {
        self.search.read(cx).text().trim().to_lowercase()
    }

    pub fn has_selections(&self) -> bool {
        self.device.is_some() || self.direction.is_some() || self.state.is_some()
    }

    pub fn matches(
        &self,
        device: &str,
        name: &str,
        rule: &ForwardStatus,
        display_name: &str,
        terms: &[&str],
    ) -> bool {
        let state = if rule.enabled() && rule.error.is_some() {
            "Error"
        } else {
            rule.state().trim_end_matches('…')
        };
        if self.device.as_ref().is_some_and(|peer| peer != device)
            || self
                .direction
                .is_some_and(|direction| direction != rule.direction)
            || self.state.is_some_and(|selected| selected != state)
        {
            return false;
        }
        if terms.is_empty() {
            return true;
        }
        let direction = match rule.direction {
            Direction::Local => "forward",
            Direction::Remote => "reverse",
        };
        let searchable = format!(
            "{name} {display_name} {} {} {} {} {direction} {state} {}",
            rule.peer,
            rule.listen,
            rule.bound.as_deref().unwrap_or(""),
            rule.target,
            rule.state()
        )
        .to_lowercase();
        terms.iter().all(|term| searchable.contains(*term))
    }

    pub fn toolbar<V: 'static>(
        &self,
        options: &[DeviceOption],
        access: fn(&mut V) -> &mut Self,
        cx: &Context<V>,
    ) -> FilterToolbar {
        let mut devices = Select::new(
            "forward-device-filter",
            Some(self.device.clone()),
            cx.listener(move |v, peer: &Option<String>, _, cx| {
                access(v).device = peer.clone();
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
            Some(self.direction),
            cx.listener(move |v, direction: &Option<Direction>, _, cx| {
                access(v).direction = *direction;
                cx.notify();
            }),
        )
        .option(None, "Any direction")
        .option(Some(Direction::Local), "Forward")
        .option(Some(Direction::Remote), "Reverse");
        let mut states = Select::new(
            "forward-state-filter",
            Some(self.state),
            cx.listener(move |v, state: &Option<&'static str>, _, cx| {
                access(v).state = *state;
                cx.notify();
            }),
        )
        .option(None, "Any state");
        for state in ["Listening", "Connecting", "Paused", "Error"] {
            states = states.option(Some(state), state);
        }

        let owner = cx.weak_entity();
        let active = usize::from(self.device.is_some())
            + usize::from(self.direction.is_some())
            + usize::from(self.state.is_some());
        FilterToolbar::new(
            "forward-filters",
            "Filter forwarding rules",
            &self.search,
            active,
        )
        .group("Device", devices)
        .group("Direction", directions)
        .group("State", states)
        .clear(move |_, cx| {
            let _ = owner.update(cx, |v, cx| {
                access(v).device = None;
                access(v).direction = None;
                access(v).state = None;
                cx.notify();
            });
        })
    }
}

fn columns() -> Div {
    div().w_full().min_w_0().flex().gap_2().items_center()
}
fn cell() -> Div {
    div().min_w_0().flex_1().flex().flex_col()
}

impl Ui {
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
    state: Option<Entity<DropdownState>>,
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
            state: None,
        }
    }
    pub fn with_state(mut self, state: Entity<DropdownState>) -> Self {
        self.state = Some(state);
        self
    }
}

impl RenderOnce for RuleMenu {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let ui = Ui::new(window, cx);
        let state = self
            .state
            .unwrap_or_else(|| DropdownState::local(self.id.clone().into(), window, cx));
        let label = SharedString::from(format!("Actions for {}", self.name));
        ui.actions_menu(self.id, label, &state, self.content, cx)
    }
}
