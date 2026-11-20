//! Opt-in direct IPv4 through a physical LAN router, independent of a VPN's default route.
use anyhow::{Result, ensure};
use crab_nat::{InternetProtocol, PortMapping, PortMappingOptions, TimeoutConfig, natpmp};
use iroh::Endpoint;
use std::{
    collections::BTreeSet,
    net::{Ipv4Addr, SocketAddr, UdpSocket},
    num::NonZeroU16,
    time::{Duration, Instant},
};
use tokio::{
    sync::oneshot,
    task::{JoinHandle, JoinSet},
    time::{sleep, timeout},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct LanRouter {
    pub(super) local: Ipv4Addr,
    gateway: Ipv4Addr,
}

fn retry() -> Option<TimeoutConfig> {
    Some(TimeoutConfig {
        initial_timeout: Duration::from_millis(250),
        max_retries: 2,
        max_retry_timeout: Some(Duration::from_secs(1)),
    })
}

fn candidates() -> BTreeSet<LanRouter> {
    let mut routers = BTreeSet::new();
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
            // on-link router actually leaves via the address our data socket will bind.
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
                routers.insert(LanRouter {
                    local: *local.ip(),
                    gateway,
                });
            }
        }
    }
    routers
}

impl LanRouter {
    pub(super) async fn discover() -> Option<Self> {
        let candidates = tokio::task::spawn_blocking(candidates).await.ok()?;
        let mut probes = JoinSet::new();
        for router in candidates {
            probes.spawn(async move {
                natpmp::external_address(router.gateway.into(), retry())
                    .await
                    .ok()
                    .map(|_| router)
            });
        }
        while let Some(result) = probes.join_next().await {
            if let Ok(Some(router)) = result {
                return Some(router);
            }
        }
        None
    }

    pub(super) fn start(self, ep: &Endpoint) -> Option<LanMapping> {
        let port = ep
            .bound_sockets()
            .into_iter()
            .find(|a| a.ip() == self.local)
            .and_then(|a| NonZeroU16::new(a.port()))?;
        let (stop, stopping) = oneshot::channel();
        let task = tokio::spawn(self.maintain(ep.clone(), port, stopping));
        Some(LanMapping { stop, task })
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

    async fn maintain(self, ep: Endpoint, port: NonZeroU16, mut stop: oneshot::Receiver<()>) {
        let mut current: Option<(PortMapping, SocketAddr)> = None;
        let mut delay = Duration::ZERO;
        let mut failed = false;
        loop {
            tokio::select! {
                biased;
                _ = &mut stop => break,
                _ = sleep(delay) => {}
            }
            let suggested = current
                .as_ref()
                .map_or(port, |(lease, _)| lease.external_port());
            match self.request(port, suggested).await {
                Ok((lease, addr)) => {
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
        }
        if let Some((lease, addr)) = current {
            ep.remove_external_addr(&addr).await;
            if lease.expiration() > Instant::now() {
                let _ = timeout(Duration::from_secs(2), lease.try_drop()).await;
            }
        }
    }
}

pub struct LanMapping {
    stop: oneshot::Sender<()>,
    task: JoinHandle<()>,
}

impl LanMapping {
    pub async fn stop(self) {
        let _ = self.stop.send(());
        let _ = self.task.await;
    }
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
        let lease = LanRouter {
            local: Ipv4Addr::LOCALHOST,
            gateway: router,
        }
        .start(&ep)
        .unwrap();
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
        lease.stop().await;
        assert!(timeout(Duration::from_secs(2), service).await??? >= 2);
        assert!(!ep.addr().ip_addrs().any(|a| *a == second));
        ep.close().await;
        Ok(())
    }
}
