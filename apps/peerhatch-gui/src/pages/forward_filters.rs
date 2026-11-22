use crate::{
    Dashboard, Input,
    components::{Icon, Select},
};
use gpui::{prelude::*, *};
use peerhatch_control::{Direction, ForwardStatus, Status};

pub(crate) struct ForwardFilters {
    pub(crate) device: Option<String>,
    pub(super) direction: Option<Direction>,
    state: Option<&'static str>,
    search: Entity<Input>,
}

impl ForwardFilters {
    pub(crate) fn new(cx: &mut Context<Dashboard>) -> Self {
        let search = Input::search("Search device or port…", cx);
        cx.observe(&search, |_, _, cx| cx.notify()).detach();
        Self {
            search,
            device: None,
            direction: None,
            state: None,
        }
    }

    pub(super) fn reset(&mut self, device: Option<String>, cx: &mut Context<Dashboard>) {
        self.search.update(cx, |input, cx| input.set_text("", cx));
        self.device = device;
        self.direction = None;
        self.state = None;
    }

    pub(super) fn query(&self, cx: &App) -> String {
        self.search.read(cx).text().trim().to_lowercase()
    }

    pub(super) fn has_selections(&self) -> bool {
        self.device.is_some() || self.direction.is_some() || self.state.is_some()
    }

    pub(super) fn matches(&self, name: &str, rule: &ForwardStatus, terms: &[&str]) -> bool {
        let state = if rule.enabled() && rule.error.is_some() {
            "Error"
        } else {
            rule.state().trim_end_matches('…')
        };
        if self.device.as_ref().is_some_and(|peer| *peer != rule.peer)
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
            "{name} {} {} {} {} {direction} {state} {}",
            rule.peer,
            rule.listen,
            rule.bound.as_deref().unwrap_or(""),
            rule.target,
            rule.state()
        )
        .to_lowercase();
        terms.iter().all(|term| searchable.contains(*term))
    }

    pub(super) fn toolbar(&self, status: &Status, width: Pixels, cx: &Context<Dashboard>) -> Div {
        let mut devices = Select::new(
            "forward-device-filter",
            Some(self.device.clone()),
            cx.listener(|v, peer: &Option<String>, _, cx| {
                v.forward_filters.device = peer.clone();
                cx.notify();
            }),
        )
        .option(None, "All devices");
        for name in status.peers.keys() {
            devices = devices.option_with_icon(
                Some(name.clone()),
                name.clone(),
                Icon::device(status.peer_platform(name)),
            );
        }
        let directions = Select::new(
            "forward-direction-filter",
            Some(self.direction),
            cx.listener(|v, direction: &Option<Direction>, _, cx| {
                v.forward_filters.direction = *direction;
                cx.notify();
            }),
        )
        .option(None, "Any direction")
        .option(Some(Direction::Local), "Forward")
        .option(Some(Direction::Remote), "Reverse");
        let mut states = Select::new(
            "forward-state-filter",
            Some(self.state),
            cx.listener(|v, state: &Option<&'static str>, _, cx| {
                v.forward_filters.state = *state;
                cx.notify();
            }),
        )
        .option(None, "Any state");
        for state in ["Listening", "Connecting", "Paused", "Error"] {
            states = states.option(Some(state), state);
        }

        let compact = width < px(730.);
        let field = |basis: f32| {
            div()
                .min_w_0()
                .max_w_full()
                .flex_grow()
                .flex_basis(px(basis))
        };
        let selects = div()
            .min_w_0()
            .max_w_full()
            .flex()
            .flex_wrap()
            .items_center()
            .gap_2()
            .when(compact, |d| d.w_full())
            .when(!compact, |d| d.w(px(444.)).flex_none())
            .child(field(123.).child(devices))
            .child(field(140.).child(directions))
            .child(field(119.).child(states));
        div()
            .w_full()
            .min_w_0()
            .flex()
            .items_center()
            .gap_2()
            .when(compact, |d| d.flex_col())
            .child(
                div()
                    .min_w_0()
                    .when(compact, |d| d.w_full())
                    .when(!compact, |d| d.flex_1())
                    .child(self.search.clone()),
            )
            .child(selects)
    }
}
