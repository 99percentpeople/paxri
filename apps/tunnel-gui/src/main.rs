#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]
//! Desktop controller. The resident gateway owns configuration and listeners.
use clap::Parser;
use gpui::{prelude::*, *};
use std::{collections::VecDeque, path::PathBuf, time::Duration};
use tunnel_control::{Command, Destination, Direction, Forward, PortAccess, SshProfile, Status};
mod chrome;
mod components;
mod pages;
mod peer_settings;
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

struct RuleForm {
    edit: bool,
    name: Entity<Input>,
    listen: Entity<Input>,
    peer: String,
    service: Entity<Input>,
    target: Entity<Input>,
    mode: RuleMode,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum RuleMode {
    Local,
    Remote,
    Service,
}

struct Dashboard {
    rule: Option<RuleForm>,
    ssh_form: Option<peer_settings::SshForm>,
    ports_form: Option<peer_settings::PortsForm>,
    name_field: Option<Entity<Input>>,
    removing: Option<String>,
    saving: bool,
    address: Option<PathBuf>,
    runtime: tokio::runtime::Handle,
    status: Option<Status>,
    error: Option<String>,
    busy: bool,
    pending: VecDeque<Command>,
    action_error: Option<String>,
}

impl Dashboard {
    fn open_rule(&mut self, name: Option<&str>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(status) = &self.status else { return };
        let f = name.and_then(|n| status.forwards.get(n));
        let form = RuleForm {
            edit: f.is_some(),
            name: Input::new(name.unwrap_or_default(), "Rule name", cx),
            listen: Input::new(
                f.map_or("127.0.0.1:8080", |f| &f.listen),
                "127.0.0.1:8080",
                cx,
            ),
            peer: f
                .map(|f| f.peer.clone())
                .or_else(|| status.peers.keys().next().cloned())
                .unwrap_or_default(),
            service: Input::new(f.map_or("", |f| &f.service), "ssh", cx),
            target: Input::new(
                f.and_then(|f| f.target.as_deref()).unwrap_or(""),
                "127.0.0.1:3000",
                cx,
            ),
            mode: match f {
                Some(f) if f.direction == Direction::Remote => RuleMode::Remote,
                Some(f) if f.target.is_none() => RuleMode::Service,
                _ => RuleMode::Local,
            },
        };
        if form.edit { &form.listen } else { &form.name }
            .read(cx)
            .focus
            .focus(window);
        self.rule = Some(form);
        self.removing = None;
        self.action_error = None;
        cx.notify();
    }

    fn save_rule(&mut self, cx: &mut Context<Self>) {
        if self.saving {
            return;
        }
        let Some(f) = &self.rule else { return };
        let name = f.name.read(cx).text().to_owned();
        let name = if f.edit { name } else { name.trim().to_owned() };
        let Ok(listen) = tunnel_control::listen_address(f.listen.read(cx).text().trim()) else {
            self.action_error = Some(
                "Enter a listen address with a port, such as 127.0.0.1:8080 or [::1]:8080.".into(),
            );
            cx.notify();
            return;
        };
        if name.is_empty() || f.peer.is_empty() {
            self.action_error = Some("Enter a rule name and choose a paired device.".into());
            cx.notify();
            return;
        }
        let destination = if f.mode == RuleMode::Service {
            Destination::Service(f.service.read(cx).text().trim().to_owned())
        } else {
            match tunnel_control::tcp_target(f.target.read(cx).text().trim()) {
                Ok(target) => Destination::Target(target),
                Err(e) => {
                    self.action_error = Some(e);
                    cx.notify();
                    return;
                }
            }
        };
        self.send(
            Command::SaveForward {
                name,
                edit: f.edit,
                rule: Forward {
                    listen,
                    peer: f.peer.clone(),
                    destination,
                    direction: if f.mode == RuleMode::Remote {
                        Direction::Remote
                    } else {
                        Direction::Local
                    },
                },
            },
            cx,
        );
    }
    fn send(&mut self, command: Command, cx: &mut Context<Self>) {
        let changing = !matches!(command, Command::Status);
        let rule_saved = matches!(command, Command::SaveForward { .. });
        let name_saved = matches!(command, Command::SetDeviceName { .. });
        let ssh_saved = matches!(command, Command::SetSshProfile { .. });
        let ports_saved = matches!(command, Command::SetPortAccess { .. });
        let removed = matches!(command, Command::RemoveForward { .. });
        if self.busy {
            if changing {
                self.saving = true;
                self.pending.push_back(command);
                cx.notify();
            }
            return;
        }
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
                match result {
                    Ok(status) => {
                        if rule_saved {
                            view.rule = None;
                        }
                        if name_saved {
                            view.name_field = None;
                        }
                        if ssh_saved {
                            view.ssh_form = None;
                        }
                        if ports_saved {
                            view.ports_form = None;
                        }
                        if removed {
                            view.removing = None;
                        }
                        view.status = Some(status);
                        view.error = None;
                        if changing {
                            view.action_error = None;
                        }
                    }
                    Err(error) if changing => {
                        view.action_error = Some(error);
                    }
                    Err(error) => {
                        view.status = None;
                        view.error = Some(error);
                    }
                }
                if let Some(next) = view.pending.pop_front() {
                    view.send(next, cx);
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
            rule: None,
            ssh_form: None,
            ports_form: None,
            name_field: None,
            removing: None,
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
        let dashboard = cx.new(|cx| Dashboard::new(address, handle, cx));
        Workspace::open_home(dashboard, cx).expect("open gateway window");
        cx.activate(true);
    });
}
