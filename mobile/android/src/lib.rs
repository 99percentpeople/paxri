mod client;
mod navigation;
mod settings;
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
use peerhatch_terminal::{Frame, Size as PtySize};
use std::sync::{
    Arc, LazyLock, Mutex,
    atomic::{AtomicBool, Ordering},
};
use tokio::{runtime::Handle, sync::mpsc, task::JoinHandle};

use alacritty_terminal::{grid::Scroll, term::TermMode};
use client::{Client, Peers, Presence};
use navigation::Drawer;
use peerhatch_ui::connections::ConnectionHistory;
use peerhatch_ui::{
    components::Ui,
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
use std::collections::BTreeMap;
use views::initial_size;

enum Update {
    Ready(Arc<Client>, Peers),
    Error(String),
    Devices(Peers),
    Paired(Result<Peers, String>),
    PairCode(String),
    Composing(String),
    HardwareText(u64, String),
    Key {
        id: u64,
        value: String,
        named: bool,
        first: Modifiers,
        held: Modifiers,
    },
    Insets,
    DeviceNameChanged,
    Gesture(u64, touch::Gesture),
    Close,
    Connected(u64),
    Output(u64, Vec<u8>),
    Presence(Presence),
    End(u64, Result<u32, String>),
}

static ACTIVITY_VISIBLE: LazyLock<tokio::sync::watch::Sender<bool>> =
    LazyLock::new(|| tokio::sync::watch::channel(false).0);

#[unsafe(no_mangle)]
pub extern "system" fn Java_org_peerhatch_android_PeerHatchActivity_activityVisibilityChanged(
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
pub extern "system" fn Java_org_peerhatch_android_PeerHatchActivity_deviceNameChanged(
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
static EDGES: Mutex<[f32; 4]> = Mutex::new([0.; 4]);
static CAN_GO_BACK: AtomicBool = AtomicBool::new(false);

fn post(event: Update) {
    if let Some(tx) = BRIDGE.lock().unwrap().as_ref() {
        let _ = tx.try_send(event);
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_org_peerhatch_android_PeerHatchActivity_initVerifier(
    mut env: EnvUnowned,
    _class: JClass,
    app: JObject,
) {
    env.with_env(|env| rustls_platform_verifier::android::init_with_env(env, app))
        .resolve::<jni::errors::ThrowRuntimeExAndDefault>();
}
#[unsafe(no_mangle)]
pub extern "system" fn Java_org_peerhatch_android_PairingScannerActivity_isInvitation(
    mut env: EnvUnowned,
    _class: JClass,
    code: JString,
) -> bool {
    env.with_env(|_| {
        let valid =
            peerhatch_native::gateway::pairing::Invitation::parse(&code.to_string()).is_ok();
        Ok::<_, jni::errors::Error>(valid)
    })
    .resolve::<jni::errors::ThrowRuntimeExAndDefault>()
}
#[unsafe(no_mangle)]
pub extern "system" fn Java_org_peerhatch_android_PeerHatchActivity_submitCode(
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
pub extern "system" fn Java_org_peerhatch_android_PeerHatchActivity_composingTextChanged(
    mut env: EnvUnowned,
    _class: JClass,
    text: JString,
) {
    env.with_env(|_| {
        post(Update::Composing(text.to_string()));
        Ok::<_, jni::errors::Error>(())
    })
    .resolve::<jni::errors::ThrowRuntimeExAndDefault>();
}
#[unsafe(no_mangle)]
pub extern "system" fn Java_org_peerhatch_android_PeerHatchActivity_insetsChanged(
    _env: EnvUnowned,
    _class: JClass,
    left: f32,
    top: f32,
    right: f32,
    bottom: f32,
) {
    *EDGES.lock().unwrap() = [left, top, right, bottom];
    post(Update::Insets);
}
#[unsafe(no_mangle)]
pub extern "system" fn Java_org_peerhatch_android_PeerHatchActivity_goBack(
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
pub extern "system" fn Java_org_peerhatch_android_PeerHatchActivity_terminalKey(
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
    SaveDeviceName(&'a str),
    Pair,
    Scan,
    PairingReady,
    HardwareText(u64, &'a str),
    BackEnabled(bool),
    DarkSystemBarIcons(bool),
    TouchArea(u64, [i32; 5]),
    StopFling(u64),
}

fn activity_action(app: &AndroidApp, action: ActivityAction<'_>) -> Result<(), String> {
    gpui_mobile::android::jni::with_env(|env| {
        let raw = app.activity_as_ptr().cast();
        // NativeActivity owns this global reference for the lifetime of AndroidApp.
        let activity =
            unsafe { env.as_cast_raw::<Global<JObject>>(&raw) }.map_err(|e| e.to_string())?;
        match action {
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
            ActivityAction::DarkSystemBarIcons(dark) => env.call_method(
                activity,
                jni_str!("setDarkSystemBarIcons"),
                jni_sig!("(Z)V"),
                &[dark.into()],
            ),
            ActivityAction::StopFling(id) => env.call_method(
                activity,
                jni_str!("stopTerminalFling"),
                jni_sig!("(J)V"),
                &[(id as i64).into()],
            ),
            ActivityAction::TouchArea(id, [x, y, width, height, toolbar]) => env.call_method(
                activity,
                jni_str!("setTerminalTouchArea"),
                jni_sig!("(JIIIII)V"),
                &[
                    (id as i64).into(),
                    x.into(),
                    y.into(),
                    width.into(),
                    height.into(),
                    toolbar.into(),
                ],
            ),
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
    number: usize,
    peer: String,
    name: SharedString,
    state: SessionState,
    screen: Emulator,
    wheel: peerhatch_ui::terminal::mouse::Wheel,
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
        let number = self.number;
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
    PairingMenu,
    Settings,
    SettingsCategory(peerhatch_ui::settings::SettingsSection),
    Peer(String),
    Terminal(u64),
}

struct Mobile {
    android: AndroidApp,
    font: TerminalFont,
    touch_area: (u64, [i32; 5]),
    rt: Handle,
    client: Option<Arc<Client>>,
    updates: Sender<Update>,
    peers: Peers,
    presence: Presence,
    connection_history: ConnectionHistory,
    sessions: BTreeMap<u64, Session>,
    last_session_number: BTreeMap<String, usize>,
    page: Page,
    drawer: Drawer,
    name_input: Entity<peerhatch_ui::components::input::Input>,
    terminal_settings: peerhatch_ui::settings::terminal::TerminalSettings,
    terminal_menus: [Entity<peerhatch_ui::components::DropdownState>; 3],
    settings_scroll: ScrollHandle,
    bottom_inset: f32,
    dark_system_bar_icons: Option<bool>,
    editing_input: Option<Entity<peerhatch_ui::components::input::Input>>,
    generation: u64,
    message: String,
    settings_error: String,
    pairing: bool,
}

impl Mobile {
    fn navigate(&mut self, page: Page, cx: &mut App) {
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
        if let Page::Terminal(id) = self.page {
            self.set_touch_area(0, [0; 5]);
            gpui_mobile::set_text_input_callback(None);
            gpui_mobile::hide_keyboard();
            if let Some(session) = self.sessions.get_mut(&id) {
                session.composing.clear();
            }
        }
        if let Page::Terminal(id) = &page
            && let Some(session) = self.sessions.get_mut(id)
        {
            session.screen.font = self.font.clone();
            if session.connected() {
                Self::start_input(session);
            }
        }
        if let Some(client) = &self.client {
            client.watch_peer(match &page {
                Page::Peer(id) => Some(id),
                Page::Terminal(id) => self.sessions.get(id).map(|s| s.peer.as_str()),
                _ => None,
            });
        }
        if self.page != page {
            self.settings_scroll.set_offset(Point::default());
        }
        self.page = page;
        self.drawer.set_open(false);
        self.sync_back();
    }

    fn sync_back(&self) {
        let enabled = self.drawer.open || self.page != Page::Devices;
        if CAN_GO_BACK.swap(enabled, Ordering::AcqRel) != enabled
            && let Err(error) = activity_action(&self.android, ActivityAction::BackEnabled(enabled))
        {
            log::warn!("Updating Android back navigation: {error}");
        }
    }

    fn back(&mut self, cx: &mut App) {
        if self.drawer.open {
            self.drawer.set_open(false);
            self.sync_back();
            return;
        }
        let page = match self.page {
            Page::SettingsCategory(_) => Page::Settings,
            Page::Terminal(id) => self
                .sessions
                .get(&id)
                .map_or(Page::Devices, |s| Page::Peer(s.peer.clone())),
            _ => Page::Devices,
        };
        self.navigate(page, cx);
    }

    fn active_mut(&mut self) -> Option<&mut Session> {
        let Page::Terminal(id) = self.page else {
            return None;
        };
        self.sessions.get_mut(&id)
    }

    fn assign_terminal_defaults(&self, cx: &mut App) {
        let titles = self
            .sessions
            .values()
            .map(|s| (s.name.clone(), s.title.clone()))
            .collect::<Vec<_>>();
        TabTitle::assign_defaults(&titles, cx);
    }

    fn open(&mut self, peer: String, window: &Window, cx: &mut Context<Self>) {
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
        *number += 1;
        self.generation += 1;
        let id = self.generation;
        let grid = initial_size(window, self.font.cell);
        let size = PtySize {
            cols: grid.cols,
            rows: grid.rows,
        };
        let (input, keys) = mpsc::unbounded_channel();
        let out = self.updates.clone();
        let dest = peer.clone();
        let job = self.rt.spawn(async move {
            let result = client.terminal(&dest, size, keys, out.clone(), id).await;
            let _ = out
                .send(Update::End(id, result.map_err(|e| format!("{e:#}"))))
                .await;
        });
        self.sessions.insert(
            id,
            Session {
                id,
                number: *number,
                peer,
                name,
                state: SessionState::Connecting,
                screen: Emulator::with_palette(
                    grid,
                    &Ui::new(window, cx).colors.terminal,
                    self.font.clone(),
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
        match event {
            Update::Ready(client, peers) => {
                self.client = Some(client);
                self.peers = peers;
                self.message.clear();
                if let Err(error) = activity_action(&self.android, ActivityAction::PairingReady) {
                    self.message = error;
                }
            }
            Update::Error(error) => self.message = error,
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
                self.message = "Approve the request on your computer.".into();
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
                        self.message.clear();
                    }
                    Err(error) => self.message = error,
                }
            }
            Update::Devices(peers) => {
                let revoked = match &self.page {
                    Page::Peer(id) => !peers.contains_key(id),
                    Page::Terminal(id) => self
                        .sessions
                        .get(id)
                        .is_some_and(|s| !peers.contains_key(&s.peer)),
                    Page::Devices
                    | Page::PairingMenu
                    | Page::Settings
                    | Page::SettingsCategory(_) => false,
                };
                if revoked {
                    self.navigate(Page::Devices, cx);
                    self.message = "Device unpaired.".into();
                }
                self.sessions.retain(|_, s| peers.contains_key(&s.peer));
                self.last_session_number
                    .retain(|peer, _| peers.contains_key(peer));
                self.peers = peers;
            }
            Update::Insets => {}
            Update::DeviceNameChanged => {
                let names = DEVICE_NAME.borrow().clone();
                self.name_input
                    .update(cx, |input, cx| input.set_placeholder(&names.system, cx));
            }
            Update::Gesture(id, gesture) => self.handle_gesture(id, gesture, cx),
            Update::Composing(text) => {
                if let Some(s) = self.active_mut().filter(|s| s.connected()) {
                    s.composing.replace(None, &text, None);
                }
            }
            Update::Close => {
                if let Some(menu) = self
                    .terminal_menus
                    .iter()
                    .find(|menu| menu.read(cx).is_open())
                {
                    menu.update(cx, |menu, cx| menu.dismiss(cx));
                } else {
                    self.back(cx);
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
                        let name = match value.as_str() {
                            "Esc" => "escape",
                            "Tab" => "tab",
                            "Enter" => "enter",
                            "↑" => "up",
                            "↓" => "down",
                            "←" => "left",
                            "→" => "right",
                            "backspace" => "backspace",
                            "home" => "home",
                            "end" => "end",
                            _ => return,
                        };
                        let Some(bytes) = keys::named_key(
                            name,
                            first,
                            s.screen.term.mode().contains(TermMode::APP_CURSOR),
                        ) else {
                            return;
                        };
                        bytes
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
            Update::Output(id, bytes) => {
                if let Some(s) = self.sessions.get_mut(&id) {
                    s.screen.process(&bytes);
                    let reply = s.screen.finish_batch();
                    if !reply.is_empty() {
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
            Update::Presence(snapshot) => {
                self.connection_history.record(
                    snapshot
                        .paths
                        .iter()
                        .map(|(id, paths)| (id.as_str(), paths.as_slice())),
                );
                self.presence = snapshot;
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
        cx.notify();
    }
}

#[unsafe(no_mangle)]
fn android_main(android: AndroidApp) {
    android_logger::init_once(
        android_logger::Config::default()
            .with_max_level(log::LevelFilter::Warn)
            .with_tag("PeerHatch"),
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
            Ok(settings) => (settings, String::new()),
            Err(error) => (
                theme::GuiSettings::default(),
                format!("Loading settings: {error:#}"),
            ),
        };
        theme::install(preferences, cx);
        let (tx, rx) = async_channel::bounded(128);
        // A busy PTY must not crowd out Back, pairing or window-inset events.
        let (controls, platform_events) = async_channel::unbounded();
        *BRIDGE.lock().unwrap() = Some(controls);
        let disk = android.internal_data_path();
        let events = tx.clone();
        handle.spawn(async move {
            let result = async {
                let dir = disk.ok_or_else(|| anyhow::anyhow!("App storage unavailable"))?;
                Client::start(&dir, events.clone()).await
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
                touch::init_fonts(cx);
                let font = cx
                    .global::<theme::fonts::MonospaceFonts>()
                    .resolve(&theme::current(cx).terminal);
                let terminal_settings =
                    peerhatch_ui::settings::terminal::TerminalSettings::new(Mobile::change_gui, cx);
                let names = DEVICE_NAME.borrow().clone();
                let name_input = peerhatch_ui::components::input::Input::setting(&names.system, cx);
                name_input.update(cx, |input, cx| input.set_text(&names.custom, cx));
                use gpui_mobile::KeyboardType;
                let mut inputs = vec![(name_input.clone(), KeyboardType::Default)];
                for (field, keyboard) in terminal_settings.numbers.iter().zip([
                    KeyboardType::Decimal,
                    KeyboardType::NumberPad,
                    KeyboardType::NumberPad,
                ]) {
                    inputs.push((field.read(cx).input.clone(), keyboard));
                }
                for (input, keyboard) in inputs {
                    cx.subscribe(
                        &input,
                        |this: &mut Mobile,
                         field,
                         _: &peerhatch_ui::components::input::EditingFinished,
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
                        &input,
                        move |this: &mut Mobile,
                              field,
                              _: &peerhatch_ui::components::input::KeyboardRequested,
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
                Mobile {
                    name_input,
                    android,
                    font,
                    touch_area: (0, [0; 5]),
                    rt: handle,
                    client: None,
                    updates: tx,
                    peers: Peers::new(),
                    presence: Presence::default(),
                    connection_history: ConnectionHistory::default(),
                    sessions: BTreeMap::new(),
                    last_session_number: BTreeMap::new(),
                    page: Page::Devices,
                    drawer: Drawer::default(),
                    settings_scroll: ScrollHandle::new(),
                    bottom_inset: 0.,
                    dark_system_bar_icons: None,
                    editing_input: None,
                    terminal_settings,
                    terminal_menus: std::array::from_fn(|_| {
                        peerhatch_ui::components::DropdownState::new(window, cx)
                    }),
                    generation: 0,
                    message: "Starting network…".into(),
                    settings_error,
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
