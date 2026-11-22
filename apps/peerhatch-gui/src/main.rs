//! Desktop controller. The resident gateway owns configuration and listeners.
use anyhow::Context as _;
use clap::{CommandFactory, Parser};
use gpui::{prelude::*, *};
use std::{
    collections::{BTreeMap, VecDeque},
    path::PathBuf,
    time::Duration,
};
use peerhatch_cli::gateway::{self, Action as GatewayAction};
use peerhatch_config::{Native, NativeChange, startup::Startup};
use peerhatch_control::{Command, GuiChange, Status};
mod chrome;
mod components;
#[cfg(windows)]
mod ime;
mod connection_metrics;
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
    about = "Open the PeerHatch desktop app, or run a command in this terminal"
)]
struct Options {
    #[command(flatten)]
    gateway: peerhatch_cli::GatewayOptions,
    /// Node TOML for the background gateway and offline desktop settings
    #[arg(long)]
    config: Option<PathBuf>,
    #[command(subcommand)]
    command: Option<peerhatch_cli::Command>,
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
    OfflineGui(theme::GuiSettings),
    OfflineNative(Native),
    Stopped(Startup, Option<Native>),
}

struct Dashboard {
    presence_watch: Option<(PathBuf, Task<()>)>,
    refresh_presence: bool,
    connection_history: connection_metrics::ConnectionHistory,
    terminal_sessions: BTreeMap<SharedString, workspace::PeerSessions>,
    home_scroll: ScrollHandle,
    devices_pane: Option<pages::DevicesPane>,
    forward_filters: pages::ForwardFilters,
    settings_editor: pages::SettingsEditor,
    name_field: TextSetting,
    relay_field: TextSetting,
    profile_native: Option<Native>,
    gateway_details: bool,
    pairing_input: Entity<Input>,
    review_request: Option<String>,
    peer_search: Entity<Input>,
    peer_filter_open: bool,
    peer_system: Option<peerhatch_control::DevicePlatform>,
    local_code: Entity<Input>,
    copied_feedback: Option<Task<()>>,
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
    error: Option<String>,
    busy: bool,
    pending: VecDeque<GatewayRequest>,
    action_error: Option<String>,
}

impl Dashboard {
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
        let changing = !matches!(request.command, Operation::Api(Command::Status));
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
        self.busy = true;
        self.saving = changing || !self.pending.is_empty();
        if let Operation::Manage(action) = command {
            self.gateway_action = Some(action);
        }
        cx.notify();
        let selected = self.address.clone();
        let selected_config = self.selected_config.clone();
        let path = self
            .active_address()
            .unwrap_or_else(peerhatch_control::default_address);
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
                            .and_then(|p| peerhatch_config::file::read_local(p).ok())
                            .map(|settings| settings.native)
                    } else {
                        None
                    };
                    return Ok(RequestResult::Gateway(Box::new(s), legacy));
                }
                let local = offline_path
                    .as_deref()
                    .map(peerhatch_config::file::read_local)
                    .transpose()?;
                let startup = local
                    .as_ref()
                    .map(|s| s.gateway.startup)
                    .unwrap_or_default();
                Ok(RequestResult::Stopped(startup, local.map(|s| s.native)))
            };
            let command = match command {
                Operation::Api(Command::Status) => {
                    let mut status = gateway::discover(selected.as_deref(), selected_config.as_deref()).await?;
                    if show_forwards && let Some(current) = &mut status {
                        let ipc = current.ipc_path.as_deref().unwrap_or(&path);
                        match peerhatch_control::request(ipc, Command::ListForwards).await {
                            Ok(list) => *current = list,
                            Err(error) => {
                                let mut directory = peerhatch_control::ForwardDirectory::default();
                                directory.errors.insert("Gateway".into(), format!("{error:#}"));
                                current.peer_forwards = Some(directory);
                            }
                        }
                    }
                    return state(status);
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
                Network(NativeChange),
            }
            let edit = match &command {
                Command::SetGui { change } => Some(Edit::Appearance(change.clone())),
                Command::SetNative { change } => Some(Edit::Network(change.clone())),
                _ => None,
            };
            match peerhatch_control::request(&path, command).await {
                Ok(status) => state(Some(status)),
                Err(err)
                    if peerhatch_control::is_offline(&err)
                        && let Some(edit) = edit =>
                {
                    let file = offline_path
                        .context("Use --config to select this offline gateway's node TOML")?;
                    tokio::task::spawn_blocking(move || {
                        Ok(match edit {
                            Edit::Appearance(change) => RequestResult::OfflineGui(
                                peerhatch_config::file::set_gui(&file, change, true)?,
                            ),
                            Edit::Network(change) => RequestResult::OfflineNative(
                                peerhatch_config::file::set_native(&file, change, true)?,
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
                    Ok(RequestResult::Stopped(startup, native)) => {
                        view.presence_watch = None;
                        view.profile_native = native;
                        view.status = None;
                        view.startup = startup;
                        view.error = Some("Gateway stopped.".into());
                        if changing {
                            view.action_error = None;
                        }
                    }
                    Ok(RequestResult::OfflineNative(settings)) => {
                        view.presence_watch = None;
                        view.profile_native = Some(settings);
                        view.status = None;
                        view.action_error = None;
                    }
                    Ok(RequestResult::OfflineGui(settings)) => {
                        theme::apply(settings, cx);
                        view.action_error = None;
                    }
                    Ok(RequestResult::Gateway(mut status, legacy)) => {
                        if show_forwards && changing {
                            view.refresh_presence = true;
                            if let Some(old) = &mut view.status
                                && old.device_id == status.device_id && old.peers == status.peers
                            {
                                status.peer_forwards = old.peer_forwards.take();
                            }
                        }
                        if view.status.as_ref().is_none_or(|old| {
                            old.device_id != status.device_id || old.ipc_path != status.ipc_path
                        }) {
                            view.connection_history = Default::default();
                        }
                        view.connection_history.record(&status);
                        view.watch_presence(&status, cx);
                        if let Some(peer) = saved_peer && view.forward_filters.device.is_some() {
                            view.forward_filters.device = Some(peer);
                        }
                        view.reconcile_device_selection(&status);
                        view.profile_native = legacy;
                        if view.local_name.as_ref() != status.display_name() {
                            view.local_name = status.display_name().to_owned().into();
                        }
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
                        view.name_field.set_placeholder(&status.computer_name, cx);
                        view.name_field
                            .sync(status.device_name.as_deref().unwrap_or_default(), cx);
                        if joined {
                            view.pairing_input =
                                Input::new("", "Paste the other device’s code", cx);
                        }
                        let code = status.pairing.code.as_deref().unwrap_or_default();
                        if view
                            .local_code
                            .update(cx, |field, cx| field.set_text(code, cx))
                        {
                            view.copied_feedback = None;
                        }
                        view.startup = status.startup;
                        view.status = Some(*status);
                        view.error = None;
                        if changing && on_complete.is_none() {
                            view.action_error = None;
                        }
                    }
                    Err(error) if changing => {
                        if on_complete.is_none() && !name_saved && !relay_saved {
                            view.action_error = Some(error);
                        }
                    }
                    Err(error) => {
                        view.presence_watch = None;
                        view.status = None;
                        view.error = Some(error);
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
        let peer_search = Input::search("Search", cx);
        cx.observe(&peer_search, |_, _, cx| cx.notify()).detach();
        let mut view = Self {
            presence_watch: None,
            refresh_presence: false,
            connection_history: Default::default(),
            terminal_sessions: BTreeMap::new(),
            home_scroll: ScrollHandle::new(),
            devices_pane: None,
            forward_filters: pages::ForwardFilters::new(cx),
            settings_editor: pages::SettingsEditor::new(cx),
            name_field: TextSetting::new("", Self::commit_device_name, cx),
            relay_field: TextSetting::new("Automatic public relay", Self::commit_relay_url, cx),
            profile_native: None,
            gateway_details: false,
            local_code: Input::read_only("", cx),
            copied_feedback: None,
            review_request: None,
            pairing_input: Input::new("", "Paste the other device’s code", cx),
            peer_search,
            peer_filter_open: false,
            peer_system: None,
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
            error: None,
            busy: false,
            pending: VecDeque::new(),
            action_error: None,
        };
        view.request(
            GatewayRequest {
                command: Operation::Open,
                on_complete: None,
            },
            cx,
        );
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(3)).await;
                if this
                    .update(cx, |view, cx| view.send(Command::Status, cx))
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
        ipc.is_some() || std::env::var_os("PEERHATCH_IPC").is_some(),
    );
    let runtime = tokio::runtime::Runtime::new().expect("create IPC runtime");
    let handle = runtime.handle().clone();
    Application::new().run(move |cx: &mut App| {
        // GPUI stops its Linux and Windows loops when the last window is removed.
        #[cfg(target_os = "macos")]
        cx.on_window_closed(|cx| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();
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
