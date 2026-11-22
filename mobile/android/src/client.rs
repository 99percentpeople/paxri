use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{Arc, Weak},
    time::Duration,
};

use anyhow::{Context, Result, bail, ensure};
use async_channel::Sender;
use peerhatch_config::Config;
use peerhatch_core::{
    PeerSession, StreamKind,
    connection::{PeerConnection, unix_time_ms},
};
use peerhatch_native::{
    EndpointTicket,
    gateway::{
        self, NativeTransport,
        pairing::{Decision, Incoming, Invitation, UnpairAck},
    },
};
use peerhatch_terminal::{Frame, Size};
use serde::{Deserialize, Serialize};
use tokio::{
    sync::{Mutex, mpsc, watch},
    time::timeout,
};

use crate::Update;

#[derive(Clone, Serialize, Deserialize)]
pub struct Peer {
    pub name: String,
    #[serde(default)]
    pub platform: peerhatch_config::DevicePlatform,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capabilities: Option<peerhatch_config::DeviceCapabilities>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_status_change_ms: Option<i64>,
    ticket: EndpointTicket,
    generation: Option<String>,
}

pub type Peers = BTreeMap<String, Peer>;

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

pub struct Client {
    viewed_peer: watch::Sender<Option<String>>,
    transport: Arc<NativeTransport>,
    presence: tokio::task::JoinHandle<()>,
    _router: iroh::protocol::Router,
    peers: Mutex<Peers>,
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

    pub fn supports_shell(&self) -> bool {
        self.platform.supports_shell(self.capabilities)
    }
}

fn config(peers: &Peers) -> Result<Arc<Config>> {
    let entries: BTreeMap<_, _> = peers
        .iter()
        .map(|(id, peer)| {
            let entry = serde_json::json!({
                "endpoint_id": id, "ticket": peer.ticket.to_string(), "pairing_id": peer.generation
            });
            (id, entry)
        })
        .collect();
    Ok(Arc::new(serde_json::from_value(serde_json::json!({
        "version": 1, "native": {"upnp": false}, "peers": entries
    }))?))
}

impl Client {
    pub async fn manage_forward(
        &self,
        peer_id: &str,
        name: &str,
        action: peerhatch_control::PeerForward,
    ) -> Result<peerhatch_control::forwarding::Rules> {
        let session = self.transport.online_session(peer_id)?;
        peerhatch_control::forwarding::query(name, session, action).await
    }

    pub async fn start(dir: &Path, updates: Sender<Update>) -> Result<Arc<Self>> {
        std::fs::create_dir_all(dir)?;
        let key = peerhatch_native::identity::init(&dir.join("device.key"))?;
        let file = dir.join("peers.json");
        let peers = match std::fs::read(&file) {
            Ok(bytes) => serde_json::from_slice(&bytes).context("Reading saved devices")?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Peers::new(),
            Err(e) => return Err(e.into()),
        };
        let (incoming, mut requests) = mpsc::channel(16);
        let (streams, mut unsolicited) = mpsc::channel(16);
        let (addresses, mut changes) = mpsc::channel(16);
        let (transport, router, _) = gateway::start(
            key,
            config(&peers)?,
            false,
            peerhatch_config::DeviceCapabilities {
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
        let (viewed_peer, viewed) = watch::channel(None);
        let client = Arc::new(Self {
            viewed_peer,
            transport,
            presence,
            _router: router,
            peers: Mutex::new(peers),
            file,
        });
        tokio::spawn(Self::watch_connections(
            Arc::downgrade(&client),
            viewed,
            updates.clone(),
        ));
        let weak = Arc::downgrade(&client);
        tokio::spawn(async move {
            let mut flows = tokio::task::JoinSet::new();
            let slots = Arc::new(tokio::sync::Semaphore::new(256));
            let Some(client) = weak.upgrade() else { return };
            let config = client.transport.subscribe_config();
            let listeners = Arc::new(peerhatch_core::reverse::Listeners::new(
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
                                let result = client.remove(&authenticated_id, pairing_id.as_deref()).await;
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
                                        _ => peerhatch_core::serve(|| current.borrow().clone(), &id, request.stream).await,
                                    }
                                } => if let Err(error) = result { log::debug!("Forwarding stream: {error:#}"); }
                            }
                        });
                    }
                    _ = flows.join_next(), if !flows.is_empty() => {}
                    changed = info.changed() => {
                        if changed.is_err() { break; }
                        let Some(client) = weak.upgrade() else { break };
                        let mut peers = client.peers.lock().await;
                        let mut next = peers.clone();
                        let mut changed = false;
                        for (id, data) in info.borrow_and_update().iter() {
                            if let Some(peer) = next.get_mut(id) {
                                if data.platform != peerhatch_config::DevicePlatform::Unknown && peer.platform != data.platform {
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
                            *peers = next.clone();
                            drop(peers);
                            let _ = updates.send(Update::Devices(next)).await;
                        }
                    }
                    ticket = changes.recv() => {
                        let (Some(ticket), Some(client)) = (ticket, weak.upgrade()) else { break };
                        let id = ticket.endpoint_addr().id.to_string();
                        let mut peers = client.peers.lock().await;
                        if peers.get(&id).is_some_and(|old| old.ticket != ticket) {
                            let mut next = peers.clone();
                            next.get_mut(&id).unwrap().ticket = ticket.clone();
                            match client.save(&next) {
                                Ok(()) => {
                                    *peers = next;
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

    pub fn watch_peer(&self, id: Option<&str>) {
        self.viewed_peer.send_if_modified(|current| {
            if current.as_deref() == id {
                return false;
            }
            *current = id.map(str::to_owned);
            true
        });
    }

    async fn watch_connections(
        weak: Weak<Self>,
        mut viewed: watch::Receiver<Option<String>>,
        updates: Sender<Update>,
    ) {
        let Some(transport) = weak.upgrade().map(|c| c.transport.clone()) else {
            return;
        };
        let mut changes = transport.subscribe_peer_changes();
        let mut config = transport.subscribe_config();
        let mut times = BTreeMap::new();
        let mut foreground = crate::ACTIVITY_VISIBLE.subscribe();
        loop {
            let latest = changes.borrow_and_update().clone();
            if times != latest {
                times = latest;
                let Some(client) = weak.upgrade() else { break };
                let mut peers = client.peers.lock().await;
                let mut dirty = false;
                for (id, changed) in &times {
                    if let Some(peer) = peers.get_mut(id)
                        && peer.last_status_change_ms.is_none_or(|old| old < *changed)
                    {
                        peer.last_status_change_ms = Some(*changed);
                        dirty = true;
                    }
                }
                if dirty && let Err(e) = client.write_peers(&peers) {
                    log::warn!("Saving peer connection times: {e}");
                }
            }
            let visible = *foreground.borrow_and_update();
            let peer = viewed.borrow_and_update().clone();
            let mut interval = Duration::from_secs(60);
            if visible {
                let snapshot = Presence {
                    paths: transport.connections(),
                    changed_at: times.clone(),
                };
                // Sample the visible connection's graphs each second. The list only needs to age its labels.
                if peer.as_deref().is_some_and(|id| snapshot.online(id)) {
                    interval = Duration::from_secs(1);
                }
                if updates.send(Update::Presence(snapshot)).await.is_err() {
                    break;
                }
            }
            tokio::select! {
                result = changes.changed() => if result.is_err() { break; },
                result = config.changed() => if result.is_err() { break; },
                result = viewed.changed() => if result.is_err() { break; },
                result = foreground.changed() => if result.is_err() { break; },
                _ = tokio::time::sleep(interval), if visible => {}
            }
        }
    }

    fn write_peers(&self, peers: &Peers) -> Result<()> {
        let mut temp = tempfile::NamedTempFile::new_in(self.file.parent().unwrap())?;
        serde_json::to_writer(temp.as_file_mut(), peers)?;
        temp.as_file().sync_all()?;
        temp.persist(&self.file)?;
        Ok(())
    }

    fn save(&self, peers: &Peers) -> Result<()> {
        let settings = config(peers)?;
        self.write_peers(peers)?;
        // Presence reacts to the configuration event and must see the new address immediately.
        for (id, peer) in peers {
            self.transport.set_peer(id.clone(), peer.ticket.clone());
        }
        self.transport.update_config(settings);
        Ok(())
    }

    pub async fn list(&self) -> Peers {
        self.peers.lock().await.clone()
    }

    pub async fn pair(&self, code: &str) -> Result<Peers> {
        let invite = Invitation::parse(code)?;
        ensure!(
            invite.ticket.endpoint_addr().id.to_string() != self.transport.id(),
            "Choose another device’s invitation"
        );
        let attempt = uuid::Uuid::new_v4().simple().to_string();
        let contact = self
            .transport
            .contact(crate::DEVICE_NAME.borrow().effective());
        let decision = timeout(
            Duration::from_secs(120),
            self.transport.request_pairing(&invite, &contact, &attempt),
        )
        .await
        .context("Pairing approval timed out")??;
        let peer = match decision {
            Decision::Accepted(contact) => contact,
            Decision::Rejected(reason) => bail!("{reason}"),
        };
        let id = peer.id().to_string();
        let mut peers = self.peers.lock().await;
        let mut next = peers.clone();
        next.insert(
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
        *peers = next;
        Ok(peers.clone())
    }

    async fn remove(&self, id: &str, generation: Option<&str>) -> Result<UnpairAck> {
        let mut peers = self.peers.lock().await;
        let Some(peer) = peers.get(id) else {
            return Ok(UnpairAck::Cleared);
        };
        if generation != peer.generation.as_deref() {
            return Ok(UnpairAck::NewerPairing);
        }
        let endpoint = peer.ticket.endpoint_addr().id;
        let mut next = peers.clone();
        next.remove(id);
        self.save(&next)?;
        self.transport.forget_peer(endpoint);
        *peers = next;
        Ok(UnpairAck::Cleared)
    }

    pub async fn terminal(
        &self,
        id: &str,
        initial: Size,
        mut input: mpsc::UnboundedReceiver<Frame>,
        out: Sender<Update>,
        generation: u64,
    ) -> Result<u32> {
        ensure!(
            self.peers
                .lock()
                .await
                .get(id)
                .is_some_and(Peer::supports_shell),
            "This device does not host an interactive shell"
        );
        let session = self.transport.online_session(id)?;
        let stream = peerhatch_terminal::open(session, Some(initial), None).await?;
        let (mut reader, mut writer) = tokio::io::split(stream);
        out.send(Update::Connected(generation)).await?;
        let send = async {
            while let Some(frame) = input.recv().await {
                frame.write(&mut writer).await?;
            }
            bail!("Terminal closed")
        };
        let receive = async {
            loop {
                match Frame::read(&mut reader).await? {
                    Frame::Data(data) => out.send(Update::Output(generation, data)).await?,
                    Frame::Exit(status) => return Ok(status),
                    Frame::Resize(_) => bail!("Unexpected terminal resize from peer"),
                }
            }
        };
        tokio::select! {
            result = receive => result,
            result = send => result,
        }
    }
}
