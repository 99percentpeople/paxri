use super::{
    directory::{DeviceOption, ListFilters, contains_terms},
    *,
};
use crate::theme::BUTTON_HEIGHT;
use paxri_config::DevicePlatform;
use paxri_control::{Direction, ForwardStatus};
use std::net::SocketAddr;

pub const NARROW_WIDTH: Pixels = px(700.);

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
        id: &str,
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
        let label = rule.display_name(id);
        contains_terms(
            query,
            format!(
                "{id} {label} {display_name} {} {} {} {} {direction} {state} {}",
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

pub fn direction_summary(direction: Direction, peer: &str) -> String {
    match direction {
        Direction::Local => format!(
            "Opens a new port on this device to reach an existing service on {peer}."
        ),
        Direction::Remote => format!(
            "Opens a new port on {peer} to reach an existing service on this device."
        ),
    }
}

pub fn direction_help(peer: &str) -> String {
    format!(
        "Forward: {}\nReverse: {}",
        direction_summary(Direction::Local, peer),
        direction_summary(Direction::Remote, peer),
    )
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ForwardRowState {
    Listening,
    Connecting,
    Paused,
    Error,
}

impl ForwardRowState {
    fn of(rule: &ForwardStatus) -> Self {
        if !rule.enabled() {
            Self::Paused
        } else if rule.error.is_some() {
            Self::Error
        } else if rule.bound.is_some() {
            Self::Listening
        } else {
            Self::Connecting
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Listening => "Listening",
            Self::Connecting => "Connecting…",
            Self::Paused => "Paused",
            Self::Error => "Error",
        }
    }
}

fn compact_endpoint(addr: &str) -> String {
    for prefix in ["127.0.0.1:", "[::1]:", "localhost:"] {
        if let Some(port) = addr.strip_prefix(prefix) {
            return format!(":{port}");
        }
    }
    addr.to_owned()
}

fn compact_narrow_route(direction: Direction, listen: &str, target: &str, peer: &str) -> String {
    let listen = compact_endpoint(listen);
    let target = compact_endpoint(target);
    match direction {
        Direction::Local => format!("{listen} → {peer} {target}"),
        Direction::Remote => format!("{peer} {listen} → {target}"),
    }
}

fn columns() -> Div {
    div().w_full().min_w_0().flex().gap(SPACE_3).items_center()
}

fn col_rule() -> Div {
    div().min_w_0().flex_1().flex().items_center()
}

fn col_direction() -> Div {
    div().w(px(76.)).flex_none().flex().items_center()
}

fn col_device() -> Div {
    div().min_w_0().w(px(136.)).flex_none().flex().items_center()
}

fn col_route() -> Div {
    div().min_w_0().flex_1().flex().items_center()
}

fn col_state() -> Div {
    div().min_w_0().w(px(112.)).flex_none().flex().items_center()
}

fn state_dot(ui: Ui, state: ForwardRowState) -> Div {
    let dot = div().size(px(7.)).rounded_full().flex_none();
    match state {
        ForwardRowState::Listening => dot.bg(rgb(ui.colors.success)),
        ForwardRowState::Connecting => dot.bg(rgb(ui.colors.warning)),
        ForwardRowState::Paused => dot
            .border_1()
            .border_color(rgb(ui.colors.muted_foreground)),
        ForwardRowState::Error => dot.bg(rgb(ui.colors.destructive.background)),
    }
}

fn state_indicator(ui: Ui, key: &str, state: ForwardRowState, error: Option<&str>) -> Stateful<Div> {
    let text_color = match state {
        ForwardRowState::Listening => ui.colors.canvas.foreground,
        ForwardRowState::Connecting | ForwardRowState::Paused => ui.colors.muted_foreground,
        ForwardRowState::Error => ui.colors.alert.foreground,
    };
    let tip = error.unwrap_or_else(|| state.label()).to_owned();
    div()
        .id((ElementId::from("state"), SharedString::from(key.to_owned())))
        .flex_none()
        .flex()
        .items_center()
        .gap(px(6.))
        .tooltip(tooltip(tip.into()))
        .child(state_dot(ui, state))
        .child(
            single_line(div())
                .text_role(TextRole::Badge)
                .text_color(rgb(text_color))
                .child(state.label()),
        )
}

fn direction_tag(ui: Ui, label: &'static str) -> Div {
    div()
        .flex_none()
        .px(px(6.))
        .py(px(1.))
        .rounded(px(4.))
        .bg(rgb(ui.colors.secondary.background))
        .border_1()
        .border_color(rgb(ui.colors.border))
        .text_role(TextRole::Badge)
        .text_color(rgb(ui.colors.muted_foreground))
        .child(label)
}

impl Ui {
    pub fn forwarding_table(self, list: ListView, narrow: bool, rows: Vec<Div>) -> ListView {
        list.divided()
            .when(!narrow, |list| list.child(self.forwarding_header()))
            .children(rows)
    }

    pub fn forwarding_header(self) -> Div {
        let label = |text: &'static str| {
            div()
                .text_role(TextRole::Badge)
                .text_color(rgb(self.colors.muted_foreground))
                .child(text)
        };
        let header = columns()
            .child(col_rule().child(label("Rule")))
            .child(col_direction().child(label("Direction")))
            .child(col_device().child(label("Device")))
            .child(col_route().child(label("Route")))
            .child(col_state().child(label("State")));
        button::row_layout(div())
            .min_h(px(28.))
            .py(SPACE_1)
            .flex_none()
            .flex()
            .items_center()
            .gap(SPACE_2)
            .border_1()
            .border_color(transparent_black())
            .child(div().min_w_0().flex_1().child(header))
            .child(div().w(BUTTON_HEIGHT).flex_none())
    }

    pub fn forwarding_row(
        self,
        key: String,
        name: &str,
        f: &ForwardStatus,
        display_name: &str,
        platform: DevicePlatform,
        narrow: bool,
        actions: impl IntoElement,
    ) -> Div {
        let ui = self;
        let state = ForwardRowState::of(f);
        let paused = state == ForwardRowState::Paused;
        let error = (state == ForwardRowState::Error)
            .then_some(f.error.as_deref())
            .flatten();
        let direction = match f.direction {
            Direction::Local => "Forward",
            Direction::Remote => "Reverse",
        };
        let listen = f.bound.as_deref().unwrap_or(&f.listen);
        let full_route = f.route_for(display_name);
        let line = |column: &'static str, value: String, tip: String, role: TextRole| {
            single_line(div())
                .w_full()
                .text_role(role)
                .when(paused, |d| d.text_color(rgb(ui.colors.muted_foreground)))
                .id((ElementId::from(column), SharedString::from(key.clone())))
                .tooltip(tooltip(tip.into()))
                .child(value)
        };
        let title = super::list_item::list_title(name.to_owned())
            .when(paused, |d| d.text_color(rgb(ui.colors.muted_foreground)))
            .id((ElementId::from("rule"), SharedString::from(key.clone())))
            .tooltip(tooltip(name.to_owned().into()));
        let item = if narrow {
            let route = compact_narrow_route(f.direction, listen, &f.target, display_name);
            let detail = match state {
                ForwardRowState::Listening => route,
                ForwardRowState::Connecting => format!("{route} · Connecting…"),
                ForwardRowState::Paused => format!("{route} · Paused"),
                ForwardRowState::Error => format!("{route} · {}", error.unwrap_or("Error")),
            };
            let detail_color = if state == ForwardRowState::Error {
                ui.colors.alert.foreground
            } else {
                ui.colors.muted_foreground
            };
            let detail_tip = match error {
                Some(message) => format!("{direction} · {message}\n{full_route}"),
                None => format!("{direction} · {}\n{full_route}", state.label()),
            };
            let body = div()
                .w_full()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(2.))
                .child(title)
                .child(
                    div()
                        .w_full()
                        .min_w_0()
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .child(state_dot(ui, state))
                        .child(
                            single_line(div())
                                .flex_1()
                                .text_role(TextRole::Caption)
                                .text_color(rgb(detail_color))
                                .id((ElementId::from("route"), SharedString::from(key.clone())))
                                .tooltip(tooltip(detail_tip.into()))
                                .child(detail),
                        ),
                );
            ListItem::custom(body)
        } else {
            let route = format!(
                "{} → {}",
                compact_endpoint(listen),
                compact_endpoint(&f.target)
            );
            let device = col_device()
                .gap(px(6.))
                .child(
                    div()
                        .flex_none()
                        .flex()
                        .items_center()
                        .text_color(rgb(ui.colors.muted_foreground))
                        .child(Icon::device(platform).draw(14.)),
                )
                .child(line(
                    "peer",
                    display_name.to_owned(),
                    display_name.to_owned(),
                    TextRole::Body,
                ));
            let fields = columns()
                .child(col_rule().child(title))
                .child(col_direction().child(direction_tag(ui, direction)))
                .child(device)
                .child(col_route().child(line("route", route, full_route, TextRole::Label)))
                .child(col_state().child(state_indicator(ui, &key, state, error)));
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
    use super::{
        Direction, ForwardRowState, ForwardStatus, compact_endpoint, compact_narrow_route,
        direction_help, direction_summary, endpoint_labels, parse_endpoints,
    };

    #[test]
    fn compact_routes_shorten_loopback_and_preserve_explicit_hosts() {
        assert_eq!(compact_endpoint("127.0.0.1:8080"), ":8080");
        assert_eq!(compact_endpoint("[::1]:3000"), ":3000");
        assert_eq!(compact_endpoint("localhost:5173"), ":5173");
        assert_eq!(compact_endpoint("0.0.0.0:8080"), "0.0.0.0:8080");
        assert_eq!(compact_endpoint("10.0.0.5:5432"), "10.0.0.5:5432");
        assert_eq!(
            compact_narrow_route(Direction::Local, "127.0.0.1:8080", "127.0.0.1:3000", "Studio"),
            ":8080 → Studio :3000"
        );
        assert_eq!(
            compact_narrow_route(Direction::Remote, "127.0.0.1:9000", "127.0.0.1:3000", "Studio"),
            "Studio :9000 → :3000"
        );
        assert_eq!(
            compact_narrow_route(Direction::Local, "0.0.0.0:8080", "10.0.0.5:3000", "Studio"),
            "0.0.0.0:8080 → Studio 10.0.0.5:3000"
        );
    }

    #[test]
    fn row_state_classifies_listening_connecting_paused_and_error() {
        let base = ForwardStatus {
            name: None,
            listen: "127.0.0.1:8080".into(),
            bound: None,
            peer: "lab".into(),
            target: "127.0.0.1:3000".into(),
            direction: Direction::Local,
            enabled: Some(true),
            error: None,
        };
        assert_eq!(ForwardRowState::of(&base), ForwardRowState::Connecting);
        assert_eq!(
            ForwardRowState::of(&ForwardStatus {
                bound: Some("127.0.0.1:8080".into()),
                ..base.clone()
            }),
            ForwardRowState::Listening
        );
        assert_eq!(
            ForwardRowState::of(&ForwardStatus {
                error: Some("address in use".into()),
                ..base.clone()
            }),
            ForwardRowState::Error
        );
        assert_eq!(
            ForwardRowState::of(&ForwardStatus {
                enabled: Some(false),
                error: Some("stale".into()),
                ..base
            }),
            ForwardRowState::Paused
        );
    }

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
        assert!(direction_summary(Direction::Local, "Lab").contains("existing service on Lab"));
        assert!(direction_summary(Direction::Remote, "Lab").contains("new port on Lab"));
        let help = direction_help("Lab");
        assert!(help.contains("Forward: Opens a new port on this device"));
        assert!(help.contains("\nReverse: Opens a new port on Lab"));
        let (listen, target) = parse_endpoints(" 8080 ", " 3000 ").unwrap();
        assert_eq!(listen.port(), 8080);
        assert_eq!(target, "127.0.0.1:3000");
        assert!(parse_endpoints("bad", "3000").is_err());
    }
}
