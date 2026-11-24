use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{Arc, Weak},
    time::Duration,
};

use anyhow::{Context, Result, bail, ensure};
use async_channel::Sender;
use paxri_config::{Config, PendingUnpair};
use paxri_core::{
    PeerSession, StreamKind,
    connection::{PeerConnection, unix_time_ms},
};
use paxri_native::{
    EndpointTicket,
    gateway::{
        self, NativeTransport,
        pairing::{Decision, Incoming, Invitation, UnpairAck},
    },
};
use paxri_terminal::Frame;
use serde::{Deserialize, Serialize};
use tokio::{
    sync::{Mutex, mpsc, oneshot, watch},
    time::timeout,
};

use crate::Update;

#[derive(Clone, Serialize, Deserialize)]
pub struct Peer {
    pub name: String,
    #[serde(default)]
    pub platform: paxri_config::DevicePlatform,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capabilities: Option<paxri_config::DeviceCapabilities>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_status_change_ms: Option<i64>,
    ticket: EndpointTicket,
    pub(super) generation: Option<String>,
}

pub type Peers = BTreeMap<String, Peer>;

#[derive(Clone, Default, Serialize, Deserialize)]
struct SavedDevices {
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pending_unpairs: BTreeMap<String, PendingUnpair>,
    // Device IDs are the top-level keys in existing peers.json files.
    #[serde(flatten)]
    peers: Peers,
}

#[derive(Default)]
pub struct Presence {
    pub paths: BTreeMap<String, Vec<PeerConnection>>,
    pub changed_at: BTreeMap<String, i64>,
}

impl Presence {
    pub fn paths_for(&self, id: &str) -> Option<&[PeerConnection]> {
        self.paths.get(id).map(Vec::as_slice)
    }

    pub fn online(&self, id: &str) -> bool {
        self.paths_for(id).is_some_and(|paths| !paths.is_empty())
    }
}

#[derive(Clone, Default, PartialEq)]
struct ViewedPage {
    peer: Option<String>,
    sessions: bool,
    terminal: Option<u64>,
}

pub struct Client {
    viewed_peer: watch::Sender<ViewedPage>,
    transport: Arc<NativeTransport>,
    presence: tokio::task::JoinHandle<()>,
    _router: iroh::protocol::Router,
    devices: Mutex<SavedDevices>,
    file: PathBuf,
}

impl Drop for Client {
    fn drop(&mut self) {
        self.presence.abort();
    }
}

impl Peer {
    pub fn supports_forward_rules(&self) -> bool {
        self.platform.supports_forward_rules(self.capabilities)
    }

    pub fn supports_creating_forward_rules(&self) -> bool {
        self.supports_forward_rules() && self.capabilities.is_some_and(|c| c.create_forward_rules)
    }

    pub fn supports_shell(&self) -> bool {
        self.platform.supports_shell(self.capabilities)
    }
}

fn config(devices: &SavedDevices) -> Result<Arc<Config>> {
    let entries: BTreeMap<_, _> = devices.peers
        .iter()
        .map(|(id, peer)| {
            let entry = serde_json::json!({
                "endpoint_id": id, "ticket": peer.ticket.to_string(), "pairing_id": peer.generation
            });
            (id, entry)
        })
        .collect();
    Ok(Arc::new(serde_json::from_value(serde_json::json!({
        "version": 1, "native": {"upnp": false}, "peers": entries, "pending_unpairs": devices.pending_unpairs
    }))?))
}

impl Client {
    pub async fn manage_forward(
        &self,
        peer_id: &str,
        name: &str,
        action: paxri_control::PeerForward,
    ) -> Result<paxri_control::forwarding::Rules> {
        let session = self.transport.online_session(peer_id)?;
        paxri_control::forwarding::query(name, session, action).await
    }

    pub async fn start(
        dir: &Path,
        updates: Sender<Update>,
        connection_state: impl Fn(bool, bool) + Send + 'static,
    ) -> Result<Arc<Self>> {
        std::fs::create_dir_all(dir)?;
        let key = paxri_native::identity::init(&dir.join("device.key"))?;
        let file = dir.join("peers.json");
        let devices = match std::fs::read(&file) {
            Ok(bytes) => serde_json::from_slice(&bytes).context("Reading saved devices")?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => SavedDevices::default(),
            Err(e) => return Err(e.into()),
        };
        let (incoming, mut requests) = mpsc::channel(16);
        let (streams, mut unsolicited) = mpsc::channel(16);
        let (addresses, mut changes) = mpsc::channel(16);
        let (transport, router, _) = gateway::start(
            key,
            config(&devices)?,
            false,
            paxri_config::DeviceCapabilities {
                forward_rules: Some(false),
                ..Default::default()
            },
            incoming,
            streams,
            addresses,
        )
        .await?;
        let transport = Arc::new(transport);
        let mut name = crate::DEVICE_NAME.subscribe();
        transport.set_device_name(name.borrow_and_update().effective());
        let presence = tokio::spawn(transport.clone().announce_presence());
        let mut info = transport.subscribe_device_info();
        let (viewed_peer, viewed) = watch::channel(ViewedPage::default());
        let client = Arc::new(Self {
            viewed_peer,
            transport,
            presence,
            _router: router,
            devices: Mutex::new(devices),
            file,
        });
        tokio::spawn(Self::watch_connections(
            Arc::downgrade(&client),
            viewed,
            updates.clone(),
            connection_state,
        ));
        tokio::spawn(Self::deliver_unpairs(
            Arc::downgrade(&client),
            client.transport.subscribe_config(),
        ));
        let weak = Arc::downgrade(&client);
        tokio::spawn(async move {
            let mut flows = tokio::task::JoinSet::new();
            let slots = Arc::new(tokio::sync::Semaphore::new(256));
            let Some(client) = weak.upgrade() else { return };
            let config = client.transport.subscribe_config();
            let listeners = Arc::new(paxri_core::reverse::Listeners::new(
                config.clone(),
                slots.clone(),
            ));
            drop(client);
            loop {
                tokio::select! {
                    changed = name.changed() => {
                        if changed.is_err() { break; }
                        let Some(client) = weak.upgrade() else { break };
                        client.transport.set_device_name(name.borrow_and_update().effective());
                    }
                    request = requests.recv() => {
                        let Some(request) = request else { break };
                        match request {
                            Incoming::Join(request) => {
                                let reason = "Enter the computer’s pairing code on the phone.";
                                let _ = request.reply.send(Decision::Rejected(reason.into()));
                            }
                            Incoming::Unpair { authenticated_id, pairing_id, reply } => {
                                let Some(client) = weak.upgrade() else { break };
                                let result = client.remove(&authenticated_id, pairing_id.as_deref(), false).await;
                                if let Ok(UnpairAck::Cleared) = result {
                                    let _ = updates.send(Update::Devices(client.list().await)).await;
                                }
                                let _ = reply.send(result.map_err(|e| e.to_string()));
                            }
                        }
                    }
                    stream = unsolicited.recv() => {
                        let Some(request) = stream else { break };
                        let id = request.session.peer_id();
                        if !matches!(request.kind, StreamKind::Services | StreamKind::Listen) || !config.borrow().is_paired(&id) { continue; }
                        let Ok(permit) = slots.clone().try_acquire_owned() else { continue };
                        let mut cfg = config.clone();
                        let listeners = listeners.clone();
                        flows.spawn(async move {
                            let _permit = permit;
                            let current = cfg.clone();
                            tokio::select! {
                                biased;
                                _ = cfg.wait_for(|c| !c.is_paired(&id)) => {}
                                result = async {
                                    match request.kind {
                                        StreamKind::Listen => listeners.serve(request.session, request.stream).await,
                                        _ => paxri_core::serve(|| current.borrow().clone(), &id, request.stream).await,
                                    }
                                } => if let Err(error) = result { log::debug!("Forwarding stream: {error:#}"); }
                            }
                        });
                    }
                    _ = flows.join_next(), if !flows.is_empty() => {}
                    changed = info.changed() => {
                        if changed.is_err() { break; }
                        let Some(client) = weak.upgrade() else { break };
                        let mut devices = client.devices.lock().await;
                        let mut next = devices.clone();
                        let mut changed = false;
                        for (id, data) in info.borrow_and_update().iter() {
                            if let Some(peer) = next.peers.get_mut(id) {
                                if data.platform != paxri_config::DevicePlatform::Unknown && peer.platform != data.platform {
                                    peer.platform = data.platform;
                                    changed = true;
                                }
                                if peer.capabilities != Some(data.capabilities) {
                                    peer.capabilities = Some(data.capabilities);
                                    changed = true;
                                }
                            }
                        }
                        if changed {
                            if let Err(e) = client.save(&next) {
                                log::warn!("Saving peer capabilities: {e}");
                            }
                            *devices = next;
                            let snapshot = devices.peers.clone();
                            drop(devices);
                            let _ = updates.send(Update::Devices(snapshot)).await;
                        }
                    }
                    ticket = changes.recv() => {
                        let (Some(ticket), Some(client)) = (ticket, weak.upgrade()) else { break };
                        let id = ticket.endpoint_addr().id.to_string();
                        let mut devices = client.devices.lock().await;
                        if devices.peers.get(&id).is_some_and(|old| old.ticket != ticket) {
                            let mut next = devices.clone();
                            next.peers.get_mut(&id).unwrap().ticket = ticket.clone();
                            match client.save(&next) {
                                Ok(()) => {
                                    *devices = next;
                                }
                                Err(e) => log::warn!("Saving peer address: {e}"),
                            }
                        }
                    }
                }
            }
        });
        Ok(client)
    }

    pub fn watch_peer(&self, id: Option<&str>, sessions: bool, terminal: Option<u64>) {
        let page = ViewedPage {
            peer: id.map(str::to_owned),
            sessions,
            terminal,
        };
        self.viewed_peer.send_if_modified(|current| {
            if *current == page {
                return false;
            }
            *current = page;
            true
        });
    }

    async fn watch_connections(
        weak: Weak<Self>,
        mut viewed: watch::Receiver<ViewedPage>,
        updates: Sender<Update>,
        connection_state: impl Fn(bool, bool),
    ) {
        let Some(transport) = weak.upgrade().map(|c| c.transport.clone()) else {
            return;
        };
        let mut changes = transport.subscribe_peer_changes();
        let mut config = transport.subscribe_config();
        let mut times = BTreeMap::new();
        let mut foreground = crate::ACTIVITY_VISIBLE.subscribe();
        let mut last_service_state = None;
        loop {
            let latest = changes.borrow_and_update().clone();
            if times != latest {
                times = latest;
                let Some(client) = weak.upgrade() else { break };
                let mut devices = client.devices.lock().await;
                let mut dirty = false;
                for (id, changed) in &times {
                    if let Some(peer) = devices.peers.get_mut(id)
                        && peer.last_status_change_ms.is_none_or(|old| old < *changed)
                    {
                        peer.last_status_change_ms = Some(*changed);
                        dirty = true;
                    }
                }
                if dirty && let Err(e) = client.write_devices(&devices) {
                    log::warn!("Saving peer connection times: {e}");
                }
            }
            let visible = *foreground.borrow_and_update();
            let page = viewed.borrow_and_update().clone();
            let mut interval = Duration::from_secs(60);
            let paths = transport.connections();
            let state = (!paths.is_empty(), paths.values().any(|links| !links.is_empty()));
            if last_service_state != Some(state) {
                connection_state(state.0, state.1);
                last_service_state = Some(state);
            }
            if visible {
                let snapshot = Presence {
                    paths,
                    changed_at: times.clone(),
                };
                // Sample the visible connection's graphs each second. The list only needs to age its labels.
                if page.peer.as_deref().is_some_and(|id| snapshot.online(id)) {
                    interval = Duration::from_secs(1);
                }
                if page.sessions { interval = Duration::from_secs(3); }
                if updates.send(Update::Presence(snapshot)).await.is_err() {
                    break;
                }
            }
            tokio::select! {
                result = changes.changed() => if result.is_err() { break; },
                result = config.changed() => if result.is_err() { break; },
                result = viewed.changed() => if result.is_err() { break; },
                result = foreground.changed() => {
                    if result.is_err() { break; }
                    if *foreground.borrow() { transport.refresh_presence(); }
                }
                _ = tokio::time::sleep(interval), if visible => {}
            }
        }
    }

    fn write_devices(&self, devices: &SavedDevices) -> Result<()> {
        let mut temp = tempfile::NamedTempFile::new_in(self.file.parent().unwrap())?;
        serde_json::to_writer(temp.as_file_mut(), devices)?;
        temp.as_file().sync_all()?;
        temp.persist(&self.file)?;
        Ok(())
    }

    fn save(&self, devices: &SavedDevices) -> Result<()> {
        let settings = config(devices)?;
        self.write_devices(devices)?;
        // Presence reacts to the configuration event and must see the new address immediately.
        for (id, peer) in &devices.peers {
            self.transport.set_peer(id.clone(), peer.ticket.clone());
        }
        self.transport.update_config(settings);
        Ok(())
    }

    pub async fn list(&self) -> Peers {
        self.devices.lock().await.peers.clone()
    }

    pub async fn pair(&self, invite: Invitation, mut cancel: oneshot::Receiver<()>) -> Result<Option<Peers>> {
        let id = invite.ticket.endpoint_addr().id.to_string();
        ensure!(id != self.transport.id(), "Choose another device’s invitation");
        ensure!(!self.devices.lock().await.peers.contains_key(&id), "Device already paired");
        let attempt = uuid::Uuid::new_v4().simple().to_string();
        let mut pending = tokio::select! {
            biased;
            _ = &mut cancel => return Ok(None),
            result = self.transport.request_private_pairing(&invite, &attempt) => result?,
        };
        let answer = tokio::select! {
            biased;
            _ = &mut cancel => None,
            result = timeout(Duration::from_secs(120), pending.decision()) => Some(result),
        };
        let decision = match answer {
            Some(Ok(decision)) => decision?,
            withdrawn => {
                pending.withdraw();
                // Approval may already have been written by the gateway. Revoke only this attempt.
                let notice = PendingUnpair {
                    name: format!("Device {}", paxri_config::peer_id::default_prefix(&id)),
                    ticket: invite.ticket.to_string(),
                    request_id: uuid::Uuid::new_v4().simple().to_string(),
                    pairing_id: Some(attempt),
                };
                let mut devices = self.devices.lock().await;
                if devices.pending_unpairs.contains_key(&id) {
                    // Keep an older local unpair until the remote has acknowledged it.
                    drop(devices);
                    self.transport.notify_unpair(&id, &notice).await?;
                } else {
                    let mut next = devices.clone();
                    next.pending_unpairs.insert(id, notice);
                    self.save(&next)?;
                    *devices = next;
                }
                if withdrawn.is_some() { bail!("Pairing approval timed out"); }
                return Ok(None);
            }
        };
        let peer = match decision {
            Decision::Accepted(contact) => contact,
            Decision::Rejected(reason) => bail!("{reason}"),
        };
        let id = peer.id().to_string();
        let mut devices = self.devices.lock().await;
        let mut next = devices.clone();
        next.pending_unpairs.remove(&id);
        next.peers.insert(
            id,
            Peer {
                name: peer.name.clone(),
                last_status_change_ms: Some(unix_time_ms()),
                capabilities: peer.capabilities,
                platform: peer.platform.unwrap_or_default(),
                ticket: peer.ticket.clone(),
                generation: peer.supports_unpair.then_some(attempt),
            },
        );
        self.save(&next)?;
        self.transport.remember_contact(&peer);
        *devices = next;
        Ok(Some(devices.peers.clone()))
    }

    pub async fn unpair(&self, id: &str, generation: Option<&str>) -> Result<Peers> {
        let ack = self.remove(id, generation, true).await?;
        ensure!(matches!(ack, UnpairAck::Cleared), "The pairing changed. Open the device to try again.");
        Ok(self.list().await)
    }

    async fn remove(&self, id: &str, generation: Option<&str>, notify_peer: bool) -> Result<UnpairAck> {
        let mut devices = self.devices.lock().await;
        let Some(peer) = devices.peers.get(id) else {
            return Ok(UnpairAck::Cleared);
        };
        if generation != peer.generation.as_deref() {
            return Ok(UnpairAck::NewerPairing);
        }
        let endpoint = peer.ticket.endpoint_addr().id;
        let mut next = devices.clone();
        next.peers.remove(id);
        if notify_peer {
            next.pending_unpairs.insert(id.to_owned(), PendingUnpair {
                name: peer.name.clone(),
                ticket: peer.ticket.to_string(),
                request_id: uuid::Uuid::new_v4().simple().to_string(),
                pairing_id: peer.generation.clone(),
            });
        }
        self.save(&next)?;
        self.transport.forget_peer(endpoint);
        *devices = next;
        Ok(UnpairAck::Cleared)
    }

    async fn deliver_unpairs(weak: Weak<Self>, mut config: watch::Receiver<Arc<Config>>) {
        loop {
            let pending = match config.wait_for(|cfg| !cfg.pending_unpairs.is_empty()).await {
                Ok(cfg) => cfg.pending_unpairs.clone(),
                Err(_) => return,
            };
            for (id, notice) in &pending {
                let Some(client) = weak.upgrade() else { return };
                if config.borrow().pending_unpairs.get(id) != Some(notice) { continue; }
                if client.transport.notify_unpair(id, notice).await.is_ok() {
                    let mut devices = client.devices.lock().await;
                    if devices.pending_unpairs.get(id) == Some(notice) {
                        let mut next = devices.clone();
                        next.pending_unpairs.remove(id);
                        match client.save(&next) {
                            Ok(()) => *devices = next,
                            Err(error) => log::warn!("Saving unpair confirmation: {error}"),
                        }
                    }
                }
            }
            // New notices wake immediately. An unreachable device is retried on a timer.
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_secs(45)) => {}
                changed = config.wait_for(|cfg| cfg.pending_unpairs.iter().any(|(id, notice)| pending.get(id) != Some(notice))) => {
                    if changed.is_err() { return; }
                }
            }
        }
    }

    pub async fn manage_terminal(&self, peer: &str, request: paxri_terminal::managed::Request) -> Result<Vec<paxri_terminal::managed::Key>> {
        let session = self.transport.online_session(peer)?;
        paxri_terminal::managed::request(session, request).await.map(|(_, keys)| keys)
    }

    pub fn spawn_terminal(
        self: Arc<Self>,
        rt: &tokio::runtime::Handle,
        peer: String,
        action: paxri_terminal::managed::Request,
        out: Sender<Update>,
        id: u64,
    ) -> (mpsc::UnboundedSender<Frame>, tokio::task::JoinHandle<()>) {
        use paxri_terminal::managed::{Rejected, Request};
        let (input, keys) = mpsc::unbounded_channel();
        let job = rt.spawn(async move {
            let connection = self.transport.online_session(&peer).ok();
            let mut result = if matches!(action, Request::End(_)) {
                self.manage_terminal(&peer, action).await.map(|_| 0)
            } else {
                self.terminal(&peer, action, keys, out.clone(), id).await
            };
            if result
                .as_ref()
                .is_err_and(|e| e.is::<tokio::time::error::Elapsed>())
                && let Some(connection) = connection
            {
                let client = self.clone();
                let peer = peer.clone();
                tokio::spawn(async move {
                    // A single stalled viewer should not tear down working port forwards.
                    // Only retire its connection if a separate gateway request also times out.
                    if timeout(
                        Duration::from_secs(8),
                        paxri_terminal::managed::request(connection.clone(), Request::List),
                    )
                    .await
                    .is_err()
                        && client
                            .transport
                            .online_session(&peer)
                            .is_ok_and(|s| s.same_connection(&connection))
                    {
                        connection.close_unresponsive();
                        client.transport.refresh_presence();
                    }
                });
            }
            if let Request::Attach(key, _) | Request::End(key) = action
                && result.as_ref().is_err_and(|e| e.is::<Rejected>())
            {
                // The shell may have exited or an earlier stop may have reached the host.
                match self.manage_terminal(&peer, Request::List).await {
                    Ok(keys) if !keys.contains(&key) => {
                        let _ = out.send(Update::SessionGone(id)).await;
                        return;
                    }
                    Ok(_) => {}
                    Err(error) => result = Err(error),
                }
            }
            let _ = out.send(Update::End(id, result)).await;
        });
        (input, job)
    }

    async fn terminal(
        &self,
        id: &str,
        action: paxri_terminal::managed::Request,
        input: mpsc::UnboundedReceiver<Frame>,
        out: Sender<Update>,
        generation: u64,
    ) -> Result<u32> {
        ensure!(
            self.devices
                .lock()
                .await
                .peers
                .get(id)
                .is_some_and(Peer::supports_shell),
            "This device does not host an interactive shell"
        );
        use paxri_terminal::managed::{self, AttachmentEvent, Request};
        let mut visibility = crate::ACTIVITY_VISIBLE.subscribe();
        let mut connections = self.transport.subscribe_peer_changes();
        let session = self.transport.online_session(id)?;
        let heartbeat = self
            .transport
            .subscribe_device_info()
            .borrow()
            .get(id)
            .is_some_and(|info| info.capabilities.terminal_heartbeat);
        let (mut stream, _) = managed::request(session.clone(), action).await?;
        let history = timeout(Duration::from_secs(15), managed::read_history(&mut stream))
            .await
            .context("Terminal history timed out")??;
        let (Request::Attach(key, _) | Request::Create(key, _)) = action else {
            bail!("Invalid interactive terminal request");
        };
        let check_hosted = || async {
            let (_, hosted) = timeout(
                Duration::from_secs(8),
                managed::request(session.clone(), Request::List),
            )
            .await
            .context("Gateway did not acknowledge terminal check")??;
            ensure!(hosted.contains(&key), "Session has ended on the gateway");
            Ok::<(), anyhow::Error>(())
        };
        if !heartbeat {
            check_hosted().await?;
        }
        if matches!(action, Request::Attach(..)) || !history.is_empty() {
            out.send(Update::Output(generation, history, true)).await?;
        }

        let out = &out;
        let attachment = managed::attachment(stream, input, heartbeat, |event| async move {
            out.send(match event {
                AttachmentEvent::Ready => Update::Connected(generation),
                AttachmentEvent::Output(bytes) => Update::Output(generation, bytes, false),
            })
            .await?;
            Ok(())
        });
        let mut viewed = self.viewed_peer.subscribe();
        let lifecycle = async {
            loop {
                tokio::select! {
                    changed = visibility.changed() => {
                        changed?;
                        if *visibility.borrow_and_update() {
                            bail!("Reattaching terminal after returning to the foreground");
                        }
                    }
                    changed = connections.changed() => {
                        changed?;
                        if !self.transport.online_session(id).is_ok_and(|live| live.same_connection(&session)) {
                            bail!("Terminal connection was replaced");
                        }
                    }
                    changed = viewed.changed() => {
                        changed?;
                        if viewed.borrow_and_update().terminal == Some(generation) {
                            bail!("Reattaching the reopened terminal");
                        }
                    }
                }
            }
        };
        // Existing gateways can still confirm a round trip on the same QUIC connection.
        // Returning to the foreground always verifies a fresh attachment to the original PTY.
        let legacy_probe = async {
            if heartbeat {
                return std::future::pending().await;
            }
            loop {
                tokio::time::sleep(Duration::from_secs(10)).await;
                check_hosted().await?;
            }
        };
        tokio::select! {
            biased;
            result = lifecycle => result,
            result = attachment => result,
            result = legacy_probe => result,
        }
    }
}
