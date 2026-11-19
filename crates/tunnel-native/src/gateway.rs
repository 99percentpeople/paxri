//! iroh adapter for the resident gateway. Each paired destination has its own connection pool slot.
use anyhow::{Context, Result, ensure};
use iroh::{
    Endpoint, EndpointAddr, EndpointId, SecretKey,
    endpoint::{Connection, RecvStream, SendStream, presets},
    protocol::{AcceptError, ProtocolHandler, Router},
};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
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
mod reverse;
pub use reverse::{BindingState, RemoteForward};
use tokio::sync::watch;

struct Destination {
    address: EndpointAddr,
    connection: Mutex<Option<Connection>>,
}

pub struct NativeTransport {
    endpoint: Endpoint,
    peers: BTreeMap<String, Destination>,
    config: watch::Sender<Arc<Config>>,
}

impl Transport for NativeTransport {
    type Stream = Join<RecvStream, SendStream>;

    async fn open(&self, peer: &str) -> Result<Self::Stream> {
        let dest = self.peers.get(peer).context("unknown peer")?;
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
    pub fn update_config(&self, config: Arc<Config>) {
        self.config.send_replace(config);
    }
    pub fn id(&self) -> String {
        self.endpoint.id().to_string()
    }
    pub fn ticket(&self) -> String {
        EndpointTicket::new(self.endpoint.addr()).to_string()
    }
}

#[derive(Debug)]
struct Services {
    config: watch::Receiver<Arc<Config>>,
    paired: BTreeSet<EndpointId>,
    streams: Arc<Semaphore>,
}

impl ProtocolHandler for Services {
    async fn accept(&self, connection: Connection) -> Result<(), AcceptError> {
        let peer = connection.remote_id();
        if !self.paired.contains(&peer) {
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
                    tasks.spawn(async move {
                        let _permit = permit;
                        if let Err(e) = tunnel_core::serve(|| config.borrow().clone(), &peer.to_string(), tokio::io::join(recv, send)).await {
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
) -> Result<(NativeTransport, Router)> {
    let mut peers = BTreeMap::new();
    let mut paired = BTreeSet::new();
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
        paired.insert(id);
        peers.insert(
            name.clone(),
            Destination {
                address,
                connection: Mutex::new(None),
            },
        );
    }
    let endpoint = if !no_relay && config.native.relay_url.is_none() {
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
        .accept(
            tunnel_core::reverse::PROTOCOL,
            reverse::Listeners::new(settings.clone(), streams.clone()),
        )
        .accept(
            PROTOCOL,
            Services {
                config: settings,
                paired,
                streams,
            },
        )
        .spawn();
    Ok((
        NativeTransport {
            endpoint,
            peers,
            config,
        },
        router,
    ))
}
