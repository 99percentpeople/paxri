//! Search and scope controls shared by the desktop and Android directories.
use super::{input::Input, *};
use paxri_config::DevicePlatform;
use std::collections::BTreeMap;

#[derive(Default)]
pub struct SessionScope {
    pub device: Option<String>,
    pub state: Option<SessionLocation>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionLocation {
    Open,
    Gateway,
    Ended,
}

impl SessionLocation {
    pub fn label(self) -> &'static str {
        match self {
            Self::Open => "Open",
            Self::Gateway => "Detached",
            Self::Ended => "Ended",
        }
    }
}

/// When a local view has ended, a shell still listed by its gateway can be attached again.
pub fn session_entries<K: Ord, V>(
    name: SharedString,
    hosted: impl IntoIterator<Item = K>,
    local: impl IntoIterator<Item = (K, SharedString, bool, V)>,
) -> BTreeMap<K, (SharedString, SessionLocation, Option<V>)> {
    let mut entries: BTreeMap<_, _> = hosted
        .into_iter()
        .map(|key| (key, (name.clone(), SessionLocation::Gateway, None)))
        .collect();
    for (key, title, open, view) in local {
        let (location, target) = if open {
            (SessionLocation::Open, Some(view))
        } else if entries.contains_key(&key) {
            (SessionLocation::Gateway, None)
        } else {
            (SessionLocation::Ended, Some(view))
        };
        entries.insert(key, (title, location, target));
    }
    entries
}

pub fn named_session_entries<V>(
    name: SharedString,
    scope: &str,
    titles: &crate::terminal::title::SessionTitles,
    hosted: impl IntoIterator<Item = paxri_control::terminal::managed::Key>,
    local: impl IntoIterator<Item = (paxri_control::terminal::managed::Key, SharedString, bool, V)>,
) -> BTreeMap<paxri_control::terminal::managed::Key, (SharedString, SessionLocation, Option<V>)> {
    let mut entries = session_entries(name.clone(), hosted, local);
    for (&key, (title, _, local)) in &mut entries {
        if local.is_none()
            && let Some(saved) = titles.get(scope, key)
        {
            *title = saved.label(&name);
        }
    }
    entries
}

pub fn session_list_notice(
    total: usize,
    visible: usize,
    initial_request: bool,
    failed: &[&str],
) -> Option<ListState> {
    if !failed.is_empty() && visible == 0 {
        return Some(ListState::empty(Icon::Terminal, format!("Couldn’t refresh sessions on {}.", failed.join(", "))));
    }
    if visible != 0 { return None; }
    Some(if initial_request {
        ListState::loading("Loading sessions…")
    } else if total == 0 {
        ListState::empty(Icon::Terminal, "No sessions yet")
    } else {
        ListState::filtered("No matching sessions")
    })
}

pub struct ListFilters<T> {
    pub search: Entity<Input>,
    pub scope: T,
}

impl<T: Default + 'static> ListFilters<T> {
    pub fn new<V: 'static>(placeholder: &str, cx: &mut Context<V>) -> Self {
        let search = Input::search(placeholder, cx);
        cx.observe(&search, |_, _, cx| cx.notify()).detach();
        Self {
            search,
            scope: T::default(),
        }
    }

    pub fn reset(&mut self, cx: &mut App) {
        self.search.update(cx, |input, cx| input.set_text("", cx));
        self.scope = T::default();
    }

    pub fn query(&self, cx: &App) -> String {
        self.search.read(cx).text().trim().to_lowercase()
    }
}

pub(super) fn contains_terms(query: &str, fields: String) -> bool {
    let text = fields.to_lowercase();
    query.split_whitespace().all(|term| text.contains(term))
}

pub type DeviceFilters = ListFilters<Option<DevicePlatform>>;
pub type SessionFilters = ListFilters<SessionScope>;
pub type DeviceOption = (String, String, DevicePlatform);

impl DeviceFilters {
    pub fn matches(
        &self,
        query: &str,
        name: &str,
        alias: &str,
        id: &str,
        platform: DevicePlatform,
        state: &str,
    ) -> bool {
        self.scope.is_none_or(|system| system == platform)
            && (query.is_empty()
                || contains_terms(
                    query,
                    format!("{name} {alias} {id} {} {state}", platform.label()),
                ))
    }

    pub fn controls<V: 'static>(
        &self,
        access: fn(&mut V) -> &mut Self,
        cx: &Context<V>,
    ) -> FilterToolbar {
        let mut systems = Select::new(
            "system",
            Some(self.scope),
            cx.listener(move |v, system: &Option<DevicePlatform>, _, cx| {
                access(v).scope = *system;
                cx.notify();
            }),
        )
        .option(None, "All systems");
        for system in [
            DevicePlatform::Android,
            DevicePlatform::Windows,
            DevicePlatform::Linux,
            DevicePlatform::MacOs,
            DevicePlatform::Web,
            DevicePlatform::Unknown,
        ] {
            systems = systems.option_with_icon(Some(system), system.label(), Icon::device(system));
        }
        let owner = cx.weak_entity();
        FilterToolbar::new(
            "device-filters",
            "Filter devices",
            &self.search,
            usize::from(self.scope.is_some()),
        )
        .group("System", systems)
        .clear(move |_, cx| {
            let _ = owner.update(cx, |v, cx| {
                access(v).scope = None;
                cx.notify();
            });
        })
    }
}

impl SessionFilters {
    pub fn has_selections(&self) -> bool {
        self.scope.device.is_some() || self.scope.state.is_some()
    }

    pub fn for_device(&mut self, device: Option<String>, cx: &mut App) {
        self.reset(cx);
        self.scope.device = device;
    }

    pub fn matches(
        &self,
        query: &str,
        peer: &str,
        name: &str,
        title: &str,
        number: usize,
        state: SessionLocation,
    ) -> bool {
        self.scope.device.as_deref().is_none_or(|id| id == peer)
            && self.scope.state.is_none_or(|selected| selected == state)
            && (query.is_empty()
                || contains_terms(
                    query,
                    format!(
                        "{name} {peer} {title} Session {number} #{number} {}",
                        state.label()
                    ),
                ))
    }

    pub fn controls<V: 'static>(
        &self,
        options: &[DeviceOption],
        access: fn(&mut V) -> &mut Self,
        cx: &Context<V>,
    ) -> FilterToolbar {
        let mut devices = Select::new(
            "device",
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
        let mut states = Select::new(
            "state",
            Some(self.scope.state),
            cx.listener(move |v, state: &Option<SessionLocation>, _, cx| {
                access(v).scope.state = *state;
                cx.notify();
            }),
        )
        .option(None, "All states");
        for state in [
            SessionLocation::Open,
            SessionLocation::Gateway,
            SessionLocation::Ended,
        ] {
            states = states.option(Some(state), state.label());
        }
        let owner = cx.weak_entity();
        let active =
            usize::from(self.scope.device.is_some()) + usize::from(self.scope.state.is_some());
        FilterToolbar::new("session-filters", "Filter sessions", &self.search, active)
            .group("Device", devices)
            .group("State", states)
            .clear(move |_, cx| {
                let _ = owner.update(cx, |v, cx| {
                    access(v).scope = SessionScope::default();
                    cx.notify();
                });
            })
    }
}

impl Ui {
    pub fn session_launcher(
        self,
        options: Vec<DeviceOption>,
        open: impl Fn(&String, &mut Window, &mut App) + 'static,
    ) -> Option<AnyElement> {
        (!options.is_empty()).then(|| {
            SessionLauncher::new(self.new_session_button(), options, open)
                .above_end()
                .into_any_element()
        })
    }
}

type OpenSession = std::rc::Rc<dyn Fn(&String, &mut Window, &mut App)>;

#[derive(IntoElement)]
pub struct SessionLauncher {
    trigger: Button,
    options: Vec<DeviceOption>,
    open: OpenSession,
    above_end: bool,
}

impl SessionLauncher {
    pub fn new(
        trigger: Button,
        options: Vec<DeviceOption>,
        open: impl Fn(&String, &mut Window, &mut App) + 'static,
    ) -> Self {
        Self {
            trigger,
            options,
            open: std::rc::Rc::new(open),
            above_end: false,
        }
    }

    fn above_end(mut self) -> Self {
        self.above_end = true;
        self
    }
}

impl RenderOnce for SessionLauncher {
    fn render(mut self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let menu_id = if self.above_end {
            "new-session-menu"
        } else {
            "tab-session-menu"
        };
        let state = DropdownState::local(menu_id.into(), window, cx);
        if self.options.len() == 1 {
            let (id, _, _) = self.options.remove(0);
            if state.read(cx).is_open() {
                state.update(cx, DropdownState::dismiss);
            }
            return self
                .trigger
                .on_click(move |_, w, cx| (self.open)(&id, w, cx))
                .into_any_element();
        }
        let mut menu = DropdownContent::new();
        for (id, name, platform) in self.options {
            let open = self.open.clone();
            menu = menu.item(
                SharedString::from(id.clone()),
                Icon::device(platform).label(name),
                move |w, cx| open(&id, w, cx),
            );
        }
        let button = self.trigger.pressed(state.read(cx).is_open());
        let menu = Dropdown::new(&state, DropdownTrigger::new(button), menu);
        if self.above_end {
            menu.above_end()
        } else {
            menu.align_start()
        }
        .into_any_element()
    }
}

pub fn count_summary(
    shown: usize,
    total: usize,
    filtered: bool,
    singular: &str,
    plural: &str,
) -> String {
    let unit = if total == 1 { singular } else { plural };
    if filtered && total > 0 {
        format!("{shown} of {total} {unit}")
    } else {
        format!("{total} {unit}")
    }
}

#[cfg(test)]
mod tests {
    use super::{SessionLocation, contains_terms, count_summary, session_entries};

    #[test]
    fn session_list_preserves_local_titles_and_picks_the_live_target() {
        let entries = session_entries(
            "Device".into(),
            [1, 2, 4],
            [
                (1, "editor".into(), true, 11),
                (2, "shell".into(), false, 22),
                (3, "logs".into(), false, 33),
            ],
        );
        assert_eq!(entries.len(), 4);
        assert_eq!(
            entries[&1],
            ("editor".into(), SessionLocation::Open, Some(11))
        );
        assert_eq!(
            entries[&2],
            ("shell".into(), SessionLocation::Gateway, None)
        );
        assert_eq!(
            entries[&3],
            ("logs".into(), SessionLocation::Ended, Some(33))
        );
        assert_eq!(
            entries[&4],
            ("Device".into(), SessionLocation::Gateway, None)
        );
    }

    #[test]
    fn search_terms_can_match_different_fields_in_any_order() {
        let fields = "RaspberryPi Linux CAFE Connected".to_owned();
        assert!(contains_terms("cafe  linux rasp", fields.clone()));
        assert!(!contains_terms("rasp offline", fields));
    }

    #[test]
    fn count_summary_formats_totals_and_filtered_subsets() {
        assert_eq!(count_summary(0, 0, false, "session", "sessions"), "0 sessions");
        assert_eq!(count_summary(1, 1, false, "session", "sessions"), "1 session");
        assert_eq!(count_summary(3, 3, false, "session", "sessions"), "3 sessions");
        assert_eq!(count_summary(1, 3, true, "session", "sessions"), "1 of 3 sessions");
        assert_eq!(count_summary(0, 1, true, "rule", "rules"), "0 of 1 rule");
        assert_eq!(count_summary(0, 0, true, "rule", "rules"), "0 rules");
    }
}
