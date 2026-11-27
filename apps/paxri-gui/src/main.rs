//! Desktop controller. The resident gateway owns configuration and listeners.
use anyhow::Context as _;
use clap::{CommandFactory, Parser};
use gpui::{prelude::*, *};
use std::{
    collections::{BTreeMap, VecDeque},
    path::PathBuf,
    time::Duration,
};
use paxri_cli::gateway::{self, Action as GatewayAction};
use paxri_config::{
    Native, NativeChange,
    startup::{GatewayChange, GatewaySettings, SessionKeepAlive, Startup},
};
use paxri_control::{Command, GuiChange, Status};
use paxri_ui::components::directory::{DeviceFilters, SessionFilters};
use paxri_ui::connections::ConnectionHistory;
mod chrome;
mod components;
#[cfg(windows)]
use paxri_ui::ime;
mod pages;
mod presence;
mod terminal;
mod theme;
mod workspace;
use components::{Input, TextSetting};
use workspace::Workspace;

#[derive(Parser)]
#[command(
    version,
    about = "Open the paxri desktop app, or run a command in this terminal"
)]
struct Options {
    #[command(flatten)]
    gateway: paxri_cli::GatewayOptions,
    /// Node TOML for the background gateway and offline desktop settings
    #[arg(long)]
    config: Option<PathBuf>,
    #[command(subcommand)]
    command: Option<paxri_cli::Command>,
}

type OnComplete = Box<dyn FnOnce(Result<(), String>, &mut App)>;

enum Operation {
    Api(Command),
    Open,
    Manage(GatewayAction),
}

struct GatewayRequest {
    command: Operation,
    on_complete: Option<OnComplete>,
}

enum RequestResult {
    Gateway(Box<Status>, Option<Native>),
    Peer(paxri_control::PeerStatus),
    OfflineGui(theme::GuiSettings),
    OfflineGateway(GatewaySettings),
    OfflineNative(Native),
    Stopped(Option<paxri_config::file::LocalSettings>),
}

enum GatewayOffline {
    Stopped,
    Failed(String),
}

struct Dashboard {
    presence_watch: Option<(PathBuf, Task<()>)>,
    refresh_presence: bool,
    listing_forwards: bool,
    reconnecting_peers: std::collections::BTreeSet<String>,
    connection_history: ConnectionHistory,
    terminal_sessions: BTreeMap<SharedString, workspace::PeerSessions>,
    session_titles: paxri_ui::terminal::title::SessionTitles,
    home_scroll: ScrollHandle,
    devices_pane: Option<pages::DevicesPane>,
    forward_filters: pages::ForwardFilters,
    settings_editor: pages::SettingsEditor,
    name_field: TextSetting,
    relay_field: TextSetting,
    profile_native: Option<Native>,
    gateway_details: bool,
    pairing_input: Entity<Input>,
    device_filters: DeviceFilters,
    session_filters: SessionFilters,
    local_code: Entity<Input>,
    pairing_qr: Option<std::rc::Rc<qrcode::QrCode>>,
    saving: bool,
    gateway_action: Option<GatewayAction>,
    address: Option<PathBuf>,
    selected_config: Option<PathBuf>,
    #[cfg(windows)]
    startup_report: Vec<String>,
    #[cfg(windows)]
    startup_inspection: Option<Task<()>>,
    config_path: Option<PathBuf>,
    runtime: tokio::runtime::Handle,
    local_name: SharedString,
    status: Option<Status>,
    startup: Startup,
    session_keep_alive: SessionKeepAlive,
    offline: Option<GatewayOffline>,
    busy: bool,
    pending: VecDeque<GatewayRequest>,
}

impl Dashboard {
    fn sync_device_name(&mut self, name: Option<&str>, fallback: &str, cx: &mut Context<Self>) {
        self.name_field.set_placeholder(fallback, cx);
        self.name_field.sync(name.unwrap_or_default(), cx);
        let display_name = name.unwrap_or(fallback);
        if self.local_name.as_ref() != display_name {
            self.local_name = display_name.to_owned().into();
        }
    }

    fn active_address(&self) -> Option<PathBuf> {
        self.status
            .as_ref()
            .and_then(|s| s.ipc_path.clone())
            .or_else(|| self.address.clone())
    }

    #[cfg(windows)]
    fn inspect_startup(&mut self, cx: &mut Context<Self>) {
        let Some(path) = self.config_path.clone() else {
            return;
        };
        let read = self
            .runtime
            .spawn_blocking(move || gateway::startup_report(&path));
        self.startup_inspection = Some(cx.spawn(async move |this, cx| {
            let lines = read
                .await
                .unwrap_or_else(|e| vec![format!("Could not read Windows startup: {e}")]);
            let _ = this.update(cx, |view, cx| {
                if view.startup_report != lines {
                    view.startup_report = lines;
                    cx.notify();
                }
            });
        }));
    }

    fn send(&mut self, command: Command, cx: &mut Context<Self>) {
        self.request(
            GatewayRequest {
                command: Operation::Api(command),
                on_complete: None,
            },
            cx,
        );
    }

    fn send_from_dialog(
        &mut self,
        command: Command,
        window: &Window,
        on_success: components::DialogSuccess,
        cx: &mut Context<Self>,
    ) {
        if self.saving {
            return;
        }
        let dialog = window.window_handle();
        self.request(
            GatewayRequest {
                command: Operation::Api(command),
                on_complete: Some(Box::new(move |result, cx| {
                    components::Dialog::complete(dialog, result, on_success, cx);
                })),
            },
            cx,
        );
    }

    fn manage(&mut self, action: GatewayAction, cx: &mut Context<Self>) {
        if self.gateway_action.is_some()
            || self.pending.iter().any(|r| matches!(r.command, Operation::Manage(_)))
        {
            return;
        }
        self.request(
            GatewayRequest {
                command: Operation::Manage(action),
                on_complete: None,
            },
            cx,
        );
    }

    fn request(&mut self, request: GatewayRequest, cx: &mut Context<Self>) {
        let metrics = matches!(request.command, Operation::Api(Command::PeerStatus { .. }));
        let changing = !metrics && !matches!(request.command, Operation::Api(Command::Status));
        if self.busy {
            if changing {
                self.saving = true;
                self.pending.push_back(request);
                cx.notify();
            }
            return;
        }
        let GatewayRequest {
            command,
            on_complete,
        } = request;
        let saved_peer = match &command {
            Operation::Api(Command::SaveForward { rule, .. }) => Some(rule.peer.clone()),
            _ => None,
        };
        let name_saved = matches!(command, Operation::Api(Command::SetDeviceName { .. }));
        let relay_saved = matches!(
            command,
            Operation::Api(Command::SetNative {
                change: NativeChange::RelayUrl(_)
            })
        );
        let joined = matches!(command, Operation::Api(Command::JoinPairing { .. }));
        let opening = matches!(command, Operation::Open);
        self.busy = true;
        self.saving = changing || !self.pending.is_empty();
        if let Operation::Manage(action) = command {
            self.gateway_action = Some(action);
        }
        if !metrics {
            cx.notify();
        }
        let selected = self.address.clone();
        let selected_config = self.selected_config.clone();
        let path = self
            .active_address()
            .unwrap_or_else(paxri_control::default_address);
        #[cfg(windows)]
        let startup_changed = matches!(command, Operation::Manage(GatewayAction::Startup { .. }));
        let offline_path = self.config_path.clone();
        let show_forwards = self.devices_pane == Some(pages::DevicesPane::Forwards);
        let job = self.runtime.spawn(async move {
            let state = |status: Option<Status>| -> anyhow::Result<RequestResult> {
                if let Some(s) = status {
                    let legacy = if s.native.is_none() {
                        s.config_path
                            .as_deref()
                            .and_then(|p| paxri_config::file::read_local(p).ok())
                            .map(|settings| settings.native)
                    } else {
                        None
                    };
                    return Ok(RequestResult::Gateway(Box::new(s), legacy));
                }
                let local = offline_path
                    .as_deref()
                    .map(paxri_config::file::read_local)
                    .transpose()?;
                Ok(RequestResult::Stopped(local))
            };
            let command = match command {
                Operation::Api(Command::PeerStatus { peer, check }) => {
                    let snapshot = paxri_control::peer_status(&path, &peer, check).await?;
                    return Ok(RequestResult::Peer(snapshot));
                }
                Operation::Api(Command::Status) => {
                    return state(gateway::discover(selected.as_deref(), selected_config.as_deref()).await?);
                }
                Operation::Api(Command::SetDeviceName { name }) => {
                    return state(gateway::set_device_name(
                        selected.as_deref(), offline_path.as_deref(), name,
                    ).await?);
                }
                Operation::Api(command) => command,
                Operation::Open => {
                    return state(gateway::open(selected.as_deref(), selected_config).await?);
                }
                Operation::Manage(action) => {
                    return state(
                        gateway::apply(action, selected.as_deref(), offline_path.clone()).await?,
                    );
                }
            };
            enum Edit {
                Appearance(GuiChange),
                Gateway(GatewayChange),
                Network(NativeChange),
            }
            let edit = match &command {
                Command::SetGui { change } => Some(Edit::Appearance(change.clone())),
                Command::SetGateway { change } => Some(Edit::Gateway(*change)),
                Command::SetNative { change } => Some(Edit::Network(change.clone())),
                _ => None,
            };
            match paxri_control::request(&path, command).await {
                Ok(status) => state(Some(status)),
                Err(err)
                    if paxri_control::is_offline(&err)
                        && let Some(edit) = edit =>
                {
                    let file = offline_path
                        .context("Use --config to select this offline gateway's node TOML")?;
                    tokio::task::spawn_blocking(move || {
                        Ok(match edit {
                            Edit::Appearance(change) => RequestResult::OfflineGui(
                                paxri_config::file::set_gui(&file, change, true)?,
                            ),
                            Edit::Gateway(change) => RequestResult::OfflineGateway(
                                paxri_config::file::set_gateway(&file, change, true)?,
                            ),
                            Edit::Network(change) => RequestResult::OfflineNative(
                                paxri_config::file::set_native(&file, change, true)?,
                            ),
                        })
                    })
                    .await?
                }
                Err(err) => Err(err),
            }
        });
        cx.spawn(async move |this, cx| {
            let result = job
                .await
                .map_err(|e| e.to_string())
                .and_then(|r| r.map_err(|e| format!("{e:#}")));
            let _ = this.update(cx, |view, cx| {
                view.busy = false;
                view.saving = false;
                view.gateway_action = None;
                let outcome = result.as_ref().map(|_| ()).map_err(Clone::clone);
                match result {
                    Ok(RequestResult::Peer(peer)) => {
                        if let Some(status) = &mut view.status
                            && status.peers.get(&peer.peer) == Some(&peer.device_id)
                            && let Some(links) = status.peer_connections.as_mut().and_then(|all| all.get_mut(&peer.peer))
                            && let Some(paths) = peer.connections
                        {
                            view.connection_history.record_peer(&peer.device_id, &paths);
                            *links = paths;
                        }
                    }
                    Err(_) if metrics => {}
                    Ok(RequestResult::Stopped(local)) => {
                        view.presence_watch = None;
                        let name = local.as_ref().and_then(|s| s.device_name.as_deref());
                        view.sync_device_name(name, &gethostname::gethostname().to_string_lossy(), cx);
                        view.startup = local.as_ref().map(|s| s.gateway.startup).unwrap_or_default();
                        view.session_keep_alive = local
                            .as_ref()
                            .map(|s| s.gateway.session_keep_alive)
                            .unwrap_or_default();
                        view.profile_native = local.map(|s| s.native);
                        view.status = None;
                        view.offline = Some(GatewayOffline::Stopped);
                    }
                    Ok(RequestResult::OfflineGateway(settings)) => {
                        view.presence_watch = None;
                        view.startup = settings.startup;
                        view.session_keep_alive = settings.session_keep_alive;
                        view.status = None;
                        view.offline = Some(GatewayOffline::Stopped);
                    }
                    Ok(RequestResult::OfflineNative(settings)) => {
                        view.presence_watch = None;
                        view.profile_native = Some(settings);
                        view.status = None;
                        view.offline = Some(GatewayOffline::Stopped);
                    }
                    Ok(RequestResult::OfflineGui(settings)) => {
                        theme::apply(settings, cx);
                    }
                    Ok(RequestResult::Gateway(mut status, legacy)) => {
                        if status.peer_forwards.is_none() && let Some(old) = &mut view.status
                            && old.device_id == status.device_id && old.ipc_path == status.ipc_path && old.peers == status.peers
                        {
                            status.peer_forwards = old.peer_forwards.take();
                        }
                        if view.status.as_ref().is_none_or(|old| {
                            old.device_id != status.device_id || old.ipc_path != status.ipc_path
                        }) {
                            view.connection_history = Default::default();
                            view.terminal_sessions.clear();
                        } else if let Some(message) = &status.pairing.message
                            && view.status.as_ref().and_then(|s| s.pairing.message.as_ref()) != Some(message)
                            && let Some(window) = cx.active_window()
                        {
                            components::Toast::show(message.clone(), window, cx);
                        }
                        let paths = status.peer_connections.iter().flatten().filter_map(|(alias, paths)| {
                            Some((status.peers.get(alias)?.as_str(), paths.as_slice()))
                        });
                        view.connection_history.record(paths);
                        view.sync_peer_connections(&status, cx);
                        view.watch_presence(&status, cx);
                        if let Some(peer) = saved_peer && view.forward_filters.scope.device.is_some() {
                            view.forward_filters.scope.device = Some(peer);
                        }
                        if let Some(old) = &view.status {
                            view.terminal_sessions.retain(|name, _| status.peers.get(name.as_ref()).is_some_and(|id| Some(id) == old.peers.get(name.as_ref())));
                        }
                        view.reconcile_device_selection(&status);
                        view.profile_native = legacy;
                        if let Some(path) = &status.config_path
                            && view.config_path.as_ref() != Some(path)
                        {
                            view.config_path = Some(path.clone());
                            #[cfg(windows)]
                            {
                                view.startup_report.clear();
                                view.inspect_startup(cx);
                            }
                        }
                        theme::apply(status.gui.clone(), cx);
                        let fallback = if status.computer_name.is_empty() { &status.device_id } else { &status.computer_name };
                        view.sync_device_name(status.device_name.as_deref(), fallback, cx);
                        if joined {
                            view.pairing_input =
                                Input::new("", "Paste the other device’s code", cx);
                        }
                        let code = status.pairing.code.as_deref().unwrap_or_default();
                        if view
                            .local_code
                            .update(cx, |field, cx| field.set_text(code, cx))
                        {
                            view.pairing_qr = status.pairing.code.as_ref()
                                .and_then(|code| qrcode::QrCode::new(code).ok())
                                .map(std::rc::Rc::new);
                        }
                        let peer_newly_online = status.peers.keys().any(|peer| {
                            status.peer_online(peer)
                                && !view.status.as_ref().is_some_and(|old| old.peer_online(peer))
                        });
                        let renamed_peers: Vec<SharedString> = status
                            .peers
                            .keys()
                            .filter(|peer| {
                                view.status.as_ref().is_some_and(|old| {
                                    old.peer_display_name(peer) != status.peer_display_name(peer)
                                })
                            })
                            .cloned()
                            .map(SharedString::from)
                            .collect();
                        view.startup = status.startup;
                        view.session_keep_alive = status.session_keep_alive;
                        view.status = Some(*status);
                        if !renamed_peers.is_empty() {
                            view.refresh_renamed_peers(&renamed_peers, cx);
                        }
                        if peer_newly_online {
                            view.refresh_hosted_sessions(true, cx);
                        }
                        if show_forwards && changing {
                            view.refresh_forward_directory(cx);
                        }
                        view.offline = None;
                    }
                    Err(error) if changing && !opening => {
                        if on_complete.is_none() && !name_saved && !relay_saved {
                            components::notify_active(error, cx);
                        }
                    }
                    Err(error) => {
                        view.presence_watch = None;
                        view.status = None;
                        view.offline = Some(GatewayOffline::Failed(error));
                    }
                }
                if let Some(native) = view.status.as_ref()
                    .and_then(|s| s.native.as_ref())
                    .or(view.profile_native.as_ref())
                {
                    view.relay_field
                        .sync(native.relay_url.as_ref().map_or("", |url| url.as_str()), cx);
                }
                if name_saved {
                    view.name_field.finish(&outcome, cx);
                } else if relay_saved {
                    view.relay_field.finish(&outcome, cx);
                }
                #[cfg(windows)]
                if startup_changed {
                    view.inspect_startup(cx);
                }
                if let Some(callback) = on_complete {
                    callback(outcome, cx);
                }
                if let Some(next) = view.pending.pop_front() {
                    view.request(next, cx);
                } else if std::mem::take(&mut view.refresh_presence) {
                    view.send(Command::Status, cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn new(
        address: Option<PathBuf>,
        selected_config: Option<PathBuf>,
        config_path: Option<PathBuf>,
        runtime: tokio::runtime::Handle,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut view = Self {
            presence_watch: None,
            refresh_presence: false,
            listing_forwards: false,
            reconnecting_peers: std::collections::BTreeSet::new(),
            connection_history: Default::default(),
            terminal_sessions: BTreeMap::new(),
            session_titles: paxri_ui::terminal::title::SessionTitles::open(
                paxri_config::file::user_config().or_else(|| config_path.clone())
                    .map(|path| path.with_file_name("session-titles.json")),
            ),
            home_scroll: ScrollHandle::new(),
            devices_pane: None,
            forward_filters: pages::ForwardFilters::for_forwards(cx),
            settings_editor: pages::SettingsEditor::new(cx),
            name_field: TextSetting::new("", Self::commit_device_name, cx),
            relay_field: TextSetting::new("Automatic public relay", Self::commit_relay_url, cx),
            profile_native: None,
            gateway_details: false,
            local_code: Input::read_only("", cx),
            pairing_qr: None,
            pairing_input: Input::new("", "Paste the other device’s code", cx),
            device_filters: DeviceFilters::new("Search", cx),
            session_filters: SessionFilters::new("Search sessions…", cx),
            saving: false,
            gateway_action: None,
            address,
            selected_config,
            #[cfg(windows)]
            startup_report: Vec::new(),
            #[cfg(windows)]
            startup_inspection: None,
            config_path,
            runtime,
            local_name: gethostname::gethostname()
                .to_string_lossy()
                .into_owned()
                .into(),
            status: None,
            startup: Startup::default(),
            session_keep_alive: SessionKeepAlive::default(),
            offline: None,
            busy: false,
            pending: VecDeque::new(),
        };
        view.request(
            GatewayRequest {
                command: Operation::Open,
                on_complete: None,
            },
            cx,
        );
        cx.spawn(async move |this, cx| {
            // Every third tick already supplies connection metrics for all peers.
            for tick in (0..9).cycle() {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                if this
                    .update(cx, |view, cx| {
                        if tick % 3 == 0 {
                            if Workspace::shows_devices(&cx.entity(), cx) {
                                view.refresh_hosted_sessions(tick == 0, cx);
                                if tick == 0 || view.devices_pane == Some(pages::DevicesPane::Forwards) {
                                    view.refresh_forward_directory(cx);
                                }
                            }
                            if view.busy {
                                view.refresh_presence = true;
                            } else {
                                view.send(Command::Status, cx);
                            }
                        } else if !view.busy
                            && let Some(pages::DevicesPane::Peer(peer)) = &view.devices_pane
                            && view.peer_online(peer)
                            && Workspace::shows_devices(&cx.entity(), cx)
                        {
                            view.send(Command::PeerStatus { peer: peer.clone(), check: false }, cx);
                        }
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
        view
    }
}

fn main() -> anyhow::Result<()> {
    let options = Options::parse();
    if let Some(command) = options.command {
        if options.config.is_some() {
            Options::command()
                .error(
                    clap::error::ErrorKind::ArgumentConflict,
                    "--config selects offline desktop settings and cannot be used with a subcommand",
                )
                .exit();
        }
        return command.run(options.gateway);
    }

    #[cfg(windows)]
    release_launch_console();
    let ipc = options.gateway.ipc;
    let config = theme::offline_path(
        options.config.clone(),
        ipc.is_some() || paxri_control::ipc_from_env().is_some(),
    );
    let runtime = tokio::runtime::Runtime::new().expect("create IPC runtime");
    let handle = runtime.handle().clone();
    #[cfg(not(windows))]
    let app = gpui_platform::application();
    #[cfg(windows)]
    let app = Application::with_platform(std::rc::Rc::new(
        gpui_windows::WindowsPlatform::new(false)
            .unwrap_or_else(|error| panic!("failed to initialize Windows platform: {error:#}")),
    ));
    app.with_quit_mode(QuitMode::LastWindowClosed).run(move |cx: &mut App| {
        theme::init(config.as_deref(), cx);
        let dashboard = cx.new(|cx| Dashboard::new(ipc, options.config, config, handle, cx));
        Workspace::open_home(dashboard, cx).expect("open gateway window");
        cx.activate(true);
    });
    Ok(())
}

#[cfg(windows)]
fn release_launch_console() {
    use windows_sys::Win32::System::Console::{FreeConsole, GetConsoleProcessList};

    // Windows versions before 11 24H2 ignore the manifest's console allocation policy.
    let mut pid = 0;
    unsafe {
        if GetConsoleProcessList(&mut pid, 1) == 1 && pid == std::process::id() {
            FreeConsole();
        }
    }
}
