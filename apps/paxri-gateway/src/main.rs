use std::{collections::BTreeMap, net::SocketAddr, path::PathBuf, sync::Arc};

use anyhow::{Context, Result};
use clap::Parser;
use tokio::{
    net::TcpListener,
    sync::{Mutex, OwnedSemaphorePermit, Semaphore},
    task::{JoinHandle, JoinSet},
};
use paxri_config::{Config, Direction, Forward};
mod account;
mod pairing;
mod forward_management;
mod platforms;
mod settings;
use settings::SettingsFile;
use paxri_control::{Command, ControlListener, ControlStream, ForwardStatus, PeerStatus, Status};
use paxri_native::gateway::NativeTransport;
mod reverse;
mod streams;
mod terminal;
mod unpair;
use reverse::{BindingState, RemoteForward, Routes};
const STREAM_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(20);

#[derive(Parser, Clone)]
#[command(version, about = "Run the resident paxri gateway")]
struct Options {
    /// Node configuration TOML
    #[arg(long)]
    config: PathBuf,
    /// Existing private key created with paxri identity
    #[arg(long)]
    key: PathBuf,
    /// Unix socket path or Windows local pipe name
    #[arg(long)]
    ipc: Option<PathBuf>,
    /// Allow this background gateway to be managed by the desktop and CLI
    #[arg(long)]
    user_managed: bool,
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
    route: tokio::sync::watch::Sender<(String, String)>,
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
                    .send_replace((rule.peer.clone(), rule.target.clone()));
            }
            Self::Remote(r) => r.set_target(rule.target.clone()),
        }
    }
}

struct Gateway {
    active_native: paxri_config::Native,
    user_managed: bool,
    pairing: pairing::Pairings,
    config: Arc<Config>,
    file: SettingsFile,
    direct_only: bool,
    transport: Arc<NativeTransport>,
    session_keep_alive: tokio::sync::watch::Sender<paxri_config::startup::SessionKeepAlive>,
    peer_changes: tokio::sync::watch::Receiver<BTreeMap<String, i64>>,
    device_info: tokio::sync::watch::Receiver<paxri_native::gateway::PeerDeviceInfo>,
    listeners: BTreeMap<String, Listener>,
    reverse_routes: Routes,
    streams: Arc<Semaphore>,
}

impl Gateway {
    fn canonicalize_peer(&self, selector: &mut String) -> Result<()> {
        let (name, _) = self.config.resolve_peer(selector)?;
        if name != selector {
            selector.clone_from(name);
        }
        Ok(())
    }

    fn device_info_for(&self, peer: &paxri_config::Peer) -> Option<paxri_config::DeviceInfo> {
        self.device_info.borrow().get(&peer.endpoint_id.to_ascii_lowercase()).cloned()
    }

    fn peer_device_name(&self, peer: &paxri_config::Peer) -> Option<String> {
        self.device_info_for(peer).and_then(|info| info.device_name)
            .or_else(|| peer.device_name.clone())
    }

    fn peer_platform(&self, peer: &paxri_config::Peer) -> Option<paxri_config::DevicePlatform> {
        self.device_info_for(peer).map(|i| i.platform)
            .filter(|p| *p != paxri_config::DevicePlatform::Unknown)
            .or(peer.platform)
    }

    fn peer_status(&self, selector: &str) -> Result<PeerStatus> {
        let (name, peer) = self.config.resolve_peer(selector)?;
        let session = self.transport.online_session(name).ok();
        Ok(PeerStatus {
            peer: name.clone(),
            device_id: peer.endpoint_id.clone(),
            platform: self.peer_platform(peer).unwrap_or_default(),
            capabilities: self.device_info_for(peer).map(|i| i.capabilities),
            state: if session.is_some() { "Connected" } else { "Offline" }.into(),
            connections: Some(session.map(|s| s.status()).into_iter().collect()),
        })
    }

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
            rule.target.clone(),
            self.reverse_routes.clone(),
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
            tokio::sync::watch::channel((rule.peer.clone(), rule.target.clone()));
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
                    if let Err(e) = paxri_core::forward(&*transport, &peer, &target, tcp).await {
                        eprintln!("forward to {peer}/{target}: {e:#}");
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

    async fn change_settings(&mut self, mut change: Command) -> Result<()> {
        // Store the configured alias while holding the same lock as the settings write.
        if let Command::SaveForward {
            rule: Forward { peer, .. },
            ..
        } = &mut change
        {
            self.canonicalize_peer(peer)?;
        }
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
        if let Command::SaveForward { name, edit: false, rule, .. } = &mut change
            && name.is_empty()
        {
            let base = format!("tcp-{}", rule.listen.port());
            *name = std::iter::once(base.clone())
                .chain((2..).map(|n| format!("{base}-{n}")))
                .find(|id| !self.config.forwards.contains_key(id))
                .expect("available forward ID");
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
        let local = paxri_config::file::read_local(&self.file.path).ok();
        let native = local.as_ref().map_or(&config.native, |p| &p.native);
        Status {
            peer_presence: true,
            network_restart_required: native != &self.active_native,
            native: Some(native.clone()),
            startup: local
                .as_ref()
                .map_or(config.gateway.startup, |p| p.gateway.startup),
            session_keep_alive: local.as_ref().map_or_else(
                || *self.session_keep_alive.borrow(),
                |p| p.gateway.session_keep_alive,
            ),
            user_managed: self.user_managed,
            executable: std::env::current_exe().ok(),
            ipc_path: None,
            config_path: Some(self.file.path.clone()),
            gui: local.map_or_else(|| config.gui.clone(), |p| p.gui),
            pairing: self.pairing.status(config),
            device_id: self.transport.id(),
            computer_name: gethostname::gethostname().to_string_lossy().into_owned(),
            device_name: config.device_name.clone(),
            ticket,
            peer_connections: Some(self.transport.connections()),
            peer_last_status_change_ms: self.peer_change_times(),
            peer_platforms: config
                .peers
                .iter()
                .filter_map(|(name, peer)| Some((name.clone(), self.peer_platform(peer)?)))
                .collect(),
            peer_device_names: config.peers.iter()
                .filter_map(|(alias, peer)| Some((alias.clone(), self.peer_device_name(peer)?)))
                .collect(),
            peer_capabilities: config.peers.iter()
                .filter_map(|(name, peer)| Some((name.clone(), self.device_info_for(peer)?.capabilities)))
                .collect(),
            peers: config
                .peers
                .iter()
                .map(|(name, p)| (name.clone(), p.endpoint_id.clone()))
                .collect(),
            peer_forwards: None,
            forwards: self.forward_status(None),
        }
    }

    fn forward_status(&self, peer_id: Option<&str>) -> BTreeMap<String, ForwardStatus> {
        self.config
            .forwards
            .iter()
            .filter(|(_, rule)| {
                peer_id.is_none_or(|id| self.config.peers[&rule.peer].endpoint_id.eq_ignore_ascii_case(id))
            })
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
                    target: rule.target.clone(),
                    enabled: Some(self.listeners.contains_key(name)),
                    error: state.error,
                };
                (name.clone(), status)
            })
            .collect()
    }
}

async fn check_peer<'a>(
    gateway: &'a Mutex<Gateway>,
    selector: &str,
) -> Result<(tokio::sync::MutexGuard<'a, Gateway>, String)> {
    let (transport, name, expected_id) = {
        let state = gateway.lock().await;
        let (name, peer) = state.config.resolve_peer(selector)?;
        (state.transport.clone(), name.clone(), peer.endpoint_id.clone())
    };
    tokio::time::timeout(
        std::time::Duration::from_secs(25),
        paxri_core::peer_platform(&*transport, &name),
    )
    .await
    .with_context(|| format!("Checking connection to {name} timed out"))??;
    let state = gateway.lock().await;
    anyhow::ensure!(
        state.config.peers.get(&name).is_some_and(|p| p.endpoint_id == expected_id),
        "Pairing changed while checking {name}"
    );
    Ok((state, name))
}

async fn handle_client(
    mut socket: ControlStream,
    gateway: Arc<Mutex<Gateway>>,
    management_slot: OwnedSemaphorePermit,
    shutdown: Option<tokio::sync::mpsc::Sender<ControlStream>>,
) -> Result<()> {
    let mut command = match paxri_control::receive(&mut socket).await {
        Ok(command) => command,
        Err(e) => return paxri_control::respond::<()>(&mut socket, Err(e)).await,
    };
    // Resolve only outgoing local requests. Transport authentication and incoming
    // stream authorization continue to use the complete public key.
    if let Command::ManagePeerForward { owner: peer, .. }
    | Command::Connect { peer, .. }
    | Command::ManagedTerminal { peer, .. }
    | Command::OpenTerminal { peer, .. }
    | Command::Exec { peer, .. }
    | Command::ExecPipe { peer, .. }
    | Command::OpenFiles { peer } = &mut command
    {
        let result = gateway.lock().await.canonicalize_peer(peer);
        if let Err(error) = result {
            return paxri_control::respond::<()>(&mut socket, Err(error)).await;
        }
    }
    match command {
        Command::ListForwards => {
            let status = forward_management::list(&gateway).await;
            paxri_control::respond(&mut socket, Ok(status)).await
        }
        Command::ManagePeerForward { owner, action } => {
            let result = forward_management::change(&gateway, &owner, action).await;
            paxri_control::respond(&mut socket, result).await
        }
        Command::Shutdown => {
            if let Some(sender) = shutdown {
                sender
                    .send(socket)
                    .await
                    .map_err(|_| anyhow::anyhow!("Gateway is already stopping"))
            } else {
                paxri_control::respond::<()>(
                    &mut socket,
                    Err(anyhow::anyhow!(
                        "This gateway is managed by its external launcher"
                    )),
                )
                .await
            }
        }
        Command::Status => {
            let status = gateway.lock().await.status();
            paxri_control::respond(&mut socket, Ok(status)).await
        }
        Command::PeerStatus { peer, check } => {
            let snapshot = if check {
                check_peer(&gateway, &peer).await.and_then(|(state, name)| state.peer_status(&name))
            } else {
                gateway.lock().await.peer_status(&peer)
            };
            paxri_control::respond(&mut socket, snapshot).await
        }
        Command::WatchPeerConnections => {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            let state = gateway.lock().await;
            let mut changes = state.transport.subscribe_peer_changes();
            let mut info = state.transport.subscribe_device_info();
            let subscription = state.streams.clone().try_acquire_owned();
            drop(state);
            let Ok(_subscription) = subscription else {
                return paxri_control::respond::<()>(&mut socket, Err(anyhow::anyhow!("gateway stream limit reached"))).await;
            };
            drop(management_slot);
            paxri_control::respond(&mut socket, Ok(())).await?;
            let (mut reader, mut writer) = tokio::io::split(socket);
            tokio::select! {
                _ = reader.read_u8() => Ok(()),
                result = async {
                    loop {
                        writer.write_all(&[1]).await?;
                        let event = tokio::select! {
                            event = changes.changed() => event,
                            event = info.changed() => event,
                        };
                        if event.is_err() { return Ok(()); }
                    }
                } => result,
            }
        }
        Command::CheckPeer { peer } => {
            let snapshot = check_peer(&gateway, &peer).await.map(|(mut state, _)| state.status());
            paxri_control::respond(&mut socket, snapshot).await
        }
        Command::SetNative { change } => {
            let mut state = gateway.lock().await;
            let result = paxri_config::file::set_native(&state.file.path, change, false)
                .map(|_| state.status());
            drop(state);
            paxri_control::respond(&mut socket, result).await
        }
        Command::SetGateway { change } => {
            let mut state = gateway.lock().await;
            let result = paxri_config::file::set_gateway(&state.file.path, change, false)
                .map(|updated| {
                    state.session_keep_alive.send_replace(updated.session_keep_alive);
                    state.status()
                });
            drop(state);
            paxri_control::respond(&mut socket, result).await
        }
        Command::SetGui { change } => {
            let mut state = gateway.lock().await;
            let result = paxri_config::file::set_gui(&state.file.path, change, false)
                .map(|_| state.status());
            drop(state);
            paxri_control::respond(&mut socket, result).await
        }
        Command::Unpair { peer, expected_id } => {
            let result = unpair::remove(&gateway, &peer, expected_id.as_deref()).await;
            paxri_control::respond(&mut socket, result).await
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
            paxri_control::respond(&mut socket, result).await
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
            paxri_control::respond(&mut socket, result).await
        }
        Command::SaveForward { .. }
        | Command::RemoveForward { .. }
        | Command::SetDeviceName { .. } => {
            let result = {
                let mut gateway = gateway.lock().await;
                gateway
                    .change_settings(command)
                    .await
                    .map(|_| gateway.status())
            };
            paxri_control::respond(&mut socket, result).await
        }
        Command::ManagedTerminal { peer, action } => {
            use paxri_terminal::managed::{Request, request};
            let (session, permit) = {
                let state = gateway.lock().await;
                let session = state.transport.online_session(&peer).and_then(|session| {
                    if state.config.peers.get(&peer).and_then(|p| state.device_info_for(p))
                        .is_some_and(|info| !info.capabilities.managed_terminals) {
                        anyhow::bail!("Update the remote gateway to use managed terminal sessions");
                    }
                    Ok(session)
                });
                (session, state.streams.clone().try_acquire_owned())
            };
            drop(management_slot);
            let opened = async {
                let permit = permit.context("gateway stream limit reached")?;
                let (stream, keys) = request(session?, action).await?;
                anyhow::Ok((permit, stream, keys))
            }.await;
            let (_permit, mut stream, keys) = match opened {
                Ok(result) => result,
                Err(e) => return paxri_control::respond::<()>(&mut socket, Err(e)).await,
            };
            paxri_control::respond(&mut socket, Ok(keys)).await?;
            if matches!(action, Request::Create(..) | Request::Attach(..)) {
                let (mut read, mut write) = tokio::io::split(&mut stream);
                let (mut input, mut output) = tokio::io::split(socket);
                tokio::select! {
                    r = tokio::io::copy(&mut input, &mut write) => r?,
                    r = tokio::io::copy(&mut read, &mut output) => r?,
                };
            }
            Ok(())
        }
        open @ (Command::Connect { .. }
        | Command::OpenTerminal { .. }
        | Command::Exec { .. }
        | Command::ExecPipe { .. }
        | Command::OpenFiles { .. }) => {
            let (transport, permit) = {
                let gateway = gateway.lock().await;
                let required = match &open {
                    Command::OpenTerminal { peer, .. } => Some((peer, true)),
                    Command::Exec { peer, .. } | Command::ExecPipe { peer, .. } => Some((peer, false)),
                    _ => None,
                };
                if let Some((name, shell)) = required
                    && let Some(info) = gateway.config.peers.get(name).and_then(|p| gateway.device_info_for(p))
                    && !(if shell { info.capabilities.shell } else { info.capabilities.exec })
                {
                    let service = if shell { "an interactive shell" } else { "command execution" };
                    let error = anyhow::anyhow!("{name} does not host {service}");
                    drop(gateway);
                    return paxri_control::respond::<()>(&mut socket, Err(error)).await;
                }
                (
                    gateway.transport.clone(),
                    gateway.streams.clone().try_acquire_owned(),
                )
            };
            let Ok(_permit) = permit else {
                return paxri_control::respond::<()>(
                    &mut socket,
                    Err(anyhow::anyhow!("gateway stream limit reached")),
                )
                .await;
            };
            drop(management_slot);
            if let Command::Connect { peer, target } = open {
                let stream = match paxri_core::open_tcp(&*transport, &peer, &target).await {
                    Ok(stream) => stream,
                    Err(e) => return paxri_control::respond::<()>(&mut socket, Err(e)).await,
                };
                paxri_control::respond(&mut socket, Ok(())).await?;
                let (reader, writer) = tokio::io::split(stream);
                return paxri_control::bridge(socket, reader, writer)
                    .await
                    .with_context(|| format!("stream to {peer}/{target}"));
            }
            let opened = async {
                match open {
                    Command::OpenTerminal { peer, size } => {
                        terminal::open_shell(transport.online_session(&peer)?, size).await
                    }
                    Command::Exec {
                        peer,
                        size,
                        invocation,
                    } => terminal::open(transport.online_session(&peer)?, Some(size), invocation).await,
                    Command::ExecPipe { peer, invocation } => {
                        terminal::open(transport.online_session(&peer)?, None, invocation).await
                    }
                    Command::OpenFiles { peer } => {
                        use paxri_core::{StreamKind, Transport};
                        transport.open(&peer, StreamKind::Files).await
                    }
                    _ => unreachable!(),
                }
            };
            use tokio::io::AsyncReadExt;
            let opened = tokio::select! {
                result = opened => result,
                _ = socket.read_u8() => return Ok(()),
            };
            let stream = match opened {
                Ok(connection) => connection,
                Err(e) => return paxri_control::respond::<()>(&mut socket, Err(e)).await,
            };
            paxri_control::respond(&mut socket, Ok(())).await?;
            let (mut recv, mut send) = tokio::io::split(stream);
            let (mut input, mut output) = tokio::io::split(socket);
            // Closing either endpoint cancels this framed application stream.
            tokio::select! {
                r = tokio::io::copy(&mut input, &mut send) => r?,
                r = tokio::io::copy(&mut recv, &mut output) => r?,
            };
            Ok(())
        }
    }
}

async fn run(
    options: Options,
    stop: impl Future<Output = Result<()>>,
    ready: impl FnOnce() -> Result<()>,
) -> Result<()> {
    let path = options.ipc.unwrap_or_else(paxri_control::default_address);
    let control = ControlListener::bind(&path)
        .await
        .with_context(|| format!("bind IPC {}", path.display()))?;
    #[cfg(unix)]
    let path = std::path::absolute(path)?;
    let profile = std::path::absolute(&options.config)?;
    let profile_dir = profile.parent().context("Missing profile directory")?;
    let tools = terminal::SessionTools::new(path.clone(), profile_dir);
    let _profile_lock = if options.user_managed {
        let parent = options
            .config
            .parent()
            .context("Missing profile directory")?;
        std::fs::create_dir_all(parent)?;
        let mut name = options.key.as_os_str().to_owned();
        name.push(".gateway.lock");
        let lock = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(name)?;
        lock.try_lock()
            .context("Another background gateway is using this device key")?;
        paxri_config::file::init(&options.config)?;
        Some(lock)
    } else {
        None
    };
    let text = std::fs::read_to_string(&options.config)
        .with_context(|| format!("read {}", options.config.display()))?;
    let config = Arc::new(paxri_config::parse(&text).map_err(anyhow::Error::msg)?);
    let key = if options.user_managed {
        paxri_native::identity::init(&options.key)?
    } else {
        paxri_native::identity::load(&options.key)?
    };
    let (pair_tx, mut pair_requests) = tokio::sync::mpsc::channel(16);
    let (stream_tx, stream_rx) = tokio::sync::mpsc::channel(64);
    let (address_tx, mut peer_addresses) = tokio::sync::mpsc::channel(32);
    let (transport, router, mapping) = paxri_native::gateway::start(
        key,
        config.clone(),
        options.no_relay,
        terminal::capabilities(),
        pair_tx,
        stream_tx,
        address_tx,
    )
    .await?;
    let routes = Routes::default();
    let mut clients = JoinSet::new();
    let stream_config = transport.subscribe_config();
    let mut peer_changes = transport.subscribe_peer_changes();
    peer_changes.mark_changed();
    let mut device_info = transport.subscribe_device_info();
    let mut gateway = Gateway {
        active_native: config.native.clone(),
        user_managed: options.user_managed,
        pairing: Default::default(),
        config: config.clone(),
        file: SettingsFile {
            path: options.config.canonicalize()?,
            text,
        },
        direct_only: options.no_relay,
        transport: Arc::new(transport),
        session_keep_alive: tokio::sync::watch::channel(config.gateway.session_keep_alive).0,
        peer_changes: peer_changes.clone(),
        device_info: device_info.clone(),
        listeners: BTreeMap::new(),
        reverse_routes: routes.clone(),
        streams: Arc::new(Semaphore::new(256)),
    };
    if let Err(e) = gateway.init_pairing().await {
        gateway.pairing.message = Some(format!("Pairing code unavailable: {e:#}"));
    }
    for name in config.forwards.keys() {
        gateway.set_forward(name, true, None).await?;
    }
    if options.user_managed
        && let Err(error) =
            paxri_config::file::record_gateway_address(&gateway.file.path, &path)
    {
        eprintln!("Could not record local gateway address: {error:#}");
    }
    ready()?;
    eprintln!(
        "gateway {} ready at {}",
        gateway.transport.id(),
        path.display()
    );
    clients.spawn(terminal::watch_capabilities(gateway.transport.clone()));
    clients.spawn(gateway.transport.clone().announce_presence());
    let gateway = Arc::new(Mutex::new(gateway));
    clients.spawn(streams::serve(
        stream_rx, stream_config, routes, gateway.clone(), tools,
    ));
    let management = Arc::new(Semaphore::new(32));
    clients.spawn(platforms::fill_missing(gateway.clone()));
    clients.spawn(unpair::retry_pending(gateway.clone()));
    tokio::pin!(stop);
    let (shutdown, mut requests) = tokio::sync::mpsc::channel(1);
    let shutdown = options.user_managed.then_some(shutdown);
    let (result, reply) = loop {
        tokio::select! {
            result = &mut stop => break (result, None),
            Some(client) = requests.recv(), if shutdown.is_some() => break (Ok(()), Some(client)),
            Ok(()) = peer_changes.changed() => {
                if let Err(err) = gateway.lock().await.persist_peer_changes() {
                    eprintln!("Saving peer connection times: {err:#}");
                }
            }
            Ok(()) = device_info.changed() => {
                if let Err(err) = gateway.lock().await.persist_peer_info() {
                    eprintln!("Saving peer information: {err:#}");
                }
            }
            Some(ticket) = peer_addresses.recv() => {
                if let Err(err) = gateway.lock().await.update_peer_ticket(ticket) {
                    eprintln!("Saving peer addresses: {err:#}");
                }
            }
            Some(request) = pair_requests.recv() => {
                use paxri_native::gateway::pairing::Incoming;
                let mut state = gateway.lock().await;
                match request {
                    Incoming::Join(join) => state.pairing.receive(join),
                    Incoming::Unpair { authenticated_id, pairing_id, reply } => {
                        let result = state.receive_unpair(&authenticated_id, pairing_id.as_deref()).await;
                        let _ = reply.send(result.map_err(|e| {
                            eprintln!("Unpairing {authenticated_id}: {e:#}");
                            "Could not save the unpair request".into()
                        }));
                    }
                }
            }

            accepted = async {
                let slot = management.clone().acquire_owned().await?;
                Ok::<_, anyhow::Error>((control.accept().await?, slot))
            } => {
                let (socket, management_slot) = accepted?;
                let gateway = gateway.clone();
                let shutdown = shutdown.clone();
                clients.spawn(async move {
                    if let Err(e) = handle_client(socket, gateway, management_slot, shutdown).await {
                        eprintln!("IPC client: {e:#}");
                    }
                });
            },
            _ = clients.join_next(), if !clients.is_empty() => {}
        }
    };
    clients.shutdown().await;
    if let Some(mapping) = mapping {
        mapping.stop().await;
    }
    let result = result.and(router.shutdown().await.map_err(anyhow::Error::from));
    if let Err(err) = gateway.lock().await.persist_peer_changes() {
        eprintln!("Saving peer connection times: {err:#}");
    }
    drop(gateway);
    drop(_profile_lock);
    drop(control);
    if let Some(mut client) = reply {
        paxri_control::respond(
            &mut client,
            result
                .as_ref()
                .map(|_| ())
                .map_err(|e| anyhow::anyhow!("{e:#}")),
        )
        .await?;
    }
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
    #[cfg(windows)]
    terminal::launch_if_requested()?;
    let options = Options::parse();
    #[cfg(windows)]
    if options.windows_service {
        return windows::dispatch();
    }
    #[cfg(windows)]
    if options.user_managed {
        // Detached processes have no console control handler. Local IPC owns shutdown.
        return tokio::runtime::Runtime::new()?.block_on(run(
            options,
            std::future::pending(),
            || Ok(()),
        ));
    }
    tokio::runtime::Runtime::new()?.block_on(run(options, signal(), || Ok(())))
}
