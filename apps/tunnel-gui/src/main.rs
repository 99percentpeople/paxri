#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]
//! Desktop controller. The resident gateway owns configuration and listeners.
use clap::Parser;
use gpui::{prelude::*, *};
use std::{collections::VecDeque, path::PathBuf, time::Duration};
use tunnel_control::{Command, Status};
mod chrome;
mod components;
mod pages;
mod terminal;
mod theme;
mod workspace;
use components::Input;
use workspace::Workspace;

#[derive(Parser)]
#[command(about = "P2P Tunnel desktop controller")]
struct Options {
    /// Same Unix socket path or Windows pipe name as tunnel-gateway
    #[arg(long)]
    ipc: Option<PathBuf>,
}

type OnComplete = Box<dyn FnOnce(Result<(), String>, &mut App)>;

struct GatewayRequest {
    command: Command,
    on_complete: Option<OnComplete>,
}

struct Dashboard {
    home_scroll: ScrollHandle,
    settings_scroll: ScrollHandle,
    terminal_settings: Entity<pages::TerminalSettings>,
    name_field: Option<Entity<Input>>,
    pairing_input: Entity<Input>,
    local_code: Entity<Input>,
    copied_feedback: Option<Task<()>>,
    saving: bool,
    address: Option<PathBuf>,
    runtime: tokio::runtime::Handle,
    status: Option<Status>,
    error: Option<String>,
    busy: bool,
    pending: VecDeque<GatewayRequest>,
    action_error: Option<String>,
}

impl Dashboard {
    fn send(&mut self, command: Command, cx: &mut Context<Self>) {
        self.request(
            GatewayRequest {
                command,
                on_complete: None,
            },
            cx,
        );
    }

    fn request(&mut self, request: GatewayRequest, cx: &mut Context<Self>) {
        let changing = !matches!(request.command, Command::Status);
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
        let name_saved = matches!(command, Command::SetDeviceName { .. });
        let joined = matches!(command, Command::JoinPairing { .. });
        self.busy = true;
        self.saving = changing || !self.pending.is_empty();
        cx.notify();
        let path = self
            .address
            .clone()
            .unwrap_or_else(tunnel_control::default_address);
        let job = self
            .runtime
            .spawn(async move { tunnel_control::request(&path, command).await });
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
                    Ok(status) => {
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
                        view.status = Some(status);
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
        runtime: tokio::runtime::Handle,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut view = Self {
            home_scroll: ScrollHandle::new(),
            settings_scroll: ScrollHandle::new(),
            terminal_settings: cx.new(|_| pages::TerminalSettings::default()),
            name_field: None,
            local_code: Input::read_only("", cx),
            copied_feedback: None,
            pairing_input: Input::new("", "Paste the other device’s code", cx),
            saving: false,
            address,
            runtime,
            status: None,
            error: None,
            busy: false,
            pending: VecDeque::new(),
            action_error: None,
        };
        view.send(Command::Status, cx);
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

fn main() {
    let address = Options::parse().ipc;
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
        theme::Appearance::init(cx);
        theme::TerminalAppearance::init(cx);
        let dashboard = cx.new(|cx| Dashboard::new(address, handle, cx));
        Workspace::open_home(dashboard, cx).expect("open gateway window");
        cx.activate(true);
    });
}
