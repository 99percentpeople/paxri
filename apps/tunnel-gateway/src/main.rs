use std::{collections::BTreeMap, net::SocketAddr, path::PathBuf, sync::Arc};

use anyhow::{Context, Result};
use clap::Parser;
use tokio::{
    net::TcpListener,
    sync::{Mutex, OwnedSemaphorePermit, Semaphore},
    task::{JoinHandle, JoinSet},
};
use tunnel_config::{Config, Destination, Direction, Forward};
mod pairing;
mod platforms;
mod settings;
use settings::SettingsFile;
use tunnel_control::{Command, ControlListener, ControlStream, ForwardStatus, Status};
use tunnel_native::gateway::{BindingState, NativeTransport, RemoteForward};

#[derive(Parser, Clone)]
#[command(about = "Run the resident P2P tunnel gateway")]
struct Options {
    /// Node configuration TOML
    #[arg(long)]
    config: PathBuf,
    /// Existing private key created with tunnel identity
    #[arg(long)]
    key: PathBuf,
    /// Unix socket path or Windows local pipe name
    #[arg(long)]
    ipc: Option<PathBuf>,
    /// Disable relay and public address discovery
    #[arg(long)]
    no_relay: bool,
    #[cfg(windows)]
    /// Run under the Windows Service Control Manager
    #[arg(long)]
    windows_service: bool,
}

struct LocalListener {
    address: SocketAddr,
    task: JoinHandle<()>,
    route: tokio::sync::watch::Sender<(String, Destination)>,
}

impl Drop for LocalListener {
    fn drop(&mut self) {
        self.task.abort();
    }
}

enum Listener {
    Local(LocalListener),
    Remote(RemoteForward),
}

impl Listener {
    fn state(&self) -> BindingState {
        match self {
            Self::Local(l) => BindingState {
                address: Some(l.address),
                error: None,
            },
            Self::Remote(r) => r.state(),
        }
    }
    async fn stop(self) {
        match self {
            Self::Local(mut l) => {
                l.task.abort();
                let _ = (&mut l.task).await;
            }
            Self::Remote(r) => r.stop().await,
        }
    }
    fn update_destination(&self, rule: &Forward) {
        match self {
            Self::Local(l) => {
                l.route
                    .send_replace((rule.peer.clone(), rule.destination.clone()));
            }
            Self::Remote(r) => r.set_target(rule.destination.address().into()),
        }
    }
}

struct Gateway {
    pairing: pairing::Pairings,
    config: Arc<Config>,
    file: SettingsFile,
    direct_only: bool,
    transport: Arc<NativeTransport>,
    listeners: BTreeMap<String, Listener>,
    streams: Arc<Semaphore>,
}

impl Gateway {
    fn apply_config(&mut self, config: Config) {
        self.config = Arc::new(config);
        self.transport.update_config(self.config.clone());
    }

    fn check_forward_peer(&self, name: &str, expected: Option<&str>) -> Result<()> {
        if let Some(peer) = expected {
            anyhow::ensure!(
                self.config
                    .forwards
                    .get(name)
                    .is_some_and(|f| f.peer == peer),
                "Forward {name} no longer belongs to {peer}"
            );
        }
        Ok(())
    }

    async fn set_forward(
        &mut self,
        name: &str,
        enabled: bool,
        expected_peer: Option<&str>,
    ) -> Result<()> {
        self.check_forward_peer(name, expected_peer)?;
        let rule = self
            .config
            .forwards
            .get(name)
            .with_context(|| format!("unknown forward {name}"))?;
        if enabled {
            if !self.listeners.contains_key(name) {
                let listener = self.prepare_listener(name, rule).await?;
                self.listeners.insert(name.to_owned(), listener);
            }
        } else if let Some(listener) = self.listeners.remove(name) {
            listener.stop().await;
        }
        Ok(())
    }

    async fn bind(name: &str, rule: &Forward) -> Result<TcpListener> {
        let socket = TcpListener::bind(rule.listen)
            .await
            .with_context(|| format!("bind forward {name} at {}", rule.listen))?;
        Ok(socket)
    }

    fn remote_listener(&self, rule: &Forward) -> Listener {
        Listener::Remote(RemoteForward::start(
            self.transport.clone(),
            rule.peer.clone(),
            rule.listen,
            rule.destination.address().into(),
            self.streams.clone(),
        ))
    }

    async fn prepare_listener(&self, name: &str, rule: &Forward) -> Result<Listener> {
        match rule.direction {
            Direction::Local => self.start_listener(Self::bind(name, rule).await?, rule),
            Direction::Remote => Ok(self.remote_listener(rule)),
        }
    }

    fn start_listener(&self, socket: TcpListener, rule: &Forward) -> Result<Listener> {
        let address = socket.local_addr()?;
        let transport = self.transport.clone();
        let slots = self.streams.clone();
        let (route, destination) =
            tokio::sync::watch::channel((rule.peer.clone(), rule.destination.clone()));
        let task = tokio::spawn(async move {
            loop {
                let tcp = match socket.accept().await {
                    Ok((tcp, _)) => tcp,
                    Err(e) => {
                        eprintln!("accept on {address}: {e}");
                        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
                        continue;
                    }
                };
                let Ok(permit) = slots.clone().try_acquire_owned() else {
                    continue;
                };
                let transport = transport.clone();
                let (peer, target) = destination.borrow().clone();
                // An established TCP flow outlives pause/resume of its listener.
                tokio::spawn(async move {
                    let _permit = permit;
                    if let Err(e) = tunnel_core::forward(&*transport, &peer, &target, tcp).await {
                        eprintln!("forward to {peer}/{}: {e:#}", target.address());
                    }
                });
            }
        });
        Ok(Listener::Local(LocalListener {
            address,
            task,
            route,
        }))
    }

    async fn change_settings(&mut self, change: Command) -> Result<()> {
        if let Command::SaveForward {
            name,
            expected_peer,
            ..
        }
        | Command::RemoveForward {
            name,
            expected_peer,
        } = &change
        {
            self.check_forward_peer(name, expected_peer.as_deref())?;
        }
        let (text, config) = self.file.prepare(&change)?;
        // Reserve any new port before saving. The active rule survives a bind or disk error.
        let mut replace = false;
        let mut socket = None;
        if let Command::SaveForward { name, edit, .. } = &change {
            let rule = &config.forwards[name];
            anyhow::ensure!(
                !self.direct_only || config.peers[&rule.peer].ticket.is_some(),
                "direct-only forwarding to {} requires a peer ticket",
                rule.peer
            );
            replace = !edit
                || self.listeners.get(name).is_some_and(|old| {
                    let prior = &self.config.forwards[name];
                    prior.direction != rule.direction
                        || match rule.direction {
                            Direction::Remote => {
                                prior.peer != rule.peer || prior.listen != rule.listen
                            }
                            Direction::Local => {
                                prior.listen != rule.listen
                                    && old.state().address != Some(rule.listen)
                            }
                        }
                });
            if replace && rule.direction == Direction::Local {
                socket = Some(Self::bind(name, rule).await?);
            }
        }
        self.file.persist(text)?;
        match change {
            Command::SaveForward { name, .. } => {
                let rule = &config.forwards[&name];
                if replace {
                    if let Some(old) = self.listeners.remove(&name) {
                        old.stop().await;
                    }
                    let listener = match socket {
                        Some(socket) => self.start_listener(socket, rule)?,
                        None => self.remote_listener(rule),
                    };
                    self.listeners.insert(name, listener);
                } else if let Some(listener) = self.listeners.get(&name) {
                    listener.update_destination(rule);
                }
            }
            Command::RemoveForward { name, .. } => {
                if let Some(old) = self.listeners.remove(&name) {
                    old.stop().await;
                }
            }
            _ => {}
        }
        self.apply_config(config);
        Ok(())
    }

    fn status(&mut self) -> Status {
        let ticket = self.transport.current_ticket().to_string();
        let config = &self.config;
        Status {
            config_path: Some(self.file.path.clone()),
            gui: tunnel_config::file::read_gui(&self.file.path).unwrap_or(config.gui),
            pairing: self.pairing.status(config.pairing_code.as_deref()),
            device_id: self.transport.id(),
            computer_name: gethostname::gethostname().to_string_lossy().into_owned(),
            device_name: config.device_name.clone(),
            terminal_access: config
                .peers
                .iter()
                .filter(|(_, p)| p.allow_terminal)
                .map(|(name, _)| name.clone())
                .collect(),
            port_access: config
                .peers
                .iter()
                .filter(|(_, p)| !p.ports.connect.is_empty() || !p.ports.listen.is_empty())
                .map(|(name, p)| (name.clone(), p.ports.clone()))
                .collect(),
            ticket,
            peer_platforms: config
                .peers
                .iter()
                .filter_map(|(name, peer)| Some((name.clone(), peer.platform?)))
                .collect(),
            peers: config
                .peers
                .iter()
                .map(|(name, p)| (name.clone(), p.endpoint_id.clone()))
                .collect(),
            services: config
                .services
                .iter()
                .map(|(name, s)| (name.clone(), s.target.clone()))
                .collect(),
            forwards: config
                .forwards
                .iter()
                .map(|(name, rule)| {
                    let state = self
                        .listeners
                        .get(name)
                        .map(Listener::state)
                        .unwrap_or_default();
                    let status = ForwardStatus {
                        listen: rule.listen.to_string(),
                        bound: state.address.map(|a| a.to_string()),
                        peer: rule.peer.clone(),
                        direction: rule.direction,
                        service: match &rule.destination {
                            Destination::Service(s) => s.clone(),
                            _ => String::new(),
                        },
                        target: match &rule.destination {
                            Destination::Target(t) => Some(t.clone()),
                            _ => None,
                        },
                        enabled: Some(self.listeners.contains_key(name)),
                        error: state.error,
                    };
                    (name.clone(), status)
                })
                .collect(),
        }
    }
}

async fn handle_client(
    mut socket: ControlStream,
    gateway: Arc<Mutex<Gateway>>,
    management_slot: OwnedSemaphorePermit,
) -> Result<()> {
    let command = match tunnel_control::receive(&mut socket).await {
        Ok(command) => command,
        Err(e) => return tunnel_control::respond::<()>(&mut socket, Err(e)).await,
    };
    match command {
        Command::Status => {
            let status = gateway.lock().await.status();
            tunnel_control::respond(&mut socket, Ok(status)).await
        }
        Command::SetGui { change } => {
            let mut state = gateway.lock().await;
            let result = tunnel_config::file::set_gui(&state.file.path, change, false)
                .map(|_| state.status());
            drop(state);
            tunnel_control::respond(&mut socket, result).await
        }
        Command::RefreshPairingCode
        | Command::CancelPairing
        | Command::JoinPairing { .. }
        | Command::AnswerPairing { .. } => {
            let mut state = gateway.lock().await;
            let result = match command {
                Command::RefreshPairingCode => {
                    let ticket = state.transport.current_ticket();
                    state.refresh_pairing(&ticket)
                }
                Command::CancelPairing => state.pairing.cancel(),
                Command::JoinPairing { code } => state.join_pairing(&code, &gateway),
                Command::AnswerPairing {
                    device_id,
                    request_id,
                    accept,
                } => state.answer_pairing(&device_id, request_id.as_deref(), accept),
                _ => unreachable!(),
            }
            .map(|_| state.status());
            drop(state);
            tunnel_control::respond(&mut socket, result).await
        }
        Command::SetForward {
            name,
            enabled,
            expected_peer,
        } => {
            let result = {
                let mut gateway = gateway.lock().await;
                gateway
                    .set_forward(&name, enabled, expected_peer.as_deref())
                    .await
                    .map(|_| gateway.status())
            };
            tunnel_control::respond(&mut socket, result).await
        }
        Command::SaveForward { .. }
        | Command::RemoveForward { .. }
        | Command::SetDeviceName { .. }
        | Command::SetTerminalAccess { .. }
        | Command::SetPortAccess { .. } => {
            let result = {
                let mut gateway = gateway.lock().await;
                gateway
                    .change_settings(command)
                    .await
                    .map(|_| gateway.status())
            };
            tunnel_control::respond(&mut socket, result).await
        }
        open @ (Command::Connect { .. } | Command::OpenTerminal { .. }) => {
            let (transport, permit) = {
                let gateway = gateway.lock().await;
                (
                    gateway.transport.clone(),
                    gateway.streams.clone().try_acquire_owned(),
                )
            };
            let Ok(_permit) = permit else {
                return tunnel_control::respond::<()>(
                    &mut socket,
                    Err(anyhow::anyhow!("gateway stream limit reached")),
                )
                .await;
            };
            drop(management_slot);
            if let Command::OpenTerminal { peer, size } = open {
                use tokio::io::AsyncReadExt;
                let opened = tokio::select! {
                    result = transport.open_terminal(&peer, size) => result,
                    _ = socket.read_u8() => return Ok(()),
                };
                let (_connection, mut send, mut recv) = match opened {
                    Ok(connection) => connection,
                    Err(e) => return tunnel_control::respond::<()>(&mut socket, Err(e)).await,
                };
                tunnel_control::respond(&mut socket, Ok(())).await?;
                let (mut input, mut output) = tokio::io::split(socket);
                // Closing the tab or the peer's terminal closes the session in both directions.
                tokio::select! {
                    r = tokio::io::copy(&mut input, &mut send) => r?,
                    r = tokio::io::copy(&mut recv, &mut output) => r?,
                };
                return Ok(());
            }
            let Command::Connect { peer, service } = open else {
                unreachable!()
            };
            let stream = match tunnel_core::open_service(&*transport, &peer, &service).await {
                Ok(stream) => stream,
                Err(e) => return tunnel_control::respond::<()>(&mut socket, Err(e)).await,
            };
            tunnel_control::respond(&mut socket, Ok(())).await?;
            let (reader, writer) = tokio::io::split(stream);
            tunnel_control::bridge(socket, reader, writer)
                .await
                .with_context(|| format!("stream to {peer}/{service}"))
        }
    }
}

async fn run(
    options: Options,
    stop: impl Future<Output = Result<()>>,
    ready: impl FnOnce() -> Result<()>,
) -> Result<()> {
    let text = std::fs::read_to_string(&options.config)
        .with_context(|| format!("read {}", options.config.display()))?;
    let config = Arc::new(tunnel_config::parse(&text).map_err(anyhow::Error::msg)?);
    let key = tunnel_native::identity::load(&options.key)?;
    let path = options.ipc.unwrap_or_else(tunnel_control::default_address);
    let control = ControlListener::bind(&path)
        .await
        .with_context(|| format!("bind IPC {}", path.display()))?;
    let (pair_tx, mut pair_requests) = tokio::sync::mpsc::channel(16);
    let (transport, router) =
        tunnel_native::gateway::start(key, config.clone(), options.no_relay, pair_tx).await?;
    let mut gateway = Gateway {
        pairing: Default::default(),
        config: config.clone(),
        file: SettingsFile {
            path: options.config.canonicalize()?,
            text,
        },
        direct_only: options.no_relay,
        transport: Arc::new(transport),
        listeners: BTreeMap::new(),
        streams: Arc::new(Semaphore::new(256)),
    };
    if let Err(e) = gateway.init_pairing().await {
        gateway.pairing.message = Some(format!("Pairing code unavailable: {e:#}"));
    }
    for name in config.forwards.keys() {
        gateway.set_forward(name, true, None).await?;
    }
    ready()?;
    eprintln!(
        "gateway {} ready at {}",
        gateway.transport.id(),
        path.display()
    );
    let gateway = Arc::new(Mutex::new(gateway));
    let management = Arc::new(Semaphore::new(32));
    let mut clients = JoinSet::new();
    clients.spawn(platforms::fill_missing(gateway.clone()));
    tokio::pin!(stop);
    let result = loop {
        tokio::select! {
            result = &mut stop => break result,
            Some(request) = pair_requests.recv() => gateway.lock().await.pairing.receive(request),
            accepted = async {
                let slot = management.clone().acquire_owned().await?;
                Ok::<_, anyhow::Error>((control.accept().await?, slot))
            } => {
                let (socket, management_slot) = accepted?;
                let gateway = gateway.clone();
                clients.spawn(async move {
                    if let Err(e) = handle_client(socket, gateway, management_slot).await {
                        eprintln!("IPC client: {e:#}");
                    }
                });
            },
            _ = clients.join_next(), if !clients.is_empty() => {}
        }
    };
    clients.shutdown().await;
    drop(gateway);
    router.shutdown().await?;
    result
}

async fn signal() -> Result<()> {
    #[cfg(unix)]
    {
        let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
        tokio::select! { r = tokio::signal::ctrl_c() => r?, _ = term.recv() => {} }
    }
    #[cfg(windows)]
    tokio::signal::ctrl_c().await?;
    Ok(())
}

#[cfg(windows)]
mod windows;

fn main() -> Result<()> {
    let options = Options::parse();
    #[cfg(windows)]
    if options.windows_service {
        return windows::dispatch();
    }
    tokio::runtime::Runtime::new()?.block_on(run(options, signal(), || Ok(())))
}
