//! iroh adapter for paired, multiplexed gateway sessions.
use anyhow::{Context, Result, ensure};
use iroh::{Endpoint, EndpointAddr, EndpointId, SecretKey, protocol::Router};
use std::{
    collections::BTreeMap,
    sync::{Arc, RwLock},
};
use tokio::sync::{mpsc, watch};
use paxri_config::{Config, DeviceCapabilities, DeviceInfo, DevicePlatform};
use paxri_core::{
    Transport,
    session::{IncomingStream, PROTOCOL},
};

use crate::EndpointTicket;
pub type PeerDeviceInfo = BTreeMap<String, DeviceInfo>;
mod addresses;
mod device_info;
mod connections;
mod presence;
mod lan_mapping;
pub use lan_mapping::LanMapping;
pub mod pairing;
pub use connections::{NativeSession, NativeStream};
pub struct NativeTransport {
    endpoint: Endpoint,
    connections: Arc<connections::Connections>,
    presence_refresh: tokio::sync::Notify,
    peers: RwLock<BTreeMap<String, EndpointAddr>>,
    config: watch::Sender<Arc<Config>>,
}

impl Transport for NativeTransport {
    type Session = NativeSession;
    async fn session(&self, peer: &str) -> Result<NativeSession> {
        let address = self
            .peers
            .read()
            .expect("peer map")
            .get(peer)
            .cloned()
            .context("unknown peer")?;
        self.connections.get(&self.endpoint, address).await
    }
}
impl NativeTransport {
    pub fn online_session(&self, peer: &str) -> Result<NativeSession> {
        let peers = self.peers.read().expect("peer map");
        let address = peers.get(peer).context("unknown peer")?;
        self.connections.live(address.id)
            .with_context(|| format!("{peer} is offline"))
    }

    pub async fn reconnect_peer(&self, peer: &str) -> Result<NativeSession> {
        let address = self
            .peers
            .read()
            .expect("peer map")
            .get(peer)
            .cloned()
            .context("unknown peer")?;
        let result = async {
            let session = self.connections.reconnect(&self.endpoint, address).await?;
            paxri_core::peer_platform(self, peer).await?;
            Ok(session)
        }
        .await;
        self.refresh_presence();
        result
    }

    pub fn endpoint_online(&self, id: &str) -> bool {
        id.parse().ok().is_some_and(|id| self.connections.live(id).is_some())
    }

    pub fn subscribe_peer_changes(&self) -> watch::Receiver<BTreeMap<String, i64>> {
        self.connections.changes.subscribe()
    }

    pub fn connections(&self) -> BTreeMap<String, Vec<paxri_core::connection::PeerConnection>> {
        let live = self.connections.snapshot();
        self.peers
            .read()
            .expect("peer map")
            .iter()
            .map(|(name, dest)| {
                (
                    name.clone(),
                    live.get(&dest.id).cloned().into_iter().collect(),
                )
            })
            .collect()
    }

    pub fn set_peer(&self, name: String, ticket: EndpointTicket) {
        self.peers
            .write()
            .expect("peer map")
            .insert(name, ticket.into());
    }

    pub fn peer_ticket(&self, name: &str) -> Result<EndpointTicket> {
        let mut addr = self.peers.read().expect("peer map").get(name).cloned().context("Unknown device address")?;
        if let Some(session) = self.connections.live(addr.id) {
            session.add_paths(&mut addr);
        }
        Ok(EndpointTicket::new(addr))
    }

    /// Called after publishing the configuration without this identity.
    pub fn forget_peer(&self, id: EndpointId) {
        self.peers.write().expect("peer map").retain(|_, address| address.id != id);
        self.connections.forget(id);
    }

    pub async fn initial_pairing_ticket(&self, direct_only: bool) -> Result<EndpointTicket> {
        crate::ticket(&self.endpoint, direct_only).await
    }

    pub fn set_capabilities(&self, capabilities: DeviceCapabilities) {
        self.connections.local_info.send_if_modified(|info| {
            if info.capabilities == capabilities { return false; }
            info.capabilities = capabilities;
            true
        });
    }

    pub fn set_device_name(&self, name: String) {
        self.connections.local_info.send_if_modified(|info| {
            if info.device_name.as_ref() == Some(&name) {
                return false;
            }
            info.device_name = Some(name);
            true
        });
    }

    pub fn subscribe_device_info(&self) -> watch::Receiver<PeerDeviceInfo> {
        self.connections.peer_info.subscribe()
    }

    pub fn remember_contact(&self, peer: &pairing::Contact) {
        // An established session may already have supplied a newer snapshot.
        self.connections.peer_info.send_if_modified(|known| {
            let std::collections::btree_map::Entry::Vacant(entry) =
                known.entry(peer.id().to_string())
            else {
                return false;
            };
            entry.insert(DeviceInfo {
                device_name: (peer.platform == DevicePlatform::Android).then(|| peer.name.clone()),
                platform: peer.platform,
                capabilities: peer.capabilities,
            });
            true
        });
    }

    pub fn contact(&self, name: String) -> pairing::Contact {
        pairing::Contact {
            name,
            ticket: EndpointTicket::new(self.endpoint.addr()),
            platform: DevicePlatform::current(),
            capabilities: self.connections.local_info.borrow().capabilities,
        }
    }

    pub fn update_config(&self, config: Arc<Config>) {
        self.config.send_replace(config);
    }
    pub fn subscribe_config(&self) -> watch::Receiver<Arc<Config>> {
        self.config.subscribe()
    }
    pub fn id(&self) -> String {
        self.endpoint.id().to_string()
    }
    pub fn current_ticket(&self) -> EndpointTicket {
        EndpointTicket::new(self.endpoint.addr())
    }
}

pub async fn start(
    key: SecretKey,
    config: Arc<Config>,
    no_relay: bool,
    capabilities: DeviceCapabilities,
    incoming: mpsc::Sender<pairing::Incoming>,
    streams: mpsc::Sender<IncomingStream<NativeSession>>,
    peer_addresses: mpsc::Sender<EndpointTicket>,
) -> Result<(NativeTransport, Router, Option<LanMapping>)> {
    let mut peers = BTreeMap::new();
    for (name, peer) in &config.peers {
        let id: EndpointId = peer
            .endpoint_id
            .parse()
            .with_context(|| format!("public key for {name}"))?;
        let address = if let Some(text) = &peer.ticket {
            let ticket: EndpointTicket =
                text.parse().with_context(|| format!("ticket for {name}"))?;
            ensure!(
                ticket.endpoint_addr().id == id,
                "ticket does not match peer {name}"
            );
            ticket.endpoint_addr().clone()
        } else if let Some(url) = config.native.relay_url.as_ref().filter(|_| !no_relay) {
            EndpointAddr::new(id).with_relay_url(url.clone().into())
        } else {
            ensure!(
                !no_relay || !config.forwards.values().any(|rule| rule.peer == *name),
                "direct-only forwarding to {name} requires a peer ticket"
            );
            EndpointAddr::new(id)
        };
        peers.insert(name.clone(), address);
    }
    // Reusing the LAN socket port also keeps a router mapping useful across gateway restarts.
    let saved = config
        .pairing_code
        .as_deref()
        .and_then(|code| pairing::Invitation::parse(code).ok())
        .filter(|invite| {
            (no_relay || config.native.lan_port_mapping)
                && invite.ticket.endpoint_addr().id == key.public()
        });
    let public = crate::public_relays(&config.native, no_relay).await;
    let bind = |reuse_saved: bool| -> Result<_> {
        let mut builder =
            crate::builder(key.clone(), &config.native, no_relay, true, public.as_ref());
        if config.native.lan_port_mapping {
            // The roaming controller owns both physical LAN and default-router leases.
            builder = builder.portmapper_config(iroh::endpoint::PortmapperConfig::Disabled);
        }
        let saved = saved
            .as_ref()
            .filter(|_| reuse_saved)
            .map(|i| i.ticket.endpoint_addr());
        if let Some(v4) = saved.and_then(|a| a.ip_addrs().find(|s| s.is_ipv4())) {
            builder = builder.bind_addr((std::net::Ipv4Addr::UNSPECIFIED, v4.port()))?;
        }
        if let Some(v6) = saved.and_then(|a| a.ip_addrs().find(|s| s.is_ipv6())) {
            builder = builder.bind_addr_with_opts(
                (std::net::Ipv6Addr::UNSPECIFIED, v6.port()),
                iroh::endpoint::BindOpts::default().set_is_required(false),
            )?;
        }
        Ok(builder)
    };
    let endpoint = match bind(true)?.bind().await {
        Ok(endpoint) => endpoint,
        Err(_) if saved.is_some() => bind(false)?.bind().await?,
        Err(e) => return Err(e.into()),
    };
    if let Some(public) = public {
        public.monitor(endpoint.clone());
    }
    let mapping = config
        .native
        .lan_port_mapping
        .then(|| LanMapping::start(endpoint.clone(), config.native.upnp));
    let (config, settings) = watch::channel(config);
    let connections = Arc::new(connections::Connections::new(
        endpoint.clone(),
        settings,
        streams,
        peer_addresses,
        DeviceInfo { platform: DevicePlatform::current(), capabilities, device_name: None },
    ));
    let router = Router::builder(endpoint.clone())
        .accept(pairing::PROTOCOL, pairing::PairingHandler::new(incoming))
        .accept(PROTOCOL, connections.clone())
        .spawn();
    Ok((
        NativeTransport {
            presence_refresh: Default::default(),
            endpoint,
            connections,
            peers: RwLock::new(peers),
            config,
        },
        router,
        mapping,
    ))
}
