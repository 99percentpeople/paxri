//! Opt-in direct IPv4 through a physical LAN router, independent of a VPN's default route.
use anyhow::{Result, ensure};
use crab_nat::{InternetProtocol, PortMapping, PortMappingOptions, TimeoutConfig, natpmp};
use iroh::{Endpoint, Watcher};
use std::{
    collections::BTreeMap,
    net::{Ipv4Addr, SocketAddr, UdpSocket},
    num::NonZeroU16,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::{
    sync::{Notify, oneshot, watch},
    task::{JoinHandle, JoinSet},
    time::{sleep, timeout},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct LanRouter {
    pub(super) local: Ipv4Addr,
    gateway: Ipv4Addr,
    interface: u32,
    gateway_mac: [u8; 6],
}

fn retry() -> Option<TimeoutConfig> {
    Some(TimeoutConfig {
        initial_timeout: Duration::from_millis(250),
        max_retries: 2,
        max_retry_timeout: Some(Duration::from_secs(1)),
    })
}

fn candidates() -> BTreeMap<LanRouter, bool> {
    let mut routers = BTreeMap::new();
    for interface in netdev::get_interfaces() {
        if !interface.is_up()
            || interface.is_loopback()
            || interface.is_point_to_point()
            || interface.is_tun()
        {
            continue;
        }
        let Some(gateways) = interface.gateway else {
            continue;
        };
        for gateway in gateways.ipv4 {
            // NAT-PMP maps to the requester's source. Verify that a request to this
            // on-link router actually leaves via the address used by our data socket.
            let source = || -> std::io::Result<SocketAddr> {
                let sock = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0))?;
                sock.connect((gateway, 5351))?;
                sock.local_addr()
            };
            if let Ok(SocketAddr::V4(local)) = source()
                && interface
                    .ipv4
                    .iter()
                    .any(|net| net.addr() == *local.ip() && net.contains(&gateway))
            {
                routers.insert(
                    LanRouter {
                        local: *local.ip(),
                        gateway,
                        interface: interface.index,
                        gateway_mac: gateways.mac_addr.octets(),
                    },
                    interface.default,
                );
            }
        }
    }
    routers
}

impl LanRouter {
    async fn discover(routers: &BTreeMap<Self, bool>, previous: Option<Self>) -> Option<Self> {
        let mut probes = JoinSet::new();
        for (&router, &preferred) in routers {
            probes.spawn(async move {
                natpmp::external_address(router.gateway.into(), retry())
                    .await
                    .ok()?;
                Some((!preferred, Some(router) != previous, router))
            });
        }
        // Prefer the current system route, then keep an already-working interface.
        let mut best = None;
        while let Some(result) = probes.join_next().await {
            if let Ok(Some(rank)) = result {
                best = Some(best.map_or(rank, |old| rank.min(old)));
            }
        }
        best.map(|(_, _, router)| router)
    }

    async fn request(
        self,
        port: NonZeroU16,
        suggested: NonZeroU16,
    ) -> Result<(PortMapping, SocketAddr)> {
        let router = self.gateway.into();
        let ip = natpmp::external_address(router, retry()).await?;
        ensure!(
            !ip.is_unspecified() && !ip.is_multicast() && !ip.is_loopback() && !ip.is_broadcast(),
            "Invalid router address {ip}"
        );
        let lease = natpmp::port_mapping(
            router,
            InternetProtocol::Udp,
            port,
            PortMappingOptions {
                external_port: Some(suggested),
                lifetime_seconds: Some(600),
                timeout_config: retry(),
            },
        )
        .await?;
        ensure!(lease.lifetime() > 0, "Router did not grant a lease");
        let address = SocketAddr::new(ip.into(), lease.external_port().get());
        Ok((lease, address))
    }

    async fn maintain(
        self,
        ep: Endpoint,
        port: NonZeroU16,
        mut stop: oneshot::Receiver<bool>,
        refresh: Arc<Notify>,
        expires: watch::Sender<Option<Instant>>,
    ) {
        let mut current: Option<(PortMapping, SocketAddr)> = None;
        let mut delay = Duration::ZERO;
        let mut failed = false;
        let release = loop {
            tokio::select! {
                biased;
                reason = &mut stop => break reason.unwrap_or(false),
                _ = refresh.notified() => {},
                _ = sleep(delay) => {}
            }
            let suggested = current
                .as_ref()
                .map_or(port, |(lease, _)| lease.external_port());
            match self.request(port, suggested).await {
                Ok((lease, addr)) => {
                    expires.send_replace(Some(lease.expiration()));
                    if current.as_ref().is_none_or(|(_, old)| *old != addr) {
                        ep.add_external_addr(addr).await;
                        if let Some((_, old)) = &current {
                            ep.remove_external_addr(old).await;
                        }
                        eprintln!(
                            "Direct UDP mapping {addr} -> {}:{port} via {}",
                            self.local, self.gateway
                        );
                    }
                    delay = Duration::from_secs(u64::from(lease.lifetime())) / 2;
                    current = Some((lease, addr));
                    failed = false;
                }
                Err(err) => {
                    if !failed {
                        eprintln!("Direct UDP mapping via {}: {err:#}", self.gateway);
                    }
                    failed = true;
                    delay = Duration::from_secs(10);
                    if let Some((lease, addr)) = &current {
                        let remaining =
                            lease.expiration().saturating_duration_since(Instant::now());
                        if remaining.is_zero() {
                            ep.remove_external_addr(addr).await;
                            current = None;
                        } else {
                            delay = delay.min(remaining);
                        }
                    }
                }
            }
        };
        if let Some((lease, addr)) = current {
            ep.remove_external_addr(&addr).await;
            if release && lease.expiration() > Instant::now() {
                let _ = timeout(Duration::from_secs(2), lease.try_drop()).await;
            }
        }
    }
}

struct MappingWorker {
    router: Option<LanRouter>,
    port: NonZeroU16,
    refresh: Arc<Notify>,
    expires: watch::Receiver<Option<Instant>>,
    stop: oneshot::Sender<bool>,
    task: JoinHandle<()>,
}

impl MappingWorker {
    async fn start(ep: &Endpoint, router: Option<LanRouter>, port: NonZeroU16, upnp: bool) -> Self {
        let ep = ep.clone();
        let (stop, stopping) = oneshot::channel();
        let refresh = Arc::new(Notify::new());
        let update = refresh.clone();
        let (expiry, expires) = watch::channel(None);
        ep.set_ipv4_source(router.map(|r| r.local)).await;
        let task = tokio::spawn(async move {
            match router {
                Some(router) => router.maintain(ep, port, stopping, update, expiry).await,
                None => default_mapping(ep, port, stopping, update, upnp).await,
            }
        });
        Self {
            router,
            port,
            refresh,
            expires,
            stop,
            task,
        }
    }

    async fn finish(self, release: bool) {
        let _ = self.stop.send(release);
        let _ = self.task.await;
    }
}

// Preserve iroh's normal PCP/UPnP/NAT-PMP fallback on networks without a physical
// NAT-PMP router. Only one worker ever publishes or renews a lease on this port.
async fn default_mapping(
    ep: Endpoint,
    port: NonZeroU16,
    mut stop: oneshot::Receiver<bool>,
    refresh: Arc<Notify>,
    upnp: bool,
) {
    let client = portmapper::Client::new(crate::mapping_protocols(upnp));
    let mut addresses = client.watch_external_address();
    client.update_local_port(port);
    let mut advertised = None;
    let release = loop {
        tokio::select! {
            biased;
            reason = &mut stop => break reason.unwrap_or(false),
            _ = refresh.notified() => client.procure_mapping(),
            result = addresses.changed() => {
                if result.is_err() { break false; }
                let next = (*addresses.borrow_and_update()).map(SocketAddr::V4);
                if advertised != next {
                    if let Some(addr) = next { ep.add_external_addr(addr).await; }
                    if let Some(old) = advertised { ep.remove_external_addr(&old).await; }
                    advertised = next;
                }
            }
        }
    };
    if let Some(old) = advertised {
        ep.remove_external_addr(&old).await;
    }
    if release {
        client.deactivate();
        // The actor processes this after the request to release its old mapping.
        if advertised.is_some() {
            let _ = timeout(Duration::from_secs(2), client.probe()).await;
        }
    }
}

pub struct LanMapping {
    stop: oneshot::Sender<()>,
    task: JoinHandle<()>,
}

impl LanMapping {
    pub(super) fn start(ep: Endpoint, upnp: bool) -> Self {
        let (stop, stopping) = oneshot::channel();
        Self {
            stop,
            task: tokio::spawn(Self::supervise(ep, stopping, upnp)),
        }
    }

    async fn supervise(ep: Endpoint, mut stop: oneshot::Receiver<()>, upnp: bool) {
        let mut interfaces = ep.watch_network_interfaces();
        let mut active: Option<MappingWorker> = None;
        let mut notified = true;
        let mut previous_network = None;
        loop {
            let routers = match tokio::task::spawn_blocking(candidates).await {
                Ok(routers) => routers,
                Err(_) => break,
            };
            let network = (routers.clone(), interfaces.get().default_route_interface);
            let changed = previous_network
                .replace(network.clone())
                .is_some_and(|old| old != network);
            let previous = active.as_ref().and_then(|worker| worker.router);
            // Do not send a delete to the same private gateway address on a new LAN.
            if previous.is_some_and(|r| !routers.contains_key(&r)) {
                ep.set_ipv4_source(None).await;
                if let Some(old) = active.take() {
                    old.finish(false).await;
                }
            }
            let router = tokio::select! {
                biased;
                _ = &mut stop => break,
                _ = ep.closed() => break,
                router = LanRouter::discover(&routers, previous) => router,
            };
            // A missed discovery reply does not invalidate a lease on the same LAN.
            let router = router.or_else(|| {
                active.as_ref().and_then(|worker| {
                    worker.router.filter(|_| {
                        worker
                            .expires
                            .borrow()
                            .is_some_and(|end| end > Instant::now())
                    })
                })
            });
            let port = ep
                .bound_sockets()
                .iter()
                .find(|addr| addr.is_ipv4())
                .and_then(|addr| NonZeroU16::new(addr.port()));
            let same = active.as_ref().is_some_and(|worker| {
                worker.router == router
                    && Some(worker.port) == port
                    && !(changed && router.is_none())
            });
            if same {
                if let Some(worker) = &active
                    && (notified || router.is_none())
                {
                    worker.refresh.notify_one();
                }
            } else {
                if let Some(old) = active.take() {
                    old.finish(old_route_reachable(previous, &routers, changed))
                        .await;
                }
                if let Some(port) = port {
                    active = Some(MappingWorker::start(&ep, router, port, upnp).await);
                } else {
                    ep.set_ipv4_source(None).await;
                }
            }
            // Some platforms coalesce route changes. Retry discovery even when a
            // router only becomes available after its interface is already up.
            notified = tokio::select! {
                biased;
                _ = &mut stop => break,
                _ = ep.closed() => break,
                update = interfaces.updated() => { if update.is_err() { break; } true },
                _ = sleep(Duration::from_secs(30)) => false,
            };
        }
        if let Some(old) = active {
            let present = tokio::task::spawn_blocking(candidates)
                .await
                .unwrap_or_default();
            let release = old_route_reachable(old.router, &present, false);
            old.finish(release).await;
        }
        ep.set_ipv4_source(None).await;
    }

    pub async fn stop(self) {
        let _ = self.stop.send(());
        let _ = self.task.await;
    }
}

fn old_route_reachable(
    old: Option<LanRouter>,
    routers: &BTreeMap<LanRouter, bool>,
    changed: bool,
) -> bool {
    old.map_or(!changed, |r| routers.contains_key(&r))
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;

    // A loopback NAT-PMP router grants short leases and changes the external port
    // on renewal. Exercise the same advertiser and release path as the gateway.
    #[tokio::test]
    async fn renews_replaces_and_releases_the_advertised_address() -> Result<()> {
        let router: Ipv4Addr = "127.0.0.71".parse()?;
        let external: Ipv4Addr = "192.0.2.34".parse()?;
        let socket = tokio::net::UdpSocket::bind((router, 5351)).await?;
        let service = tokio::spawn(async move {
            let mut packet = [0u8; 64];
            let mut grants = 0;
            loop {
                let (n, from) = socket.recv_from(&mut packet).await?;
                let mut response = vec![0, packet[1] | 128, 0, 0, 0, 0, 0, 1];
                let release = match (packet[1], n) {
                    (0, 2) => {
                        response.extend(external.octets());
                        false
                    }
                    (1, 12) => {
                        assert_ne!(&packet[4..6], &[0, 0]);
                        let release = packet[8..12] == [0; 4];
                        grants += u16::from(!release);
                        response.extend(&packet[4..6]);
                        response.extend((45100 + grants).to_be_bytes());
                        response.extend(if release { 0u32 } else { 2 }.to_be_bytes());
                        release
                    }
                    _ => anyhow::bail!("unexpected mapping request"),
                };
                socket.send_to(&response, from).await?;
                if release {
                    return Ok::<_, anyhow::Error>(grants);
                }
            }
        });
        let ep = Endpoint::builder(iroh::endpoint::presets::Minimal)
            .bind_addr((Ipv4Addr::LOCALHOST, 0))?
            .relay_mode(iroh::RelayMode::Disabled)
            .portmapper_config(iroh::endpoint::PortmapperConfig::Disabled)
            .bind()
            .await?;
        let router = LanRouter {
            local: Ipv4Addr::LOCALHOST,
            gateway: router,
            interface: 1,
            gateway_mac: [0; 6],
        };
        let port = NonZeroU16::new(
            ep.bound_sockets()
                .into_iter()
                .find(|a| a.is_ipv4())
                .unwrap()
                .port(),
        )
        .unwrap();
        let lease = MappingWorker::start(&ep, Some(router), port, false).await;
        let first: SocketAddr = (external, 45101).into();
        let second: SocketAddr = (external, 45102).into();
        for addr in [first, second] {
            timeout(Duration::from_secs(8), async {
                while !ep.addr().ip_addrs().any(|a| *a == addr) {
                    sleep(Duration::from_millis(20)).await;
                }
            })
            .await?;
        }
        assert!(!ep.addr().ip_addrs().any(|a| *a == first));
        lease.finish(true).await;
        assert!(timeout(Duration::from_secs(2), service).await??? >= 2);
        assert!(!ep.addr().ip_addrs().any(|a| *a == second));
        ep.close().await;
        Ok(())
    }
}
