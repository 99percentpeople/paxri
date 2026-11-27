mod bridge;
mod client;
mod fonts;
mod forwards;
mod gateway;
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
use gateway::{GATEWAY, GatewayPolicy, GatewayStatus};
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
use sessions::{
    AttachmentId, LifecycleEvent, PeerConnectionState, PeerLifecycle, Session, SessionAction,
    SessionState, StreamEnd,
};
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
    GatewayStatus(GatewayStatus),
    GatewayStopped,
    Ready(Arc<Client>, Peers),
    Error(String),
    Devices(Peers),
    Paired(Result<Option<Peers>, String>),
    PairCode(String),
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
    BatteryStatus(bool),
    Resumed,
    NetworkRefreshed,
    HostConfirmed(AttachmentId),
    Connected(AttachmentId),
    RetrySession(String, u64),
    Output(AttachmentId, Vec<u8>, bool),
    Hosted(String, Option<Vec<SessionKey>>),
    EndedHosted(String, SessionKey, Result<(), String>),
    Presence(Presence),
    Reconnected {
        id: String,
        name: String,
        result: Result<(Vec<paxri_core::connection::PeerConnection>, PeerConnectionState), String>,
    },
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

enum Modal {
    Unpair {
        id: String,
        generation: Option<String>,
        name: String,
    },
    RemoveForward {
        owner: String,
        name: String,
        route: String,
    },
    PairCode {
        input: Entity<paxri_ui::components::input::Input>,
    },
    RenameSession {
        peer: String,
        key: SessionKey,
        input: Entity<paxri_ui::components::input::Input>,
    },
    OpenLink {
        uri: String,
    },
}

struct Mobile {
    android: AndroidApp,
    window: AnyWindowHandle,
    ui_font: Font,
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
    lifecycles: BTreeMap<String, PeerLifecycle>,
    connection_history: ConnectionHistory,
    forwards: forwards::ForwardList,
    rule_form: forwards::RuleForm,
    sessions: BTreeMap<u64, Session>,
    session_names: paxri_ui::terminal::title::SessionTitles,
    ending_hosted: BTreeSet<(String, SessionKey)>,
    reconnecting: BTreeSet<String>,
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
    modal: Option<Modal>,
    name_input: Entity<paxri_ui::components::input::Input>,
    relay_input: Entity<paxri_ui::components::input::Input>,
    saved_relay: String,
    active_relay: String,
    gateway_status: GatewayStatus,
    gateway_policy: GatewayPolicy,
    gateway_details: bool,
    data_dir: Option<std::path::PathBuf>,
    battery_unrestricted: bool,
    keybar: Arc<KeybarSettings>,
    key_editor: Option<keybar::Editor>,
    terminal_settings: paxri_ui::settings::terminal::TerminalSettings,
    settings_scroll: ScrollHandle,
    bottom_inset: f32,
    dark_system_bar_icons: Option<bool>,
    editing_input: Option<Entity<paxri_ui::components::input::Input>>,
    generation: u64,
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
                        && view.modal.is_none()
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
        let connected = self.session_connected(id);
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
            Some(keys::TerminalInput::Send(bytes)) if connected => {
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
                } else if field == this.relay_input {
                    this.commit_relay_url(cx);
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

    fn short_peer_id(&self, id: &str) -> String {
        let peers: BTreeMap<_, _> = self.peers.keys().map(|k| (k.clone(), k.clone())).collect();
        paxri_config::peer_id::short_ids(&peers)
            .remove(id)
            .unwrap_or_else(|| paxri_config::peer_id::default_prefix(id).to_owned())
    }

    fn open_modal(&mut self, modal: Modal, cx: &mut Context<Self>) {
        DropdownState::dismiss_active(cx);
        if matches!(self.page, Page::Terminal(_)) {
            self.set_touch_area(0, [0; 5], self.keybar.clone(), false);
        }
        self.modal = Some(modal);
        self.sync_back(cx);
        cx.notify();
    }

    fn dismiss_modal(&mut self, window: Option<&mut Window>, cx: &mut Context<Self>) {
        let Some(modal) = self.modal.take() else {
            return;
        };
        if let Modal::PairCode { input } | Modal::RenameSession { input, .. } = modal {
            if self.editing_input.as_ref() == Some(&input) {
                self.editing_input = None;
                gpui_mobile::set_text_input_callback(None);
                gpui_mobile::hide_keyboard();
            }
            if let Some(window) = window {
                window.blur(cx);
            }
        }
        if let Page::Terminal(id) = self.page
            && self.session_connected(id)
            && let Some(s) = self.sessions.get(&id)
        {
            Self::start_input(s);
        }
        self.sync_back(cx);
        cx.notify();
    }

    fn confirm_unpair(
        &mut self,
        id: String,
        generation: Option<String>,
        name: String,
        cx: &mut Context<Self>,
    ) {
        self.open_modal(
            Modal::Unpair {
                id,
                generation,
                name,
            },
            cx,
        );
    }

    fn confirm_remove_forward(
        &mut self,
        owner: String,
        name: String,
        route: String,
        cx: &mut Context<Self>,
    ) {
        self.open_modal(Modal::RemoveForward { owner, name, route }, cx);
    }

    fn open_link_modal(&mut self, uri: String, cx: &mut Context<Self>) {
        self.open_modal(Modal::OpenLink { uri }, cx);
    }

    fn open_pair_code_modal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let input = paxri_ui::components::input::Input::setting("Paste pairing code", cx);
        Self::bind_input(&input, gpui_mobile::KeyboardType::Default, cx);
        let focus_input = input.clone();
        self.open_modal(Modal::PairCode { input }, cx);
        window.on_next_frame(move |w, cx| {
            focus_input.update(cx, |field, cx| field.edit_at_end(w, cx));
        });
    }

    fn open_rename_session_modal(
        &mut self,
        peer: String,
        key: SessionKey,
        current: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let input = paxri_ui::components::input::Input::setting("Use terminal title", cx);
        input.update(cx, |field, cx| field.set_text(&current, cx));
        Self::bind_input(&input, gpui_mobile::KeyboardType::Default, cx);
        let focus_input = input.clone();
        self.open_modal(Modal::RenameSession { peer, key, input }, cx);
        window.on_next_frame(move |w, cx| {
            focus_input.update(cx, |field, cx| field.edit_at_end(w, cx));
        });
    }

    fn reconnect_device(&mut self, id: String, name: String, cx: &mut Context<Self>) {
        let Some(client) = self.client.clone() else {
            return;
        };
        if !self.reconnecting.insert(id.clone()) {
            return;
        }
        Toast::show(format!("Reconnecting to {name}…"), self.window, cx);
        let updates = self.updates.clone();
        self.rt.spawn(async move {
            let result = client.reconnect(&id).await.map_err(|e| format!("{e:#}"));
            let _ = updates.send(Update::Reconnected { id, name, result }).await;
        });
    }

    fn unpair_device(
        &mut self,
        id: String,
        generation: Option<String>,
        cx: &mut Context<Self>,
    ) {
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
        cx.notify();
    }

    fn submit_pair_code(&mut self, code: String, cx: &mut Context<Self>) -> bool {
        let Some(client) = self.client.clone() else {
            return false;
        };
        if self.pairing.is_some() {
            return false;
        }
        let invite = match paxri_native::gateway::pairing::Invitation::parse(&code) {
            Ok(invite) => invite,
            Err(error) => {
                Toast::show(error.to_string(), self.window, cx);
                return false;
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
        true
    }

    fn navigate(&mut self, page: Page, cx: &mut Context<Self>) {
        if self.page != page {
            self.modal = None;
        }
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
            } else if self.relay_input == input {
                self.commit_relay_url(cx);
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
        let focused_session = if let Page::Terminal(id) = &page
            && let Some(session) = self.sessions.get(id)
        {
            Some((
                session.peer.clone(),
                *id,
                self.page != page
                    && session.job.is_some()
                    && session.attempt_started_at.elapsed() >= std::time::Duration::from_secs(3),
            ))
        } else {
            None
        };
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
        if let Some((peer, id, stalled)) = focused_session {
            self.emit_lifecycle(&peer, LifecycleEvent::SessionFocused { id, stalled }, cx);
        }
        self.watch_current_page();
        self.dispatch_all_pending(cx);
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
        let enabled = self.drawer.open
            || self.modal.is_some()
            || self.page != Page::Devices
            || DropdownState::has_open(cx);
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
        if let Update::HostConfirmed(id)
        | Update::Connected(id)
        | Update::Output(id, ..)
        | Update::End(id, _)
        | Update::SessionGone(id) = &event
            && !self.session_generation_matches(*id)
        {
            return;
        }
        let devices_changed = matches!(
            &event,
            Update::Presence(_) | Update::Ready(..) | Update::Devices(_) | Update::Paired(_)
        );
        match event {
            Update::GatewayStatus(status) => {
                if status.is_running() {
                    self.active_relay = status.relay_url().unwrap_or_default().to_owned();
                }
                self.gateway_status = status;
            }
            Update::GatewayStopped => {
                self.client = None;
                self.presence = Presence::default();
                self.hosted.clear();
                self.reconnecting.clear();
                self.listing_peers.clear();
                self.session_list_errors.clear();
                let peer_ids: Vec<String> = self.lifecycles.keys().cloned().collect();
                for peer in peer_ids {
                    self.emit_lifecycle(
                        &peer,
                        LifecycleEvent::ConnectionChanged(PeerConnectionState::Disconnected),
                        cx,
                    );
                }
                self.sync_forwards(false);
            }
            Update::Ready(client, peers) => {
                self.client = Some(client);
                self.peers = peers;
                self.watch_current_page();
                self.refresh_sessions();
                if let Err(error) = activity_action(&self.android, ActivityAction::PairingReady) {
                    Toast::show(error, self.window, cx);
                }
            }
            Update::Error(error) => {
                Toast::show(error, self.window, cx);
            }
            Update::PairCode(code) => {
                self.submit_pair_code(code, cx);
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
                let removed_peers: Vec<String> = self
                    .peers
                    .keys()
                    .filter(|p| !peers.contains_key(*p))
                    .cloned()
                    .collect();
                if !removed_peers.is_empty() {
                    Toast::show("Device unpaired", self.window, cx);
                }
                for removed in &removed_peers {
                    self.emit_lifecycle(removed, LifecycleEvent::PeerRemoved, cx);
                    self.lifecycles.remove(removed);
                    if let Err(e) = self.session_names.retain(removed, |_| false) {
                        log::warn!("Removing session titles: {e}");
                    }
                }
                self.sessions.retain(|_, s| peers.contains_key(&s.peer));
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
                let mut names_changed = false;
                for s in self.sessions.values_mut() {
                    if let Some(peer) = self.peers.get(&s.peer)
                        && s.name.as_ref() != peer.name.as_str()
                    {
                        s.name = peer.name.clone().into();
                        s.title.update(cx, |title, cx| {
                            if title.clear_fallback() {
                                cx.notify();
                            }
                        });
                        names_changed = true;
                    }
                }
                if names_changed {
                    self.assign_terminal_defaults(cx);
                }
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
                    && self.session_connected(id)
                    && let Some(s) = self.sessions.get_mut(&id)
                {
                    s.composing.replace(None, &text, Some(selection));
                }
            }
            Update::Close => {
                if DropdownState::back_active(cx) {
                    return;
                }
                if self.modal.is_some() {
                    self.dismiss_modal(None, cx);
                    return;
                }
                self.back(cx);
            }
            Update::Shortcut(id, key, action) => {
                if self.page == Page::Terminal(id)
                    && !self.drawer.open
                    && self.modal.is_none()
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
                    && self.session_connected(id)
                    && let Some(s) = self.sessions.get_mut(&id)
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
            Update::BatteryStatus(unrestricted) => {
                if self.battery_unrestricted == unrestricted {
                    return;
                }
                self.battery_unrestricted = unrestricted;
            }
            Update::Resumed => {
                self.battery_unrestricted = load_battery_unrestricted(&self.android);
                let away = take_background_duration();
                let should_refresh = away.is_none_or(|elapsed| elapsed >= std::time::Duration::from_secs(2))
                    || self.sessions.values().any(|s| !self.peer_connection(&s.peer).is_connected());
                if should_refresh && let Some(client) = self.client.clone() {
                    let updates = self.updates.clone();
                    self.rt.spawn(async move {
                        client.refresh_network().await;
                        let _ = updates.send(Update::NetworkRefreshed).await;
                    });
                } else {
                    self.dispatch_all_pending(cx);
                }
            }
            Update::NetworkRefreshed => {
                let peers: Vec<String> = self.lifecycles.keys().cloned().collect();
                for peer in peers {
                    self.emit_lifecycle(&peer, LifecycleEvent::NetworkRefreshed, cx);
                }
                self.dispatch_all_pending(cx);
                self.refresh_sessions();
            }
            Update::HostConfirmed(AttachmentId {
                session: id,
                generation,
            }) => {
                if let Some(peer) = self.sessions.get(&id).map(|s| s.peer.clone()) {
                    self.emit_lifecycle(
                        &peer,
                        LifecycleEvent::HostConfirmed { id, generation },
                        cx,
                    );
                }
            }
            Update::Connected(AttachmentId {
                session: id,
                generation,
            }) => {
                if let Some(peer) = self.sessions.get(&id).map(|s| s.peer.clone()) {
                    self.emit_lifecycle(
                        &peer,
                        LifecycleEvent::SessionConnected { id, generation },
                        cx,
                    );
                }
            }
            Update::RetrySession(peer, id) => {
                self.emit_lifecycle(&peer, LifecycleEvent::RetryTimerElapsed { id }, cx);
                return;
            }
            Update::EndedHosted(peer, key, result) => {
                self.ending_hosted.remove(&(peer.clone(), key));
                if !self.peers.contains_key(&peer) {
                    return;
                }
                match result {
                    Ok(()) => {
                        self.remove_hosted(&peer, key);
                        if let Some(id) = self
                            .sessions
                            .values()
                            .find(|s| s.peer == peer && s.key == key)
                            .map(|s| s.id)
                        {
                            self.dismiss_session(id, cx);
                        }
                    }
                    Err(e) => Toast::show(format!("Couldn’t end session: {e}"), self.window, cx),
                }
                self.refresh_sessions();
            }
            Update::Hosted(peer, keys) => {
                let ended = self.listing_peers.remove(&peer).unwrap_or_default();
                if !self.peers.contains_key(&peer) || !self.peer_connection(&peer).is_connected() {
                    return;
                }
                if let Some(mut keys) = keys {
                    keys.retain(|k| !ended.contains(k));
                    let lifecycles = &self.lifecycles;
                    if let Err(e) = self.session_names.retain(&peer, |k| {
                        keys.contains(&k)
                            || self.sessions.values().any(|s| {
                                s.peer == peer
                                    && s.key == k
                                    && lifecycles
                                        .get(&s.peer)
                                        .and_then(|l| l.session_state(s.id))
                                        .is_some_and(SessionState::is_open)
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
            Update::Output(
                AttachmentId {
                    session: id,
                    generation,
                },
                bytes,
                replay,
            ) => {
                if let Some(peer) = self.sessions.get(&id).map(|s| s.peer.clone()) {
                    self.emit_lifecycle(
                        &peer,
                        LifecycleEvent::HostConfirmed { id, generation },
                        cx,
                    );
                }
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
                    let belled = s.screen.take_bell();
                    if !replay && belled && visible && theme::terminal_controls(cx).terminal_bell {
                        if s.bell.ring_in(
                            move |this: &mut Mobile| {
                                this.sessions.get_mut(&id).map(|s| &mut s.bell)
                            },
                            cx,
                        ) {
                            let _ = activity_action(&self.android, ActivityAction::TerminalBell);
                        }
                    }
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
            Update::Reconnected { id, name, result } => {
                self.reconnecting.remove(&id);
                match result {
                    Ok((paths, conn_state)) => {
                        let label = self
                            .peers
                            .get(&id)
                            .map_or(name.as_str(), |peer| peer.name.as_str());
                        self.connection_history.record_peer(&id, &paths);
                        self.presence.paths.insert(id.clone(), paths.clone());
                        self.presence.states.insert(id.clone(), conn_state);
                        Toast::show(
                            paxri_ui::connections::reconnect_toast(label, &paths),
                            self.window,
                            cx,
                        );
                        self.emit_lifecycle(
                            &id,
                            LifecycleEvent::ConnectionChanged(conn_state),
                            cx,
                        );
                        self.dispatch_all_pending(cx);
                        self.refresh_sessions();
                        self.forwards.invalidate(&id);
                        self.sync_forwards(true);
                    }
                    Err(error) => {
                        Toast::show(
                            format!("Could not reconnect to {name}: {error}"),
                            self.window,
                            cx,
                        );
                    }
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
                let mut peer_ids: BTreeSet<String> = self.peers.keys().cloned().collect();
                peer_ids.extend(snapshot.states.keys().cloned());
                peer_ids.extend(self.lifecycles.keys().cloned());
                let next_states: Vec<(String, PeerConnectionState)> = peer_ids
                    .into_iter()
                    .map(|peer| {
                        let state = snapshot.state_for(&peer);
                        (peer, state)
                    })
                    .collect();
                self.presence = snapshot;
                for (peer, state) in next_states {
                    self.emit_lifecycle(&peer, LifecycleEvent::ConnectionChanged(state), cx);
                }
                self.dispatch_all_pending(cx);
                self.refresh_sessions();
                if matches!(self.page, Page::Settings)
                    || matches!(
                        self.page,
                        Page::SettingsCategory(section)
                            if section != paxri_ui::settings::catalog::GATEWAY
                    )
                {
                    return;
                }
            }
            Update::SessionGone(AttachmentId {
                session: id,
                generation,
            }) => {
                if let Some(peer) = self.sessions.get(&id).map(|s| s.peer.clone()) {
                    self.emit_lifecycle(
                        &peer,
                        LifecycleEvent::SessionGone { id, generation },
                        cx,
                    );
                }
            }
            Update::End(
                AttachmentId {
                    session: id,
                    generation,
                },
                result,
            ) => {
                let Some(peer) = self.sessions.get(&id).map(|s| s.peer.clone()) else {
                    return;
                };
                if let Some(s) = self.sessions.get_mut(&id) {
                    s.job = None;
                }
                let end = match result {
                    Ok(code) => StreamEnd::Exited(code),
                    Err(ref e) if e.is::<paxri_terminal::managed::Rejected>() => {
                        StreamEnd::Rejected(e.to_string())
                    }
                    Err(ref e) if e.is::<client::ConnectionReplaced>() => {
                        StreamEnd::ConnectionReplaced
                    }
                    Err(ref e) if e.is::<tokio::time::error::Elapsed>() => StreamEnd::TimedOut,
                    Err(e) => StreamEnd::Disconnected(e.to_string()),
                };
                self.emit_lifecycle(
                    &peer,
                    LifecycleEvent::StreamEnded {
                        id,
                        generation,
                        end,
                    },
                    cx,
                );
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
    let handle = GATEWAY.handle();
    let disk = android.internal_data_path();
    // Load local settings/peers and attach to the process-wide embedded gateway
    // immediately so network bootstrap runs in parallel with Vulkan/GPUI init and
    // survives Activity recreation or closing the window.
    let (local_settings, settings_error) = match disk.as_deref() {
        Some(dir) => match settings::load_dir(dir) {
            Ok(settings) => (settings, None),
            Err(error) => (
                paxri_config::file::LocalSettings::default(),
                Some(format!("Loading settings: {error:#}")),
            ),
        },
        None => (
            paxri_config::file::LocalSettings::default(),
            Some("App storage unavailable".to_owned()),
        ),
    };
    let preferences = local_settings.gui;
    let gateway_policy = GatewayPolicy::from(&local_settings.gateway);
    let initial_relay = local_settings
        .native
        .relay_url
        .as_ref()
        .map(|url| url.as_str().to_owned())
        .unwrap_or_default();
    *HARDWARE_CONTROLS.lock().unwrap() = preferences.controls.clone();
    let initial_peers = disk
        .as_deref()
        .map(client::load_saved_peers)
        .unwrap_or_default();
    let (tx, rx) = async_channel::bounded(128);
    let cleanup_tx = tx.clone();
    GATEWAY.attach_ui(
        disk.clone(),
        gateway_policy,
        Arc::new(AndroidGatewayHost),
        tx.clone(),
    );
    let initial_gateway_status = GATEWAY.status();
    let active_relay = initial_gateway_status
        .relay_url()
        .map(str::to_owned)
        .unwrap_or_else(|| initial_relay.clone());
    gpui_mobile::android::jni::install_panic_hook();
    gpui_mobile::android::jni::init_platform(&android);
    let platform = gpui_mobile::android::jni::shared_platform().unwrap();
    // A busy PTY must not crowd out Back, pairing or window-inset events.
    let (controls, platform_events) = async_channel::unbounded();
    *BRIDGE.lock().unwrap() = Some(controls.clone());
    CAN_GO_BACK.store(false, Ordering::Release);
    Application::with_platform(platform.into_rc()).run(move |cx| {
        theme::install(preferences, cx);
        let activity = android.clone();
        Toast::set_native(
            move |message| activity_action(&activity, ActivityAction::Toast(message)).is_ok(),
            cx,
        );
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
                let relay_input = paxri_ui::components::input::Input::setting(
                    "Automatic public relay",
                    cx,
                );
                relay_input.update(cx, |input, cx| input.set_text(&initial_relay, cx));
                use gpui_mobile::KeyboardType;
                let device_filters = DeviceFilters::new("Search devices…", cx);
                let forward_filters = ForwardFilters::for_forwards(cx);
                let session_filters = SessionFilters::new("Search sessions…", cx);
                let mut inputs = vec![
                    (name_input.clone(), KeyboardType::Default),
                    (relay_input.clone(), KeyboardType::Default),
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
                    relay_input,
                    saved_relay: initial_relay,
                    active_relay,
                    gateway_status: initial_gateway_status,
                    gateway_policy,
                    gateway_details: false,
                    data_dir: disk,
                    battery_unrestricted: load_battery_unrestricted(&android),
                    terminal_zoom: load_terminal_zoom(&android),
                    session_names: paxri_ui::terminal::title::SessionTitles::open(
                        android
                            .internal_data_path()
                            .map(|dir| dir.join("session-titles.json")),
                    ),
                    ending_hosted: BTreeSet::new(),
                    reconnecting: BTreeSet::new(),
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
                    peers: initial_peers,
                    presence: Presence::default(),
                    lifecycles: BTreeMap::new(),
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
                    modal: None,
                    settings_scroll: ScrollHandle::new(),
                    bottom_inset: 0.,
                    dark_system_bar_icons: None,
                    editing_input: None,
                    keybar: theme::current(cx).keybar.unwrap_or_default(),
                    key_editor: None,
                    terminal_settings,
                    generation: 0,
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
    GATEWAY.detach_ui(&cleanup_tx);
    controls.close();
    gpui_mobile::set_text_input_callback(None);
    gpui_mobile::android::jni::shutdown_platform();
}
