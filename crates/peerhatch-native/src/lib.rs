//! Native iroh transport and standalone authenticated connectivity probes.
pub mod gateway;
pub mod identity;
mod relay;

use std::time::{Duration, Instant};

use anyhow::{Context, Result, ensure};
use iroh::{
    Endpoint, EndpointAddr, EndpointId, RelayMode, SecretKey,
    endpoint::{Connection, presets},
    protocol::{AcceptError, ProtocolHandler, Router},
};
pub use iroh_tickets::endpoint::EndpointTicket;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    time::{sleep, timeout},
};
use peerhatch_config::Native;

const ALPN: &[u8] = b"p2p-tunnel/probe/1";
const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);
// Allow a stalled direct path to expire and QUIC to recover over the relay.
const IO_TIMEOUT: Duration = Duration::from_secs(35);

pub async fn bind(key: SecretKey, config: &Native, no_relay: bool) -> Result<Endpoint> {
    bind_probe(key, config, no_relay, false).await
}

pub async fn bind_probe(
    key: SecretKey,
    config: &Native,
    no_relay: bool,
    relay_only: bool,
) -> Result<Endpoint> {
    ensure!(!(no_relay && relay_only), "Incompatible probe transports");
    let public = public_relays(config, no_relay).await;
    let mut builder = builder(key, config, no_relay, false, public.as_ref());
    if relay_only {
        builder = builder.clear_ip_transports();
    }
    let endpoint = builder.bind().await?;
    if let Some(public) = public {
        public.monitor(endpoint.clone());
    }
    Ok(endpoint)
}

async fn public_relays(config: &Native, no_relay: bool) -> Option<relay::PublicRelays> {
    if no_relay || config.relay_url.is_some() {
        return None;
    }
    Some(relay::PublicRelays::rank().await)
}

fn mapping_protocols(upnp: bool) -> portmapper::Config {
    portmapper::Config { enable_upnp: upnp, ..Default::default() }
}

fn builder(
    key: SecretKey,
    config: &Native,
    no_relay: bool,
    discovery: bool,
    public: Option<&relay::PublicRelays>,
) -> iroh::endpoint::Builder {
    let builder = if discovery && !no_relay && config.relay_url.is_none() {
        Endpoint::builder(presets::N0).relay_mode(public.map_or(RelayMode::Default, |p| p.mode()))
    } else {
        let mode = match (no_relay, &config.relay_url) {
            (true, _) => RelayMode::Disabled,
            (false, Some(url)) => RelayMode::custom([url.clone().into()]),
            (false, None) => public.map_or(RelayMode::Default, |p| p.mode()),
        };
        Endpoint::builder(presets::Minimal).relay_mode(mode)
    };
    let builder = match public {
        Some(relays) => builder.net_report_config(relays.discovery()),
        None => builder,
    };
    builder
        .portmapper_config(iroh::endpoint::PortmapperConfig::Custom(mapping_protocols(config.upnp)))
        .ca_tls_config(iroh::tls::CaTlsConfig::system())
        .secret_key(key)
}

#[derive(Debug)]
struct Responder {
    allowed_peer: EndpointId,
}

impl ProtocolHandler for Responder {
    async fn accept(&self, conn: Connection) -> Result<(), AcceptError> {
        if conn.remote_id() != self.allowed_peer {
            conn.close(1u32.into(), b"peer not allowed");
            return Ok(());
        }
        serve_stream(&conn)
            .await
            .map_err(|e| AcceptError::from_boxed(e.into()))
    }
}

async fn serve_stream(conn: &Connection) -> Result<()> {
    let (mut tx, mut rx) = timeout(IO_TIMEOUT, conn.accept_bi()).await??;
    let count = timeout(IO_TIMEOUT, rx.read_u16()).await??;
    ensure!((1..=300).contains(&count), "invalid probe count");
    for seq in 1..=count {
        timeout(IO_TIMEOUT, async {
            ensure!(rx.read_u16().await? == seq, "unexpected probe sequence");
            tx.write_u16(seq).await?;
            Ok::<_, anyhow::Error>(())
        })
        .await
        .context("probe idle timeout")??;
    }
    tx.finish()?;
    // Keep the connection alive until the client has consumed the final echo.
    timeout(IO_TIMEOUT, conn.closed())
        .await
        .context("client close timeout")?;
    Ok(())
}

pub fn listen(endpoint: Endpoint, allowed_peer: EndpointId) -> Router {
    Router::builder(endpoint)
        .accept(ALPN, Responder { allowed_peer })
        .spawn()
}

pub async fn ticket(endpoint: &Endpoint, no_relay: bool) -> Result<EndpointTicket> {
    if !no_relay {
        // A LAN or already reachable IP can work even when the relay is unavailable.
        let _ = timeout(Duration::from_secs(8), endpoint.online()).await;
    }
    let addr = endpoint.addr();
    ensure!(!addr.is_empty(), "no reachable address candidates yet");
    Ok(EndpointTicket::new(addr))
}

/// Bootstrap a paired connection through the peer's known home relay.
pub fn relay_ticket(peer: EndpointId, config: &Native) -> Result<EndpointTicket> {
    let relay = config.relay_url.as_ref().context(
        "pass --ticket or configure native.relay_url on both peers to use their shared relay",
    )?;
    let addr = EndpointAddr::new(peer).with_relay_url(relay.clone().into());
    Ok(EndpointTicket::new(addr))
}

pub struct Sample {
    pub sequence: u16,
    pub elapsed: Duration,
    pub path: &'static str,
    pub address: String,
    pub path_tx_bytes: u64,
    pub path_rx_bytes: u64,
}

pub async fn probe(
    endpoint: &Endpoint,
    dest: EndpointTicket,
    expected: EndpointId,
    count: u16,
    mut report: impl FnMut(Sample),
) -> Result<()> {
    ensure!((1..=300).contains(&count), "probe count must be in 1..=300");
    let addr = dest.endpoint_addr();
    ensure!(addr.id == expected, "ticket does not match configured peer");
    let conn = timeout(CONNECT_TIMEOUT, endpoint.connect(addr.clone(), ALPN))
        .await
        .context("connecting to peer timed out")??;
    let (mut tx, mut rx) = timeout(IO_TIMEOUT, conn.open_bi()).await??;
    timeout(IO_TIMEOUT, tx.write_u16(count)).await??;
    for sequence in 1..=count {
        let start = Instant::now();
        timeout(IO_TIMEOUT, async {
            tx.write_u16(sequence).await?;
            ensure!(rx.read_u16().await? == sequence, "incorrect probe reply");
            Ok::<_, anyhow::Error>(())
        })
        .await
        .context("probe reply timeout")??;
        let elapsed = start.elapsed();
        let paths = conn.paths();
        let mut sample = Sample {
            sequence,
            elapsed,
            path: "unknown",
            address: String::new(),
            path_tx_bytes: 0,
            path_rx_bytes: 0,
        };
        if let Some(path) = paths.iter().find(|p| p.is_selected()) {
            sample.path = if path.is_ip() {
                "direct"
            } else if path.is_relay() {
                "relay"
            } else {
                "other"
            };
            sample.address = path.remote_addr().to_string();
            let stats = path.stats();
            sample.path_tx_bytes = stats.udp_tx.bytes;
            sample.path_rx_bytes = stats.udp_rx.bytes;
        }
        report(sample);
        if sequence < count {
            sleep(Duration::from_secs(1)).await;
        }
    }
    tx.finish()?;
    timeout(IO_TIMEOUT, rx.read_to_end(0))
        .await
        .context("waiting for final FIN")??;
    conn.close(0u32.into(), b"probe complete");
    Ok(())
}
