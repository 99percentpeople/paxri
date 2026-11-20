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
use peerhatch_config::startup::Startup;
use peerhatch_control::{Command, Status};
mod chrome;
mod components;
mod pages;
mod peer_check;
mod terminal;
mod theme;
mod workspace;
use components::Input;
use workspace::Workspace;

#[derive(Parser)]
#[command(version, about = "Open the PeerHatch desktop app, or run a command in this terminal")]
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
    Gateway(Box<Status>),
    OfflineGui(theme::GuiSettings),
    Stopped(Startup),
}

struct Dashboard {
    peer_checks: BTreeMap<String, peer_check::PeerCheck>,
    terminal_sessions: BTreeMap<SharedString, workspace::PeerSessions>,
    home_scroll: ScrollHandle,
    settings_scroll: ScrollHandle,
    name_field: Option<Entity<Input>>,
    pairing_input: Entity<Input>,
    peer_search: Entity<Input>,
    peer_system: Option<peerhatch_control::DevicePlatform>,
    local_code: Entity<Input>,
    copied_feedback: Option<Task<()>>,
    saving: bool,
    address: Option<PathBuf>,
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
    fn send(&mut self, command: Command, cx: &mut Context<Self>) {
        self.request(
            GatewayRequest {
                command: Operation::Api(command),
                on_complete: None,
            },
            cx,
        );
    }

    fn manage(&mut self, action: GatewayAction, cx: &mut Context<Self>) {
        if !self.saving {
            self.request(
                GatewayRequest {
                    command: Operation::Manage(action),
                    on_complete: None,
                },
                cx,
            );
        }
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
        let name_saved = matches!(command, Operation::Api(Command::SetDeviceName { .. }));
        let joined = matches!(command, Operation::Api(Command::JoinPairing { .. }));
        self.busy = true;
        self.saving = changing || !self.pending.is_empty();
        cx.notify();
        let path = self
            .address
            .clone()
            .unwrap_or_else(peerhatch_control::default_address);
        let offline_path = self.config_path.clone();
        let job = self.runtime.spawn(async move {
            let state = |status: Option<Status>| -> anyhow::Result<RequestResult> {
                if let Some(s) = status {
                    return Ok(RequestResult::Gateway(Box::new(s)));
                }
                let config = offline_path
                    .clone()
                    .or_else(peerhatch_config::file::user_config);
                let mode = config
                    .as_deref()
                    .map(peerhatch_config::file::startup)
                    .transpose()?
                    .unwrap_or_default();
                Ok(RequestResult::Stopped(mode))
            };
            let command = match command {
                Operation::Api(Command::Status) => return state(gateway::status(&path).await?),
                Operation::Api(command) => command,
                Operation::Open => return state(gateway::open(&path, offline_path.clone()).await?),
                Operation::Manage(action) => {
                    return state(gateway::apply(action, &path, offline_path.clone()).await?);
                }
            };
            let change = match &command {
                Command::SetGui { change } => Some(*change),
                _ => None,
            };
            match (peerhatch_control::request(&path, command).await, change) {
                (Ok(status), _) => Ok(RequestResult::Gateway(Box::new(status))),
                (Err(err), Some(change)) if peerhatch_control::is_offline(&err) => {
                    let file = offline_path
                        .context("Use --config to select this offline gateway's node TOML")?;
                    let gui = tokio::task::spawn_blocking(move || {
                        peerhatch_config::file::set_gui(&file, change, true)
                    })
                    .await??;
                    Ok(RequestResult::OfflineGui(gui))
                }
                (Err(err), _) => Err(err),
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
                let outcome = result.as_ref().map(|_| ()).map_err(Clone::clone);
                match result {
                    Ok(RequestResult::Stopped(startup)) => {
                        view.status = None;
                        view.startup = startup;
                        view.error = Some("Gateway stopped.".into());
                        if changing {
                            view.action_error = None;
                        }
                    }
                    Ok(RequestResult::OfflineGui(settings)) => {
                        theme::apply(settings, cx);
                        view.action_error = None;
                    }
                    Ok(RequestResult::Gateway(status)) => {
                        if view.local_name.as_ref() != status.display_name() {
                            view.local_name = status.display_name().to_owned().into();
                        }
                        if let Some(path) = &status.config_path {
                            view.config_path = Some(path.clone());
                        }
                        theme::apply(status.gui, cx);
                        if name_saved {
                            view.name_field = None;
                        }
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
                        if on_complete.is_none() {
                            view.action_error = Some(error);
                        }
                    }
                    Err(error) => {
                        view.status = None;
                        view.error = Some(error);
                    }
                }
                if let Some(callback) = on_complete {
                    callback(outcome, cx);
                }
                if let Some(next) = view.pending.pop_front() {
                    view.request(next, cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn new(
        address: Option<PathBuf>,
        config_path: Option<PathBuf>,
        runtime: tokio::runtime::Handle,
        cx: &mut Context<Self>,
    ) -> Self {
        let peer_search = Input::new("", "Search devices", cx);
        cx.observe(&peer_search, |_, _, cx| cx.notify()).detach();
        let mut view = Self {
            peer_checks: BTreeMap::new(),
            terminal_sessions: BTreeMap::new(),
            home_scroll: ScrollHandle::new(),
            settings_scroll: ScrollHandle::new(),
            name_field: None,
            local_code: Input::read_only("", cx),
            copied_feedback: None,
            pairing_input: Input::new("", "Paste the other device’s code", cx),
            peer_search,
            peer_system: None,
            saving: false,
            address,
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
        options.config,
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
        let dashboard = cx.new(|cx| Dashboard::new(ipc, config, handle, cx));
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
