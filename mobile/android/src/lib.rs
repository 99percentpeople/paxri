mod client;
mod fonts;
mod forwards;
mod keybar;
mod navigation;
mod settings;
mod sessions;
mod touch;
mod views;

use android_activity::AndroidApp;
use anyhow::Result;
use async_channel::Sender;
use gpui::{prelude::*, *};
use jni::{
    EnvUnowned, jni_sig, jni_str,
    objects::{Global, JClass, JObject, JString},
};
use paxri_config::keybar::KeybarSettings;
use paxri_terminal::{Frame, Size as PtySize};
use std::sync::{
    Arc, LazyLock, Mutex,
    atomic::{AtomicBool, Ordering},
};
use tokio::{runtime::Handle, sync::mpsc, task::JoinHandle};

use alacritty_terminal::{grid::Scroll, term::TermMode};
use client::{Client, Peers, Presence};
use navigation::Drawer;
use paxri_ui::connections::ConnectionHistory;
use paxri_ui::{
    components::{Toast, Ui},
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
use std::collections::{BTreeMap, BTreeSet};
use paxri_ui::components::directory::{DeviceFilters, SessionFilters};
use views::initial_size;
use paxri_terminal::managed::{Key as SessionKey, Request as TerminalRequest};

enum Update {
    Ready(Arc<Client>, Peers),
    Error(String),
    Devices(Peers),
    Paired(Result<Peers, String>),
    PairCode(String),
    Composing {
        id: u64,
        text: String,
        selection: std::ops::Range<usize>,
    },
    HardwareText(u64, String),
    Zoom(u64, theme::Zoom),
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
    Connected(u64),
    Output(u64, Vec<u8>, bool),
    Hosted(String, Option<Vec<SessionKey>>),
    Presence(Presence),
    ForwardChanged(String, Result<(), String>),
    ForwardRules(
        String,
        u64,
        Result<paxri_control::forwarding::Rules, String>,
    ),
    End(u64, Result<u32, String>),
}

static ACTIVITY_VISIBLE: LazyLock<tokio::sync::watch::Sender<bool>> =
    LazyLock::new(|| tokio::sync::watch::channel(false).0);

#[unsafe(no_mangle)]
pub extern "system" fn Java_org_paxri_android_PaxriActivity_activityVisibilityChanged(
    _env: EnvUnowned,
    _class: JClass,
    visible: bool,
) {
    ACTIVITY_VISIBLE.send_replace(visible);
}

#[derive(Clone, PartialEq)]
struct DeviceName {
    custom: String,
    system: String,
}

impl DeviceName {
    fn effective(&self) -> String {
        if self.custom.is_empty() {
            &self.system
        } else {
            &self.custom
        }
        .to_owned()
    }
}

static DEVICE_NAME: LazyLock<tokio::sync::watch::Sender<DeviceName>> = LazyLock::new(|| {
    tokio::sync::watch::channel(DeviceName {
        custom: String::new(),
        system: "Android".into(),
    })
    .0
});

#[unsafe(no_mangle)]
pub extern "system" fn Java_org_paxri_android_PaxriActivity_deviceNameChanged(
    mut env: EnvUnowned,
    _class: JClass,
    name: JString,
    model: JString,
) {
    env.with_env(|_| {
        let clean = |value: String| {
            let mut text: String = value.chars().filter(|c| !c.is_control()).collect();
            text.truncate(text.floor_char_boundary(256));
            text.trim().to_owned()
        };
        let system = clean(model.to_string());
        let name = DeviceName {
            custom: clean(name.to_string()),
            system: if system.is_empty() {
                "Android".into()
            } else {
                system
            },
        };
        if DEVICE_NAME.send_if_modified(|current| {
            if *current == name {
                return false;
            }
            *current = name;
            true
        }) {
            post(Update::DeviceNameChanged);
        }
        Ok::<_, jni::errors::Error>(())
    })
    .resolve::<jni::errors::ThrowRuntimeExAndDefault>();
}

static BRIDGE: Mutex<Option<Sender<Update>>> = Mutex::new(None);
static SYSTEM_UI_REM: Mutex<f32> = Mutex::new(16.);
static EDGES: Mutex<[f32; 4]> = Mutex::new([0.; 4]);
static CAN_GO_BACK: AtomicBool = AtomicBool::new(false);

fn post(event: Update) {
    if let Some(tx) = BRIDGE.lock().unwrap().as_ref() {
        let _ = tx.try_send(event);
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_org_paxri_android_PaxriActivity_initVerifier(
    mut env: EnvUnowned,
    _class: JClass,
    app: JObject,
) {
    env.with_env(|env| rustls_platform_verifier::android::init_with_env(env, app))
        .resolve::<jni::errors::ThrowRuntimeExAndDefault>();
}
#[unsafe(no_mangle)]
pub extern "system" fn Java_org_paxri_android_PairingScannerActivity_isInvitation(
    mut env: EnvUnowned,
    _class: JClass,
    code: JString,
) -> bool {
    env.with_env(|_| {
        let valid =
            paxri_native::gateway::pairing::Invitation::parse(&code.to_string()).is_ok();
        Ok::<_, jni::errors::Error>(valid)
    })
    .resolve::<jni::errors::ThrowRuntimeExAndDefault>()
}
#[unsafe(no_mangle)]
pub extern "system" fn Java_org_paxri_android_PaxriActivity_submitCode(
    mut env: EnvUnowned,
    _class: JClass,
    code: JString,
) {
    env.with_env(|_| {
        post(Update::PairCode(code.to_string()));
        Ok::<_, jni::errors::Error>(())
    })
    .resolve::<jni::errors::ThrowRuntimeExAndDefault>();
}
#[unsafe(no_mangle)]
pub extern "system" fn Java_org_paxri_android_PaxriActivity_composingTextChanged(
    mut env: EnvUnowned,
    _class: JClass,
    id: i64,
    text: JString,
    start: i32,
    end: i32,
) {
    env.with_env(|_| {
        post(Update::Composing {
            id: id as u64,
            text: text.to_string(),
            selection: start.max(0) as usize..end.max(0) as usize,
        });
        Ok::<_, jni::errors::Error>(())
    })
    .resolve::<jni::errors::ThrowRuntimeExAndDefault>();
}
#[unsafe(no_mangle)]
pub extern "system" fn Java_org_paxri_android_PaxriActivity_systemTextSizeChanged(
    _env: EnvUnowned, _class: JClass, size: f32,
) {
    if size.is_finite() && size > 0. {
        *SYSTEM_UI_REM.lock().unwrap() = size;
        post(Update::DisplayChanged);
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_org_paxri_android_PaxriActivity_insetsChanged(
    _env: EnvUnowned,
    _class: JClass,
    left: f32,
    top: f32,
    right: f32,
    bottom: f32,
) {
    *EDGES.lock().unwrap() = [left, top, right, bottom];
    post(Update::DisplayChanged);
}
#[unsafe(no_mangle)]
pub extern "system" fn Java_org_paxri_android_PaxriActivity_zoomKey(
    _env: EnvUnowned, _class: JClass, session: i64, direction: i32,
) {
    post(Update::Zoom(session as u64, match direction {
        1 => theme::Zoom::In,
        -1 => theme::Zoom::Out,
        _ => theme::Zoom::Reset,
    }));
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_org_paxri_android_PaxriActivity_goBack(
    _env: EnvUnowned,
    _class: JClass,
) -> bool {
    let open = CAN_GO_BACK.load(Ordering::Acquire);
    if open {
        post(Update::Close);
    }
    open
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_org_paxri_android_PaxriActivity_terminalKey(
    mut env: EnvUnowned,
    _class: JClass,
    session: i64,
    text: JString,
    named: bool,
    first: i32,
    held: i32,
) {
    let modifiers = |bits: i32| Modifiers {
        control: bits & 1 != 0,
        alt: bits & 2 != 0,
        shift: bits & 4 != 0,
        ..Default::default()
    };
    env.with_env(|_| {
        post(Update::Key {
            id: session as u64,
            value: text.to_string(),
            named,
            first: modifiers(first),
            held: modifiers(held),
        });
        Ok::<_, jni::errors::Error>(())
    })
    .resolve::<jni::errors::ThrowRuntimeExAndDefault>();
}

#[derive(Clone, Copy)]
enum ActivityAction<'a> {
    Toast(&'a str),
    SaveDeviceName(&'a str),
    SaveZoom(f32),
    Pair,
    Scan,
    PairingReady,
    DeviceConnections { paired: bool, connected: bool },
    HardwareText(u64, &'a str),
    BackEnabled(bool),
    Appearance(bool, &'a theme::Palette),
    TouchArea(u64, [i32; 5], &'a KeybarSettings),
    StopFling(u64),
    ShowTerminalKeyboard(u64),
}

fn load_terminal_zoom(app: &AndroidApp) -> f32 {
    gpui_mobile::android::jni::with_env(|env| {
        let raw = app.activity_as_ptr().cast();
        let activity = unsafe { env.as_cast_raw::<Global<JObject>>(&raw) }.map_err(|e| e.to_string())?;
        env.call_method(activity, jni_str!("terminalZoom"), jni_sig!("()F"), &[])
            .and_then(|value| value.f()).map_err(|e| e.to_string())
    }).unwrap_or(1.)
}

fn activity_action(app: &AndroidApp, action: ActivityAction<'_>) -> Result<(), String> {
    gpui_mobile::android::jni::with_env(|env| {
        let raw = app.activity_as_ptr().cast();
        // NativeActivity owns this global reference for the lifetime of AndroidApp.
        let activity =
            unsafe { env.as_cast_raw::<Global<JObject>>(&raw) }.map_err(|e| e.to_string())?;
        match action {
            ActivityAction::DeviceConnections { paired, connected } => env.call_method(
                activity, jni_str!("setDeviceConnections"), jni_sig!("(ZZ)V"),
                &[paired.into(), connected.into()],
            ),
            ActivityAction::Toast(message) => {
                let text = JString::from_str(env, message).map_err(|e| e.to_string())?;
                env.call_method(activity, jni_str!("showToast"), jni_sig!("(Ljava/lang/String;)V"), &[(&text).into()])
            }
            ActivityAction::SaveZoom(scale) => env.call_method(
                activity, jni_str!("saveTerminalZoom"), jni_sig!("(F)V"), &[scale.into()],
            ),
            ActivityAction::SaveDeviceName(name) => {
                let value = JString::from_str(env, name).map_err(|e| e.to_string())?;
                env.call_method(
                    activity,
                    jni_str!("saveDeviceName"),
                    jni_sig!("(Ljava/lang/String;)V"),
                    &[(&value).into()],
                )
            }
            ActivityAction::HardwareText(id, key) => {
                let text = JString::from_str(env, key).map_err(|e| e.to_string())?;
                env.call_method(
                    activity,
                    jni_str!("terminalHardwareText"),
                    jni_sig!("(JLjava/lang/String;)V"),
                    &[(id as i64).into(), (&text).into()],
                )
            }
            ActivityAction::BackEnabled(enabled) => env.call_method(
                activity,
                jni_str!("setBackEnabled"),
                jni_sig!("(Z)V"),
                &[enabled.into()],
            ),
            ActivityAction::Appearance(dark_icons, colors) => {
                let argb = |rgb: u32| ((0xff000000 | rgb) as i32).into();
                env.call_method(activity, jni_str!("setChromeAppearance"), jni_sig!("(ZIIIII)V"),
                    &[dark_icons.into(), argb(colors.border), argb(colors.secondary.background),
                      argb(colors.input_border), argb(colors.secondary.foreground), argb(colors.terminal.ansi[4])])
            }
            ActivityAction::StopFling(id) => env.call_method(
                activity,
                jni_str!("stopTerminalFling"),
                jni_sig!("(J)V"),
                &[(id as i64).into()],
            ),
            ActivityAction::ShowTerminalKeyboard(id) => env.call_method(
                activity,
                jni_str!("showTerminalKeyboard"),
                jni_sig!("(J)V"),
                &[(id as i64).into()],
            ),
            ActivityAction::TouchArea(id, [x, y, width, height, toolbar], keys) => {
                let rows: Vec<Vec<_>> = keys.rows.iter().map(|row| row.iter().map(|key|
                    (key.as_str(), paxri_config::keybar::label(key))).collect()).collect();
                let layout = serde_json::json!({
                    "rows": rows, "row_height": keys.row_height, "haptics": keys.haptics,
                }).to_string();
                let layout = JString::from_str(env, layout).map_err(|e| e.to_string())?;
                env.call_method(activity, jni_str!("setTerminalTouchArea"),
                    jni_sig!("(JIIIIILjava/lang/String;)V"),
                    &[(id as i64).into(), x.into(), y.into(), width.into(), height.into(),
                      toolbar.into(), (&layout).into()])
            }
            ActivityAction::Pair => env.call_method(
                activity,
                jni_str!("requestPairingCode"),
                jni_sig!("()V"),
                &[],
            ),
            ActivityAction::Scan => {
                env.call_method(activity, jni_str!("scanPairingCode"), jni_sig!("()V"), &[])
            }
            ActivityAction::PairingReady => env.call_method(
                activity,
                jni_str!("pairingClientReady"),
                jni_sig!("()V"),
                &[],
            ),
        }
        .map(|_| ())
        .map_err(|e| e.to_string())
    })
}

struct Session {
    id: u64,
    key: SessionKey,
    peer: String,
    name: SharedString,
    state: SessionState,
    screen: Emulator,
    wheel: paxri_ui::terminal::mouse::Wheel,
    composing: Preedit,
    title: Entity<TabTitle>,
    input: mpsc::UnboundedSender<Frame>,
    job: JoinHandle<()>,
}

enum SessionState {
    Connecting,
    Connected,
    Ended(String),
}

impl Session {
    fn connected(&self) -> bool {
        matches!(self.state, SessionState::Connected)
    }

    fn labels(&self, cx: &App) -> (SharedString, String) {
        let number = self.key.number;
        let subtitle = match &self.state {
            SessionState::Connected => format!("Session {number}"),
            SessionState::Connecting => format!("Session {number} · Connecting…"),
            SessionState::Ended(reason) => format!("Session {number} · {reason}"),
        };
        (self.title.read(cx).label(&self.name), subtitle)
    }
}
impl Drop for Session {
    fn drop(&mut self) {
        self.job.abort();
    }
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
    touch_area: Option<(u64, [i32; 5], Arc<KeybarSettings>)>,
    rt: Handle,
    client: Option<Arc<Client>>,
    updates: Sender<Update>,
    peers: Peers,
    presence: Presence,
    connection_history: ConnectionHistory,
    forwards: forwards::ForwardList,
    rule_form: forwards::RuleForm,
    sessions: BTreeMap<u64, Session>,
    last_session_number: BTreeMap<String, usize>,
    hosted: BTreeMap<String, Vec<SessionKey>>,
    listing_peers: BTreeSet<String>,
    session_list_errors: BTreeSet<String>,
    filter_menu: Entity<paxri_ui::components::DropdownState>,
    device_filters: DeviceFilters,
    session_filters: SessionFilters,
    terminal_return: Page,
    page: Page,
    drawer: Drawer,
    name_input: Entity<paxri_ui::components::input::Input>,
    keybar: Arc<KeybarSettings>,
    key_editor: Option<keybar::Editor>,
    keybar_menu: Entity<paxri_ui::components::DropdownState>,
    terminal_settings: paxri_ui::settings::terminal::TerminalSettings,
    terminal_menus: [Entity<paxri_ui::components::DropdownState>; 3],
    settings_scroll: ScrollHandle,
    bottom_inset: f32,
    dark_system_bar_icons: Option<bool>,
    editing_input: Option<Entity<paxri_ui::components::input::Input>>,
    generation: u64,
    starting_network: bool,
    pairing: bool,
}

impl Mobile {
    fn apply_zoom(&mut self, zoom: theme::Zoom, save: bool, cx: &mut Context<Self>) {
        let scale = zoom.scale(self.terminal_zoom);
        if self.terminal_zoom != scale {
            self.terminal_zoom = scale;
            self.update_session_fonts();
            cx.notify();
        }
        if save && let Err(error) = activity_action(&self.android, ActivityAction::SaveZoom(scale)) {
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
            if let Some(zoom) = theme::Zoom::from_key(&event.keystroke) {
                let _ = mobile.update(cx, |view, cx| {
                    if matches!(view.page, Page::Terminal(_)) && !view.drawer.open {
                        view.apply_zoom(zoom, true, cx);
                        window.prevent_default();
                        cx.stop_propagation();
                    }
                });
            }
        });
        cx.on_release(move |_, _| drop(subscription)).detach();
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
            self.set_touch_area(0, [0; 5], self.keybar.clone());
            self.key_editor = None;
        }
        if let Page::Terminal(id) = self.page {
            self.set_touch_area(0, [0; 5], self.keybar.clone());
            gpui_mobile::set_text_input_callback(None);
            gpui_mobile::hide_keyboard();
            if let Some(session) = self.sessions.get_mut(&id) {
                session.composing.clear();
            }
        }
        if let Page::Terminal(id) = &page
            && let Some(session) = self.sessions.get(id)
            && session.connected()
        {
            Self::start_input(session);
        }
        if matches!(page, Page::Terminal(_)) && !matches!(self.page, Page::Terminal(_)) {
            self.terminal_return = self.page.clone();
        }
        if self.page != page {
            self.filter_menu.update(cx, |menu, cx| menu.dismiss(cx));
            self.settings_scroll.set_offset(Point::default());
        }
        self.page = page;
        self.watch_current_page();
        self.refresh_sessions();
        self.sync_forwards(false);
        self.drawer.set_open(false);
        self.sync_back(cx);
    }

    fn watch_current_page(&self) {
        if let Some(client) = &self.client {
            client.watch_peer(match &self.page {
                Page::Peer(id) => Some(id),
                Page::Terminal(id) => self.sessions.get(id).map(|s| s.peer.as_str()),
                _ => None,
            }, matches!(self.page, Page::Sessions(_)));
        }
    }

    fn sync_back(&self, cx: &App) {
        let enabled = self.drawer.open || self.page != Page::Devices || self.filter_menu.read(cx).is_open();
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
            Page::SettingsCategory(_) => Page::Settings,
            Page::ForwardEdit { ref scope, .. } => Page::Forwards(scope.clone()),
            Page::Forwards(Some(ref id)) => Page::Peer(id.clone()),
            Page::Sessions(Some(ref peer)) => Page::Peer(peer.clone()),
            Page::Terminal(_) => self.terminal_return.clone(),
            _ => Page::Devices,
        };
        self.navigate(page, cx);
    }

    fn assign_terminal_defaults(&self, cx: &mut App) {
        let titles = self
            .sessions
            .values()
            .map(|s| (s.name.clone(), s.title.clone()))
            .collect::<Vec<_>>();
        TabTitle::assign_defaults(&titles, cx);
    }

    fn refresh_sessions(&mut self) {
        let Some(client) = &self.client else { return };
        for (peer, saved) in &self.peers {
            match &self.page {
                Page::Sessions(_) => {}
                Page::Peer(selected) if selected == peer => {}
                _ => continue,
            }
            if !self.presence.online(peer) || !saved.capabilities.is_some_and(|c| c.managed_terminals)
                || !self.listing_peers.insert(peer.clone()) { continue; }
            let peer = peer.clone();
            let client = client.clone();
            let updates = self.updates.clone();
            self.rt.spawn(async move {
                let keys = tokio::time::timeout(std::time::Duration::from_secs(2), client.manage_terminal(&peer, TerminalRequest::List))
                    .await.ok().and_then(Result::ok);
                let _ = updates.send(Update::Hosted(peer, keys)).await;
            });
        }
    }

    fn end_session(&mut self, id: u64, cx: &mut Context<Self>) {
        let Some(session) = self.sessions.get(&id) else { return; };
        if matches!(session.state, SessionState::Ended(_)) {
            self.back(cx);
            self.sessions.remove(&id);
            cx.notify();
        } else {
            let _ = session.input.send(Frame::Exit(0));
        }
    }

    fn open(&mut self, peer: String, saved_key: Option<SessionKey>, window: &Window, cx: &mut Context<Self>) {
        let Some(client) = self.client.clone() else {
            return;
        };
        let Some(saved) = self.peers.get(&peer) else {
            return;
        };
        if !saved.supports_shell() || !self.presence.online(&peer) {
            return;
        }
        let name = saved.name.clone().into();
        let number = self.last_session_number.entry(peer.clone()).or_default();
        let key = match saved_key {
            Some(key) => key,
            None => {
                let highest = self.hosted.get(&peer).into_iter().flatten().map(|k| k.number).max().unwrap_or(0);
                *number = (*number).max(highest).saturating_add(1);
                match SessionKey::new(*number) {
                    Ok(key) => key,
                    Err(e) => { Toast::show(e.to_string(), self.window, cx); return; }
                }
            }
        };
        self.sessions.retain(|_, s| s.peer != peer || s.key != key || !matches!(s.state, SessionState::Ended(_)));
        self.generation += 1;
        let id = self.generation;
        let font = self.font.clone().scaled(self.terminal_zoom);
        let grid = initial_size(window, font.cell, &self.keybar);
        let size = PtySize {
            cols: grid.cols,
            rows: grid.rows,
        };
        let (input, keys) = mpsc::unbounded_channel();
        let out = self.updates.clone();
        let dest = peer.clone();
        let job = self.rt.spawn(async move {
            let action = if saved_key.is_some() { TerminalRequest::Attach(key, size) } else { TerminalRequest::Create(key, size) };
            let result = client.terminal(&dest, action, keys, out.clone(), id).await;
            let _ = out
                .send(Update::End(id, result.map_err(|e| format!("{e:#}"))))
                .await;
        });
        self.sessions.insert(
            id,
            Session {
                id,
                key,
                peer,
                name,
                state: SessionState::Connecting,
                screen: Emulator::with_palette(
                    grid,
                    &Ui::new(window, cx).colors.terminal,
                    font,
                ),
                wheel: Default::default(),
                composing: Preedit::default(),
                title: cx.new(|_| TabTitle::default()),
                input,
                job,
            },
        );
        self.assign_terminal_defaults(cx);
        self.navigate(Page::Terminal(id), cx);
        cx.notify();
    }
    fn start_input(s: &Session) {
        let id = s.id;
        gpui_mobile::set_text_input_callback(Some(Box::new(move |text| {
            if text.is_empty() {
                return;
            }
            post(Update::HardwareText(id, text.to_owned()));
        })));
    }
    fn update(&mut self, event: Update, cx: &mut Context<Self>) {
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
            Update::PairCode(code) => {
                let Some(client) = self.client.clone() else {
                    return;
                };
                if self.pairing {
                    return;
                }
                if self.page == Page::PairingMenu {
                    self.navigate(Page::Devices, cx);
                }
                self.pairing = true;
                let tx = self.updates.clone();
                self.rt.spawn(async move {
                    let outcome = client.pair(&code).await;
                    let _ = tx
                        .send(Update::Paired(outcome.map_err(|e| format!("{e:#}"))))
                        .await;
                });
            }
            Update::Paired(result) => {
                self.pairing = false;
                match result {
                    Ok(peers) => {
                        self.peers = peers;
                        Toast::show("Device paired", self.window, cx);
                    }
                    Err(error) => Toast::show(error, self.window, cx),
                }
            }
            Update::Devices(peers) => {
                let revoked = match &mut self.page {
                    Page::Peer(id) | Page::Sessions(Some(id)) | Page::Forwards(Some(id)) => !peers.contains_key(id),
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
                    | Page::Sessions(None) | Page::Forwards(None) => false,
                };
                if revoked {
                    self.navigate(Page::Devices, cx);
                }
                if self.peers.keys().any(|id| !peers.contains_key(id)) {
                    Toast::show("Device unpaired", self.window, cx);
                }
                self.sessions.retain(|_, s| peers.contains_key(&s.peer));
                self.hosted.retain(|peer, _| peers.contains_key(peer));
                self.session_list_errors.retain(|peer| peers.contains_key(peer));
                if self.session_filters.scope.device.as_ref().is_some_and(|id| !peers.contains_key(id)) {
                    self.session_filters.scope.device = None;
                }
                if matches!(&self.terminal_return, Page::Sessions(Some(id)) | Page::Peer(id) if !peers.contains_key(id)) {
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
            Update::Composing { id, text, selection } => {
                if self.page == Page::Terminal(id)
                    && let Some(s) = self.sessions.get_mut(&id).filter(|s| s.connected())
                {
                    s.composing.replace(None, &text, Some(selection));
                }
            }
            Update::Close => {
                if let Some(menu) = self
                    .terminal_menus
                    .iter()
                    .chain([
                        &self.filter_menu,
                        &self.keybar_menu,
                        &self.rule_form.device_menu,
                        &self.rule_form.direction_menu,
                    ])
                    .find(|menu| menu.read(cx).is_open())
                {
                    menu.update(cx, |menu, cx| menu.back(cx));
                } else {
                    self.back(cx);
                }
            }
            Update::Zoom(id, zoom) => {
                if self.page == Page::Terminal(id) && !self.drawer.open {
                    self.apply_zoom(zoom, true, cx);
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
                            keys::committed_text(if value == "Space" { " " } else { &value }, first, held)
                        } else {
                            let name = if value == "Esc" { "escape".into() } else { value.to_ascii_lowercase() };
                            let Some(bytes) = keys::named_key(&name, first, s.screen.term.mode().contains(TermMode::APP_CURSOR))
                            else { return };
                            bytes
                        }
                    } else {
                        keys::committed_text(&value, first, held)
                    };
                    s.screen.input_started();
                    s.screen.scroll(Scroll::Bottom);
                    let _ = s.input.send(Frame::Data(bytes));
                }
            }
            Update::Connected(id) => {
                if let Some(s) = self.sessions.get_mut(&id) {
                    s.state = SessionState::Connected;
                    if self.page == Page::Terminal(id) {
                        Self::start_input(s);
                    }
                }
            }
            Update::Hosted(peer, keys) => {
                self.listing_peers.remove(&peer);
                if !self.peers.contains_key(&peer) { return; }
                if let Some(keys) = keys {
                    let recovered = self.session_list_errors.remove(&peer);
                    if !recovered && self.hosted.get(&peer) == Some(&keys) { return; }
                    self.hosted.insert(peer, keys);
                } else if !self.session_list_errors.insert(peer) { return; }
            }
            Update::Output(id, bytes, replay) => {
                if let Some(s) = self.sessions.get_mut(&id) {
                    s.screen.process(&bytes);
                    let reply = s.screen.finish_batch();
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
                if let Page::ForwardEdit { owner: Some(peer), scope } = &self.page
                    && peer == &owner
                {
                    match result {
                        Ok(()) => self.navigate(Page::Forwards(scope.as_ref().map(|_| owner)), cx),
                        Err(error) => Toast::show(error, self.window, cx),
                    }
                } else {
                    // A list opened while the save was pending may still contain its previous rules.
                    self.forwards.invalidate(&owner);
                    self.sync_forwards(false);
                }
            }
            Update::ForwardRules(peer, request, result) => {
                let error = result.as_ref().err().cloned();
                if !self.forwards.complete(&peer, request, result) {
                    return;
                }
                if let Some(error) = error {
                    Toast::show(error, self.window, cx);
                }
            }
            Update::Presence(snapshot) => {
                self.connection_history.record(
                    snapshot
                        .paths
                        .iter()
                        .map(|(id, paths)| (id.as_str(), paths.as_slice())),
                );
                self.presence = snapshot;
                self.refresh_sessions();
                if matches!(self.page, Page::Settings | Page::SettingsCategory(_)) {
                    return;
                }
            }
            Update::End(id, result) => {
                if let Some(s) = self.sessions.get_mut(&id) {
                    s.composing.clear();
                    if self.page == Page::Terminal(id) {
                        gpui_mobile::set_text_input_callback(None);
                        gpui_mobile::hide_keyboard();
                    }
                    s.state = SessionState::Ended(match result {
                        Ok(code) => format!("Exited · {code}"),
                        Err(e) => e,
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
    Application::with_platform(platform.into_rc()).run(move |cx| {
        let (preferences, settings_error) = match settings::load(&android) {
            Ok(settings) => (settings, None),
            Err(error) => (
                theme::GuiSettings::default(),
                Some(format!("Loading settings: {error:#}")),
            ),
        };
        theme::install(preferences, cx);
        let activity = android.clone();
        Toast::set_native(move |message| {
            activity_action(&activity, ActivityAction::Toast(message)).is_ok()
        }, cx);
        let (tx, rx) = async_channel::bounded(128);
        // A busy PTY must not crowd out Back, pairing or window-inset events.
        let (controls, platform_events) = async_channel::unbounded();
        *BRIDGE.lock().unwrap() = Some(controls);
        let disk = android.internal_data_path();
        let events = tx.clone();
        let network_activity = android.clone();
        handle.spawn(async move {
            let result = async {
                let dir = disk.ok_or_else(|| anyhow::anyhow!("App storage unavailable"))?;
                Client::start(&dir, events.clone(), move |paired, connected| {
                    if let Err(error) = activity_action(&network_activity, ActivityAction::DeviceConnections { paired, connected }) {
                        log::warn!("Updating background connection service: {error}");
                    }
                }).await
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
                cx.spawn(async move |this, cx| {
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
                })
                .detach();
                Mobile::bind_terminal_shortcuts(cx);
                let ui_font = fonts::init(cx);
                let font = cx
                    .global::<theme::fonts::MonospaceFonts>()
                    .resolve(&theme::current(cx).terminal);
                let terminal_settings =
                    paxri_ui::settings::terminal::TerminalSettings::new(|view: &mut Mobile, change, cx| { view.change_gui(change, cx); }, cx);
                let names = DEVICE_NAME.borrow().clone();
                let name_input = paxri_ui::components::input::Input::setting(&names.system, cx);
                name_input.update(cx, |input, cx| input.set_text(&names.custom, cx));
                use gpui_mobile::KeyboardType;
                let filter_menu = paxri_ui::components::DropdownState::new(window, cx);
                cx.observe(&filter_menu, |this: &mut Mobile, _, cx| this.sync_back(cx))
                    .detach();
                let device_filters = DeviceFilters::new("Search devices…", cx);
                let session_filters = SessionFilters::new("Search sessions…", cx);
                let mut inputs = vec![
                    (name_input.clone(), KeyboardType::Default),
                    (device_filters.search.clone(), KeyboardType::Default),
                    (session_filters.search.clone(), KeyboardType::Default),
                ];
                let rule_form = forwards::RuleForm::new(window, cx);
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
                    android,
                    window: window.window_handle(),
                    ui_font,
                    font,
                    touch_area: None,
                    rt: handle,
                    client: None,
                    updates: tx,
                    peers: Peers::new(),
                    presence: Presence::default(),
                    connection_history: ConnectionHistory::default(),
                    sessions: BTreeMap::new(),
                    last_session_number: BTreeMap::new(),
                    hosted: BTreeMap::new(),
                    listing_peers: BTreeSet::new(),
                    session_list_errors: BTreeSet::new(),
                    filter_menu,
                    device_filters,
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
                    keybar_menu: paxri_ui::components::DropdownState::new(window, cx),
                    terminal_settings,
                    terminal_menus: std::array::from_fn(|_| {
                        paxri_ui::components::DropdownState::new(window, cx)
                    }),
                    generation: 0,
                    starting_network: true,
                    pairing: false,
                }
            })
        })
        .expect("open Android window");
        cx.activate(true);
    });
    CAN_GO_BACK.store(false, Ordering::Release);
    gpui_mobile::set_text_input_callback(None);
    BRIDGE.lock().unwrap().take();
    rt.shutdown_background();
}
