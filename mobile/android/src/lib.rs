mod bridge;
mod client;
mod fonts;
mod forwards;
mod keybar;
mod navigation;
mod sessions;
mod settings;
mod touch;
mod views;

use alacritty_terminal::term::TermMode;
use android_activity::AndroidApp;
use anyhow::Result;
use async_channel::Sender;
use bridge::*;
use client::{Client, Peers, Presence};
use gpui::{prelude::*, *};
use navigation::Drawer;
use paxri_config::keybar::KeybarSettings;
use paxri_config::shortcuts::{TerminalAction, TerminalControls};
use paxri_terminal::managed::{Key as SessionKey, Request as TerminalRequest};
use paxri_terminal::{Frame, Size as PtySize};
use paxri_ui::{
    components::{
        ActiveDropdown, DropdownState, Toast, Ui,
        directory::{DeviceFilters, SessionFilters},
        forwarding::ForwardFilters,
    },
    connections::ConnectionHistory,
    terminal::{
        cursor::Preedit,
        emulator::{Emulator, GridSize},
        keys,
        title::TabTitle,
    },
    theme::{
        self,
        fonts::{CellSize, TerminalFont},
    },
};
use sessions::{AttachmentId, Session, SessionState};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, atomic::Ordering};
use tokio::{
    runtime::Handle,
    sync::{mpsc, oneshot},
    task::JoinHandle,
};
use views::initial_size;

struct PendingPairing {
    peer_id: String,
    cancel: Option<oneshot::Sender<()>>,
}

enum Update {
    Ready(Arc<Client>, Peers),
    Error(String),
    Devices(Peers),
    Paired(Result<Option<Peers>, String>),
    PairCode(String),
    Unpair { id: String, generation: Option<String> },
    Composing {
        id: u64,
        text: String,
        selection: std::ops::Range<usize>,
    },
    HardwareText(u64, String),
    Shortcut(u64, Keystroke, Option<TerminalAction>),
    Key {
        id: u64,
        value: String,
        named: bool,
        first: Modifiers,
        held: Modifiers,
    },
    DisplayChanged,
    DeviceNameChanged,
    Gesture(u64, touch::Gesture),
    Close,
    Resumed,
    Connected(AttachmentId),
    RetrySessions,
    Output(AttachmentId, Vec<u8>, bool),
    Hosted(String, Option<Vec<SessionKey>>),
    EndedHosted(String, SessionKey, Result<(), String>),
    RenameSession(String, SessionKey, String),
    Presence(Presence),
    RemoveForward { owner: String, name: String },
    ForwardChanged(String, Result<(), String>),
    ForwardRules(
        String,
        u64,
        Result<paxri_control::forwarding::Rules, String>,
    ),
    End(AttachmentId, Result<u32>),
    SessionGone(AttachmentId),
}

#[derive(Clone, PartialEq)]
enum Page {
    Devices,
    Sessions(Option<String>),
    PairingMenu,
    Settings,
    Forwards(Option<String>),
    ForwardEdit {
        owner: Option<String>,
        scope: Option<String>,
    },
    SettingsCategory(paxri_ui::settings::SettingsSection),
    Peer(String),
    Terminal(u64),
}

struct Mobile {
    android: AndroidApp,
    window: AnyWindowHandle,
    ui_font: SharedString,
    terminal_zoom: f32,
    font: TerminalFont,
    selection_handles: Option<(u64, [i32; 5], bool)>,
    touch_area: Option<(u64, [i32; 5], Arc<KeybarSettings>, bool)>,
    keyboard_on_entry: Option<u64>,
    rt: Handle,
    client: Option<Arc<Client>>,
    updates: Sender<Update>,
    peers: Peers,
    presence: Presence,
    connection_history: ConnectionHistory,
    forwards: forwards::ForwardList,
    rule_form: forwards::RuleForm,
    sessions: BTreeMap<u64, Session>,
    session_names: paxri_ui::terminal::title::SessionTitles,
    ending_hosted: BTreeSet<(String, SessionKey)>,
    last_session_number: BTreeMap<String, usize>,
    hosted: BTreeMap<String, Vec<SessionKey>>,
    // Each in-flight list tracks sessions that ended after the request started.
    listing_peers: BTreeMap<String, Vec<SessionKey>>,
    session_list_errors: BTreeSet<String>,
    forward_filters: ForwardFilters,
    device_filters: DeviceFilters,
    session_filters: SessionFilters,
    terminal_return: Page,
    page: Page,
    drawer: Drawer,
    name_input: Entity<paxri_ui::components::input::Input>,
    keybar: Arc<KeybarSettings>,
    key_editor: Option<keybar::Editor>,
    terminal_settings: paxri_ui::settings::terminal::TerminalSettings,
    settings_scroll: ScrollHandle,
    bottom_inset: f32,
    dark_system_bar_icons: Option<bool>,
    editing_input: Option<Entity<paxri_ui::components::input::Input>>,
    generation: u64,
    starting_network: bool,
    pairing: Option<PendingPairing>,
}

impl Mobile {
    fn apply_zoom(&mut self, zoom: theme::Zoom, save: bool, cx: &mut Context<Self>) {
        let scale = zoom.scale(self.terminal_zoom);
        if self.terminal_zoom != scale {
            self.terminal_zoom = scale;
            self.update_session_fonts();
            cx.notify();
        }
        if save && let Err(error) = activity_action(&self.android, ActivityAction::SaveZoom(scale))
        {
            log::warn!("Saving terminal zoom: {error}");
        }
    }

    fn update_session_fonts(&mut self) {
        let font = self.font.clone().scaled(self.terminal_zoom);
        for session in self.sessions.values_mut() {
            session.screen.font = font.clone();
        }
    }

    fn bind_terminal_shortcuts(cx: &mut Context<Self>) {
        let mobile = cx.weak_entity();
        let subscription = cx.intercept_keystrokes(move |event, window, cx| {
            let action = keys::local_action(&event.keystroke, theme::terminal_controls(cx));
            if action.is_some() || event.keystroke.modifiers.control {
                let _ = mobile.update(cx, |view, cx| {
                    if let Page::Terminal(id) = view.page
                        && !view.drawer.open
                        && !DropdownState::has_open(cx)
                        && view.terminal_shortcut(id, &event.keystroke, action, cx)
                    {
                        window.prevent_default();
                        cx.stop_propagation();
                    }
                });
            }
        });
        cx.on_release(move |_, _| drop(subscription)).detach();
    }

    fn terminal_shortcut(
        &mut self,
        id: u64,
        key: &Keystroke,
        action: Option<TerminalAction>,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(s) = self.sessions.get_mut(&id) else {
            return false;
        };
        match keys::terminal_input(key, action, &s.screen) {
            Some(keys::TerminalInput::Copy(text)) => {
                if let Err(error) = gpui_mobile::packages::clipboard::set_text(&text) {
                    Toast::show(error, self.window, cx);
                }
            }
            Some(keys::TerminalInput::Zoom(zoom)) => self.apply_zoom(zoom, true, cx),
            Some(keys::TerminalInput::Paste) => self.handle_gesture(id, touch::Gesture::Paste, cx),
            Some(keys::TerminalInput::Send(bytes)) if s.connected() => {
                s.screen.term.selection = None;
                s.write_input(bytes);
            }
            _ => return false,
        }
        cx.notify();
        true
    }

    fn bind_input(
        input: &Entity<paxri_ui::components::input::Input>,
        keyboard: gpui_mobile::KeyboardType,
        cx: &mut Context<Self>,
    ) {
        cx.subscribe(
            input,
            |this: &mut Mobile,
             field,
             _: &paxri_ui::components::input::EditingFinished,
             cx| {
                if this.editing_input.as_ref() != Some(&field) {
                    return;
                }
                if field == this.name_input {
                    this.commit_device_name(cx);
                }
                this.editing_input = None;
                gpui_mobile::set_text_input_callback(None);
                gpui_mobile::hide_keyboard();
                cx.notify();
            },
        )
        .detach();
        cx.subscribe(
            input,
            move |this: &mut Mobile,
                  field,
                  _: &paxri_ui::components::input::KeyboardRequested,
                  cx| {
                this.editing_input = Some(field.clone());
                let field = field.downgrade();
                let mut app = cx.to_async();
                gpui_mobile::set_text_input_callback(Some(Box::new(move |text| {
                    let _ = field.update(&mut app, |input, cx| {
                        input.insert_hardware_text(text, cx)
                    });
                })));
                gpui_mobile::show_keyboard_with_type(keyboard);
            },
        )
        .detach();
    }

    fn navigate(&mut self, page: Page, cx: &mut Context<Self>) {
        if let Page::Forwards(scope) = &page
            && !matches!(self.page, Page::ForwardEdit { .. })
        {
            self.forward_filters.for_device(scope.clone(), cx);
        }
        if self.page != page
            && let Some(input) = self.editing_input.take()
        {
            if self.name_input == input {
                self.commit_device_name(cx);
            } else if let Some(field) = self
                .terminal_settings
                .numbers
                .iter()
                .find(|f| f.read(cx).input == input)
            {
                field.update(cx, |field, cx| field.commit(cx));
            }
            gpui_mobile::set_text_input_callback(None);
            gpui_mobile::hide_keyboard();
        }
        if self.page == Page::SettingsCategory(keybar::SECTION) && self.page != page {
            self.finish_keybar_fields(None, cx);
            self.set_touch_area(0, [0; 5], self.keybar.clone(), true);
            self.key_editor = None;
        }
        if let Page::Terminal(id) = self.page {
            self.set_touch_area(0, [0; 5], self.keybar.clone(), true);
            gpui_mobile::set_text_input_callback(None);
            gpui_mobile::hide_keyboard();
            if let Some(session) = self.sessions.get_mut(&id) {
                let release = session.touch_mouse.end(*session.screen.term.mode());
                if !release.is_empty() {
                    let _ = session.input.send(Frame::Data(release));
                }
                session.composing.clear();
                session.screen.term.selection = None;
            }
        }
        if let Page::Terminal(id) = &page
            && let Some(session) = self.sessions.get_mut(id)
            && session.connected()
        {
            if self.page == page {
                Self::start_input(session);
            } else {
                session.detach();
            }
        }
        if matches!(page, Page::Terminal(_)) && !matches!(self.page, Page::Terminal(_)) {
            self.terminal_return = self.page.clone();
        }
        if self.page != page {
            self.keyboard_on_entry = match page {
                Page::Terminal(id) => Some(id),
                _ => None,
            };
            DropdownState::dismiss_active(cx);
            self.settings_scroll.set_offset(Point::default());
        }
        self.page = page;
        self.watch_current_page();
        self.retry_sessions();
        self.refresh_sessions();
        self.sync_forwards(false);
        self.drawer.set_open(false);
        self.sync_back(cx);
    }

    fn watch_current_page(&self) {
        if let Some(client) = &self.client {
            client.watch_peer(
                match &self.page {
                    Page::Peer(id) => Some(id),
                    Page::Terminal(id) => self.sessions.get(id).map(|s| s.peer.as_str()),
                    _ => None,
                },
                match self.page {
                    Page::Sessions(_) => Some(std::time::Duration::from_secs(3)),
                    Page::Devices | Page::Forwards(_) => Some(std::time::Duration::from_secs(9)),
                    _ => None,
                },
            );
        }
    }

    fn sync_back(&self, cx: &App) {
        let enabled = self.drawer.open || self.page != Page::Devices || DropdownState::has_open(cx);
        if CAN_GO_BACK.swap(enabled, Ordering::AcqRel) != enabled
            && let Err(error) = activity_action(&self.android, ActivityAction::BackEnabled(enabled))
        {
            log::warn!("Updating Android back navigation: {error}");
        }
    }

    fn back(&mut self, cx: &mut Context<Self>) {
        if self.drawer.open {
            self.drawer.set_open(false);
            self.sync_back(cx);
            return;
        }
        let page = match self.page {
            Page::SettingsCategory(keybar::SECTION | paxri_ui::settings::catalog::SHORTCUTS) => {
                Page::SettingsCategory(paxri_ui::settings::catalog::TERMINAL_CONTROLS)
            }
            Page::SettingsCategory(_) => Page::Settings,
            Page::ForwardEdit { ref scope, .. } => Page::Forwards(scope.clone()),
            Page::Forwards(Some(ref id)) => Page::Peer(id.clone()),
            Page::Sessions(Some(ref peer)) => Page::Peer(peer.clone()),
            Page::Terminal(_) => self.terminal_return.clone(),
            _ => Page::Devices,
        };
        self.navigate(page, cx);
    }

    fn update(&mut self, event: Update, cx: &mut Context<Self>) {
        if let Update::Connected(id)
        | Update::Output(id, ..)
        | Update::End(id, _)
        | Update::SessionGone(id) = &event
            && !self
                .sessions
                .get(&id.session)
                .is_some_and(|s| s.attachment == *id)
        {
            return;
        }
        let devices_changed = matches!(
            &event,
            Update::Presence(_) | Update::Ready(..) | Update::Devices(_) | Update::Paired(_)
        );
        match event {
            Update::Ready(client, peers) => {
                self.client = Some(client);
                self.peers = peers;
                self.watch_current_page();
                self.refresh_sessions();
                self.starting_network = false;
                if let Err(error) = activity_action(&self.android, ActivityAction::PairingReady) {
                    Toast::show(error, self.window, cx);
                }
            }
            Update::Error(error) => {
                self.starting_network = false;
                Toast::show(error, self.window, cx);
            }
            Update::Unpair { id, generation } => {
                let Some(client) = self.client.clone() else {
                    return;
                };
                let updates = self.updates.clone();
                self.rt.spawn(async move {
                    let event = match client.unpair(&id, generation.as_deref()).await {
                        Ok(peers) => Update::Devices(peers),
                        Err(err) => Update::Error(format!("Unpairing device: {err:#}")),
                    };
                    let _ = updates.send(event).await;
                });
            }
            Update::PairCode(code) => {
                let Some(client) = self.client.clone() else {
                    return;
                };
                if self.pairing.is_some() {
                    return;
                }
                let invite = match paxri_native::gateway::pairing::Invitation::parse(&code) {
                    Ok(invite) => invite,
                    Err(error) => {
                        Toast::show(error.to_string(), self.window, cx);
                        return;
                    }
                };
                let (cancel, canceled) = oneshot::channel();
                self.pairing = Some(PendingPairing {
                    peer_id: invite.ticket.endpoint_addr().id.to_string(),
                    cancel: Some(cancel),
                });
                self.navigate(Page::Devices, cx);
                let tx = self.updates.clone();
                self.rt.spawn(async move {
                    let outcome = client.pair(invite, canceled).await;
                    let _ = tx
                        .send(Update::Paired(outcome.map_err(|e| format!("{e:#}"))))
                        .await;
                });
            }
            Update::Paired(result) => {
                self.pairing = None;
                match result {
                    Ok(Some(peers)) => {
                        self.peers = peers;
                        Toast::show("Device paired", self.window, cx);
                    }
                    Ok(None) => Toast::show("Pairing canceled", self.window, cx),
                    Err(error) => Toast::show(error, self.window, cx),
                }
            }
            Update::Devices(peers) => {
                let revoked = match &mut self.page {
                    Page::Peer(id) | Page::Sessions(Some(id)) | Page::Forwards(Some(id)) => {
                        !peers.contains_key(id)
                    }
                    Page::ForwardEdit { owner, scope } => {
                        if scope.as_ref().is_some_and(|id| !peers.contains_key(id)) {
                            *scope = None;
                        }
                        let removed = owner.as_ref().is_some_and(|id| !peers.contains_key(id));
                        if removed && self.rule_form.existing.is_none() {
                            *owner = None;
                        }
                        removed && self.rule_form.existing.is_some()
                    }
                    Page::Terminal(id) => self
                        .sessions
                        .get(id)
                        .is_some_and(|s| !peers.contains_key(&s.peer)),
                    Page::Devices
                    | Page::PairingMenu
                    | Page::Settings
                    | Page::SettingsCategory(_)
                    | Page::Sessions(None)
                    | Page::Forwards(None) => false,
                };
                if revoked {
                    self.navigate(Page::Devices, cx);
                }
                if self.peers.keys().any(|id| !peers.contains_key(id)) {
                    Toast::show("Device unpaired", self.window, cx);
                }
                self.sessions.retain(|_, s| peers.contains_key(&s.peer));
                for removed in self.peers.keys().filter(|p| !peers.contains_key(*p)) {
                    if let Err(e) = self.session_names.retain(removed, |_| false) {
                        log::warn!("Removing session titles: {e}");
                    }
                }
                self.hosted.retain(|peer, _| peers.contains_key(peer));
                self.session_list_errors
                    .retain(|peer| peers.contains_key(peer));
                if self
                    .forward_filters
                    .scope
                    .device
                    .as_ref()
                    .is_some_and(|id| !peers.contains_key(id))
                {
                    self.forward_filters.scope.device = None;
                }
                if self
                    .session_filters
                    .scope
                    .device
                    .as_ref()
                    .is_some_and(|id| !peers.contains_key(id))
                {
                    self.session_filters.scope.device = None;
                }
                if matches!(&self.terminal_return, Page::Sessions(Some(id)) | Page::Peer(id) if !peers.contains_key(id))
                {
                    self.terminal_return = Page::Sessions(None);
                }
                self.last_session_number
                    .retain(|peer, _| peers.contains_key(peer));
                self.peers = peers;
            }
            Update::DisplayChanged => {}
            Update::DeviceNameChanged => {
                let names = DEVICE_NAME.borrow().clone();
                self.name_input
                    .update(cx, |input, cx| input.set_placeholder(&names.system, cx));
            }
            Update::Gesture(id, gesture) => self.handle_gesture(id, gesture, cx),
            Update::Composing {
                id,
                text,
                selection,
            } => {
                if self.page == Page::Terminal(id)
                    && let Some(s) = self.sessions.get_mut(&id).filter(|s| s.connected())
                {
                    s.composing.replace(None, &text, Some(selection));
                }
            }
            Update::Close => {
                if !DropdownState::back_active(cx) {
                    self.back(cx);
                }
            }
            Update::Shortcut(id, key, action) => {
                if self.page == Page::Terminal(id)
                    && !self.drawer.open
                    && !DropdownState::has_open(cx)
                {
                    self.terminal_shortcut(id, &key, action, cx);
                }
            }
            Update::HardwareText(id, value) => {
                if self.page == Page::Terminal(id) {
                    let _ =
                        activity_action(&self.android, ActivityAction::HardwareText(id, &value));
                }
            }
            Update::Key {
                id,
                value,
                named,
                first,
                held,
            } => {
                if self.page == Page::Terminal(id)
                    && let Some(s) = self.sessions.get_mut(&id).filter(|s| s.connected())
                {
                    let bytes = if named {
                        if value == "Space" || value.chars().count() == 1 {
                            keys::committed_text(
                                if value == "Space" { " " } else { &value },
                                first,
                                held,
                            )
                        } else {
                            let name = if value == "Esc" {
                                "escape".into()
                            } else {
                                value.to_ascii_lowercase()
                            };
                            let Some(bytes) = keys::named_key(
                                &name,
                                first,
                                s.screen.term.mode().contains(TermMode::APP_CURSOR),
                            ) else {
                                return;
                            };
                            bytes
                        }
                    } else {
                        keys::committed_text(&value, first, held)
                    };
                    s.write_input(bytes);
                }
            }
            Update::Resumed => {
                if let Some(client) = self.client.clone() {
                    self.rt.spawn(async move { client.refresh_network().await });
                }
                for s in self.sessions.values_mut().filter(|s| s.connected()) {
                    s.detach();
                }
                self.retry_sessions();
            }
            Update::Connected(AttachmentId { session: id, .. }) => {
                if let Some(s) = self.sessions.get_mut(&id)
                    && matches!(s.state, SessionState::Connecting | SessionState::Reconnecting)
                {
                    s.state = SessionState::Connected;
                    if self.page == Page::Terminal(id) {
                        Self::start_input(s);
                    }
                }
            }
            Update::RetrySessions => {
                self.retry_sessions();
                return;
            }
            Update::RenameSession(peer, key, name) => self.rename_session(peer, key, name, cx),
            Update::EndedHosted(peer, key, result) => {
                self.ending_hosted.remove(&(peer.clone(), key));
                if !self.peers.contains_key(&peer) {
                    return;
                }
                match result {
                    Ok(()) => {
                        self.remove_hosted(&peer, key);
                        if let Some(s) = self
                            .sessions
                            .values()
                            .find(|s| s.peer == peer && s.key == key)
                        {
                            self.dismiss_session(s.attachment.session, cx);
                        }
                    }
                    Err(e) => Toast::show(format!("Couldn’t end session: {e}"), self.window, cx),
                }
                self.refresh_sessions();
            }
            Update::Hosted(peer, keys) => {
                let ended = self.listing_peers.remove(&peer).unwrap_or_default();
                if !self.peers.contains_key(&peer) || !self.presence.online(&peer) {
                    return;
                }
                if let Some(mut keys) = keys {
                    keys.retain(|k| !ended.contains(k));
                    if let Err(e) = self.session_names.retain(&peer, |k| {
                        keys.contains(&k)
                            || self.sessions.values().any(|s| {
                                s.peer == peer
                                    && s.key == k
                                    && !matches!(s.state, SessionState::Ended(_))
                            })
                    }) {
                        log::warn!("Cleaning up session titles: {e}");
                    }
                    let recovered = self.session_list_errors.remove(&peer);
                    if !recovered && self.hosted.get(&peer) == Some(&keys) {
                        return;
                    }
                    self.hosted.insert(peer, keys);
                } else if !self.session_list_errors.insert(peer) {
                    return;
                }
            }
            Update::Output(AttachmentId { session: id, .. }, bytes, replay) => {
                if let Some(s) = self.sessions.get_mut(&id) {
                    if replay {
                        s.screen = Emulator::with_palette(
                            s.grid_size(),
                            s.screen.palette,
                            s.screen.font.clone(),
                        );
                    }
                    s.screen.process(&bytes);
                    let visible = self.page == Page::Terminal(id) && *ACTIVITY_VISIBLE.borrow();
                    let reply = s.screen.finish_batch_with_clipboard(|_, text| {
                        if !replay
                            && visible
                            && theme::terminal_controls(cx).remote_clipboard
                            && let Err(error) = gpui_mobile::packages::clipboard::set_text(&text)
                        {
                            Toast::show(error, self.window, cx);
                        }
                    });
                    if !replay && !reply.is_empty() {
                        let _ = s.input.send(Frame::Data(reply));
                    }
                    if let Some(title) = s.screen.take_title_update()
                        && s.title.update(cx, |tab, _| tab.set_pty(title))
                    {
                        self.assign_terminal_defaults(cx);
                    } else if self.page != Page::Terminal(id) {
                        return;
                    }
                }
            }
            Update::RemoveForward { owner, name } => self.remove_forward(&owner, name, cx),
            Update::ForwardChanged(owner, result) => {
                self.rule_form.saving = false;
                if let Page::ForwardEdit {
                    owner: Some(peer),
                    scope,
                } = &self.page
                    && peer == &owner
                {
                    match result {
                        Ok(()) => self.navigate(Page::Forwards(scope.clone()), cx),
                        Err(error) => Toast::show(error, self.window, cx),
                    }
                } else {
                    if let Err(error) = result {
                        Toast::show(error, self.window, cx);
                    }
                    // A list opened while the save was pending may still contain its previous rules.
                    self.forwards.invalidate(&owner);
                    self.sync_forwards(true);
                }
            }
            Update::ForwardRules(peer, request, result) => {
                if !self.forwards.complete(&peer, request, result) {
                    return;
                }
            }
            Update::Presence(snapshot) => {
                self.hosted.retain(|peer, _| snapshot.online(peer));
                self.connection_history.record(
                    snapshot
                        .paths
                        .iter()
                        .map(|(id, paths)| (id.as_str(), paths.as_slice())),
                );
                for s in self.sessions.values_mut() {
                    if self.presence.online(&s.peer)
                        && !snapshot.online(&s.peer)
                        && !matches!(s.state, SessionState::Ended(_))
                    {
                        s.detach();
                    }
                }
                self.presence = snapshot;
                self.retry_sessions();
                self.refresh_sessions();
                if matches!(self.page, Page::Settings | Page::SettingsCategory(_)) {
                    return;
                }
            }
            Update::SessionGone(AttachmentId { session: id, .. }) => {
                let Some(s) = self.sessions.get(&id) else {
                    return;
                };
                if self.page == Page::Terminal(id) && !matches!(s.state, SessionState::Stopping) {
                    Toast::show("Session has ended on the gateway", self.window, cx);
                }
                self.remove_hosted(&s.peer.clone(), s.key);
                self.dismiss_session(id, cx);
            }
            Update::End(AttachmentId { session: id, .. }, result) => {
                let Some(s) = self.sessions.get_mut(&id) else {
                    return;
                };
                s.job = None;
                if result.is_ok() && matches!(s.state, SessionState::Stopping) {
                    let (peer, key) = (s.peer.clone(), s.key);
                    self.remove_hosted(&peer, key);
                    self.dismiss_session(id, cx);
                } else if result
                    .as_ref()
                    .is_err_and(|e| !e.is::<paxri_terminal::managed::Rejected>())
                {
                    if !matches!(s.state, SessionState::Stopping) {
                        s.state = SessionState::Reconnecting;
                    }
                    s.composing.clear();
                    let retry = std::time::Duration::from_secs(1);
                    s.retry_at = std::time::Instant::now() + retry;
                    let updates = self.updates.clone();
                    self.rt.spawn(async move {
                        tokio::time::sleep(retry).await;
                        let _ = updates.send(Update::RetrySessions).await;
                    });
                } else {
                    s.composing.clear();
                    if self.page == Page::Terminal(id) {
                        gpui_mobile::set_text_input_callback(None);
                        gpui_mobile::hide_keyboard();
                    }
                    s.state = SessionState::Ended(match result {
                        Ok(code) => format!("Exited · {code}"),
                        Err(e) => e.to_string(),
                    });
                }
            }
        }
        if devices_changed {
            self.sync_forwards(false);
        }
        cx.notify();
    }
}

#[unsafe(no_mangle)]
fn android_main(android: AndroidApp) {
    android_logger::init_once(
        android_logger::Config::default()
            .with_max_level(log::LevelFilter::Warn)
            .with_tag("paxri"),
    );
    gpui_mobile::android::jni::install_panic_hook();
    gpui_mobile::android::jni::init_platform(&android);
    let platform = gpui_mobile::android::jni::shared_platform().unwrap();
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("Android runtime");
    let handle = rt.handle().clone();
    // A busy PTY must not crowd out Back, pairing or window-inset events.
    let (controls, platform_events) = async_channel::unbounded();
    *BRIDGE.lock().unwrap() = Some(controls.clone());
    CAN_GO_BACK.store(false, Ordering::Release);
    Application::with_platform(platform.into_rc()).run(move |cx| {
        let (preferences, settings_error) = match settings::load(&android) {
            Ok(settings) => (settings, None),
            Err(error) => (
                theme::GuiSettings::default(),
                Some(format!("Loading settings: {error:#}")),
            ),
        };
        *HARDWARE_CONTROLS.lock().unwrap() = preferences.controls.clone();
        theme::install(preferences, cx);
        let activity = android.clone();
        Toast::set_native(
            move |message| activity_action(&activity, ActivityAction::Toast(message)).is_ok(),
            cx,
        );
        let (tx, rx) = async_channel::bounded(128);
        let disk = android.internal_data_path();
        let events = tx.clone();
        let network_activity = android.clone();
        handle.spawn(async move {
            let result = async {
                let dir = disk.ok_or_else(|| anyhow::anyhow!("App storage unavailable"))?;
                Client::start(&dir, events.clone(), move |paired, connected| {
                    if let Err(error) = activity_action(
                        &network_activity,
                        ActivityAction::DeviceConnections { paired, connected },
                    ) {
                        log::warn!("Updating background connection service: {error}");
                    }
                })
                .await
            }
            .await;
            let event = match result {
                Ok(c) => {
                    let peers = c.list().await;
                    Update::Ready(c, peers)
                }
                Err(e) => Update::Error(format!("{e:#}")),
            };
            let _ = events.send(event).await;
        });
        cx.open_window(WindowOptions::default(), move |window, cx| {
            theme::observe(window);
            if let Some(error) = settings_error {
                Toast::show(error, window.window_handle(), cx);
            }
            cx.new(|cx| {
                let event_task = cx.spawn(async move |this, cx| {
                    while let Ok(event) = tokio::select! {
                        control = platform_events.recv() => control,
                        network = rx.recv() => network,
                    } {
                        if this
                            .update(cx, |this: &mut Mobile, cx| this.update(event, cx))
                            .is_err()
                        {
                            break;
                        }
                    }
                });
                // Cancel the receiver on its owner thread before network shutdown
                // can wake it from a Tokio worker after the window has gone.
                cx.on_release(move |_, _| drop(event_task)).detach();
                cx.observe_global::<ActiveDropdown>(|this: &mut Mobile, cx| {
                    this.sync_back(cx);
                    cx.notify();
                })
                .detach();
                Mobile::bind_terminal_shortcuts(cx);
                let ui_font = fonts::init(cx);
                let font = cx
                    .global::<theme::fonts::MonospaceFonts>()
                    .resolve(&theme::current(cx).terminal);
                let terminal_settings = paxri_ui::settings::terminal::TerminalSettings::new(
                    |view: &mut Mobile, change, cx| {
                        view.change_gui(change, cx);
                    },
                    cx,
                );
                let names = DEVICE_NAME.borrow().clone();
                let name_input = paxri_ui::components::input::Input::setting(&names.system, cx);
                name_input.update(cx, |input, cx| input.set_text(&names.custom, cx));
                use gpui_mobile::KeyboardType;
                let device_filters = DeviceFilters::new("Search devices…", cx);
                let forward_filters = ForwardFilters::for_forwards(cx);
                let session_filters = SessionFilters::new("Search sessions…", cx);
                let mut inputs = vec![
                    (name_input.clone(), KeyboardType::Default),
                    (forward_filters.search.clone(), KeyboardType::Default),
                    (device_filters.search.clone(), KeyboardType::Default),
                    (session_filters.search.clone(), KeyboardType::Default),
                ];
                let rule_form = forwards::RuleForm::new(cx);
                inputs.extend(
                    rule_form
                        .inputs
                        .iter()
                        .map(|input| (input.clone(), KeyboardType::Default)),
                );
                for (field, keyboard) in terminal_settings.numbers.iter().zip([
                    KeyboardType::Decimal,
                    KeyboardType::NumberPad,
                    KeyboardType::NumberPad,
                ]) {
                    inputs.push((field.read(cx).input.clone(), keyboard));
                }
                for (input, keyboard) in inputs {
                    Mobile::bind_input(&input, keyboard, cx);
                }
                Mobile {
                    name_input,
                    terminal_zoom: load_terminal_zoom(&android),
                    session_names: paxri_ui::terminal::title::SessionTitles::open(
                        android
                            .internal_data_path()
                            .map(|dir| dir.join("session-titles.json")),
                    ),
                    ending_hosted: BTreeSet::new(),
                    android,
                    window: window.window_handle(),
                    ui_font,
                    font,
                    touch_area: None,
                    keyboard_on_entry: None,
                    selection_handles: None,
                    rt: handle,
                    client: None,
                    updates: tx,
                    peers: Peers::new(),
                    presence: Presence::default(),
                    connection_history: ConnectionHistory::default(),
                    sessions: BTreeMap::new(),
                    last_session_number: BTreeMap::new(),
                    hosted: BTreeMap::new(),
                    listing_peers: BTreeMap::new(),
                    session_list_errors: BTreeSet::new(),
                    device_filters,
                    forward_filters,
                    session_filters,
                    terminal_return: Page::Devices,
                    forwards: Default::default(),
                    rule_form,
                    page: Page::Devices,
                    drawer: Drawer::default(),
                    settings_scroll: ScrollHandle::new(),
                    bottom_inset: 0.,
                    dark_system_bar_icons: None,
                    editing_input: None,
                    keybar: theme::current(cx).keybar.unwrap_or_default(),
                    key_editor: None,
                    terminal_settings,
                    generation: 0,
                    starting_network: true,
                    pairing: None,
                }
            })
        })
        .expect("open Android window");
        cx.activate(true);
    });
    // Android may have started the replacement window before destroying this one.
    {
        let mut bridge = BRIDGE.lock().unwrap();
        if bridge
            .as_ref()
            .is_some_and(|active| active.same_channel(&controls))
        {
            bridge.take();
            CAN_GO_BACK.store(false, Ordering::Release);
        }
    }
    controls.close();
    gpui_mobile::set_text_input_callback(None);
    rt.shutdown_timeout(std::time::Duration::from_secs(1));
    gpui_mobile::android::jni::shutdown_platform();
}
