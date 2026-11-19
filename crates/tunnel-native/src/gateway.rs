//! iroh adapter for the resident gateway. Each paired destination has its own connection pool slot.
use anyhow::{Context, Result, ensure};
use iroh::{
    Endpoint, EndpointAddr, EndpointId, SecretKey,
    endpoint::{Connection, RecvStream, SendStream, presets},
    protocol::{AcceptError, ProtocolHandler, Router},
};
use std::{
    collections::BTreeMap,
    sync::{Arc, RwLock},
};
use tokio::{
    io::Join,
    sync::{Mutex, Semaphore},
    task::JoinSet,
    time::timeout,
};
use tunnel_config::Config;
use tunnel_core::{PROTOCOL, Transport};

use crate::{CONNECT_TIMEOUT, EndpointTicket};
pub mod pairing;
mod reverse;
mod terminal;
pub use reverse::{BindingState, RemoteForward};
use tokio::sync::watch;

struct Destination {
    address: EndpointAddr,
    connection: Mutex<Option<Connection>>,
}

pub struct NativeTransport {
    endpoint: Endpoint,
    peers: RwLock<BTreeMap<String, Arc<Destination>>>,
    config: watch::Sender<Arc<Config>>,
}

impl Transport for NativeTransport {
    type Stream = Join<RecvStream, SendStream>;

    async fn open(&self, peer: &str) -> Result<Self::Stream> {
        let dest = self.peer(peer)?;
        timeout(CONNECT_TIMEOUT, async {
            let connection = {
                let mut slot = dest.connection.lock().await;
                match slot.as_ref().filter(|c| c.close_reason().is_none()) {
                    Some(conn) => conn.clone(),
                    None => {
                        let conn = self
                            .endpoint
                            .connect(dest.address.clone(), PROTOCOL)
                            .await?;
                        *slot = Some(conn.clone());
                        conn
                    }
                }
            };
            let (send, recv) = connection.open_bi().await?;
            Ok(tokio::io::join(recv, send))
        })
        .await
        .context("opening peer stream timed out")?
    }
}

impl NativeTransport {
    fn peer(&self, name: &str) -> Result<Arc<Destination>> {
        self.peers
            .read()
            .expect("peer map")
            .get(name)
            .cloned()
            .context("unknown peer")
    }

    pub fn set_peer(&self, name: String, ticket: EndpointTicket) {
        let address = ticket.into();
        let mut peers = self.peers.write().expect("peer map");
        if peers.get(&name).is_none_or(|dest| dest.address != address) {
            peers.insert(
                name,
                Arc::new(Destination {
                    address,
                    connection: Mutex::new(None),
                }),
            );
        }
    }

    pub async fn initial_pairing_ticket(&self, direct_only: bool) -> Result<EndpointTicket> {
        crate::ticket(&self.endpoint, direct_only).await
    }

    pub fn contact(&self, name: String) -> pairing::Contact {
        pairing::Contact {
            name,
            ticket: EndpointTicket::new(self.endpoint.addr()),
            platform: Some(tunnel_config::DevicePlatform::current()),
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

#[derive(Debug)]
struct Services {
    config: watch::Receiver<Arc<Config>>,
    streams: Arc<Semaphore>,
}

impl ProtocolHandler for Services {
    async fn accept(&self, connection: Connection) -> Result<(), AcceptError> {
        let peer = connection.remote_id().to_string();
        if !self
            .config
            .borrow()
            .peers
            .values()
            .any(|p| p.endpoint_id.eq_ignore_ascii_case(&peer))
        {
            connection.close(1u32.into(), b"peer not allowed");
            return Ok(());
        }
        let mut tasks = JoinSet::new();
        loop {
            tokio::select! {
                result = connection.accept_bi() => {
                    let Ok((mut send, mut recv)) = result else { break };
                    let Ok(permit) = self.streams.clone().try_acquire_owned() else {
                        let _ = send.reset(2u32.into());
                        let _ = recv.stop(2u32.into());
                        continue;
                    };
                    let config = self.config.clone();
                    let peer = peer.clone();
                    tasks.spawn(async move {
                        let _permit = permit;
                        if let Err(e) = tunnel_core::serve(|| config.borrow().clone(), &peer, tokio::io::join(recv, send)).await {
                            eprintln!("inbound stream from {peer}: {e:#}");
                        }
                    });
                }
                _ = tasks.join_next(), if !tasks.is_empty() => {}
            }
        }
        Ok(())
    }
}

pub async fn start(
    key: SecretKey,
    config: Arc<Config>,
    no_relay: bool,
    incoming: tokio::sync::mpsc::Sender<pairing::Incoming>,
) -> Result<(NativeTransport, Router)> {
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
        peers.insert(
            name.clone(),
            Arc::new(Destination {
                address,
                connection: Mutex::new(None),
            }),
        );
    }
    // With no relay or discovery the saved code's LAN ports are its only introduction path.
    let saved = config
        .pairing_code
        .as_deref()
        .and_then(|code| pairing::Invitation::parse(code).ok())
        .filter(|invite| no_relay && invite.ticket.endpoint_addr().id == key.public());
    let endpoint = if let Some(saved) = saved {
        let mut builder = Endpoint::builder(presets::Minimal)
            .secret_key(key.clone())
            .ca_tls_config(iroh::tls::CaTlsConfig::system())
            .relay_mode(iroh::RelayMode::Disabled);
        let address = saved.ticket.endpoint_addr();
        if let Some(v4) = address.ip_addrs().find(|a| a.is_ipv4()) {
            builder = builder.bind_addr((std::net::Ipv4Addr::UNSPECIFIED, v4.port()))?;
        }
        if let Some(v6) = address.ip_addrs().find(|a| a.is_ipv6()) {
            builder = builder.bind_addr_with_opts(
                (std::net::Ipv6Addr::UNSPECIFIED, v6.port()),
                iroh::endpoint::BindOpts::default().set_is_required(false),
            )?;
        }
        match builder.bind().await {
            Ok(endpoint) => endpoint,
            Err(_) => crate::bind(key, &config.native, true).await?,
        }
    } else if !no_relay && config.native.relay_url.is_none() {
        Endpoint::builder(presets::N0)
            .secret_key(key)
            .ca_tls_config(iroh::tls::CaTlsConfig::system())
            .bind()
            .await?
    } else {
        crate::bind(key, &config.native, no_relay).await?
    };
    let (config, settings) = watch::channel(config);
    let streams = Arc::new(Semaphore::new(256));
    let router = Router::builder(endpoint.clone())
        .accept(pairing::PROTOCOL, pairing::PairingHandler::new(incoming))
        .accept(
            tunnel_terminal::PROTOCOL,
            terminal::Terminals::new(settings.clone()),
        )
        .accept(
            tunnel_core::reverse::PROTOCOL,
            reverse::Listeners::new(settings.clone(), streams.clone()),
        )
        .accept(
            PROTOCOL,
            Services {
                config: settings,
                streams,
            },
        )
        .spawn();
    Ok((
        NativeTransport {
            endpoint,
            peers: RwLock::new(peers),
            config,
        },
        router,
    ))
}
