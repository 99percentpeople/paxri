//! Choose a public home relay using the same authenticated transport that carries data.
use anyhow::{Context, Result};
use futures_util::{SinkExt, StreamExt};
use iroh::{Endpoint, RelayMap, RelayMode, RelayUrl, SecretKey, Watcher, dns::DnsResolver};
use iroh_relay::{
    client::ClientBuilder,
    protos::relay::{ClientToRelayMsg, RelayToClientMsg},
    tls::{CaTlsConfig, default_provider},
};
use std::{collections::BTreeMap, time::Duration};
use tokio::{
    task::JoinSet,
    time::{Instant, sleep_until, timeout},
};

const PROBE_TIMEOUT: Duration = Duration::from_secs(4);
const RECHECK: Duration = Duration::from_secs(300);
const RETRY: Duration = Duration::from_secs(10);

async fn measure(url: RelayUrl, dns: DnsResolver, key: SecretKey) -> Result<Duration> {
    let tls = CaTlsConfig::system().client_config(default_provider())?;
    let mut client = ClientBuilder::new(url, key, dns)
        .tls_client_config(tls)
        .connect()
        .await?;
    let mut samples = [Duration::ZERO; 3];
    for (i, sample) in samples.iter_mut().enumerate() {
        let token = (i as u64).to_be_bytes();
        let start = Instant::now();
        client.send(ClientToRelayMsg::Ping(token)).await?;
        loop {
            match client.next().await.context("Relay closed before pong")?? {
                RelayToClientMsg::Pong(data) if data == token => break,
                RelayToClientMsg::Ping(data) => client.send(ClientToRelayMsg::Pong(data)).await?,
                _ => {}
            }
        }
        *sample = start.elapsed();
    }
    samples.sort_unstable();
    Ok(samples[1])
}

async fn latencies(map: &RelayMap, dns: DnsResolver) -> BTreeMap<RelayUrl, Duration> {
    let mut probes = JoinSet::new();
    // A probe must never displace the real gateway's registration at a relay.
    let key = SecretKey::generate();
    for url in map.urls::<Vec<_>>() {
        let dns = dns.clone();
        let key = key.clone();
        probes.spawn(async move {
            let rtt = timeout(PROBE_TIMEOUT, measure(url.clone(), dns, key))
                .await
                .ok()?
                .ok()?;
            Some((url, rtt))
        });
    }
    let mut times = BTreeMap::new();
    while let Some(result) = probes.join_next().await {
        if let Ok(Some((url, duration))) = result {
            times.insert(url, duration);
        }
    }
    times
}

fn fastest(times: &BTreeMap<RelayUrl, Duration>, current: Option<&RelayUrl>) -> Option<RelayUrl> {
    let (best, rtt) = times.iter().min_by_key(|(_, rtt)| *rtt)?;
    if let Some(current) = current
        && times
            .get(current)
            .is_some_and(|old| *old <= *rtt + (*rtt / 10).max(Duration::from_millis(5)))
    {
        return Some(current.clone());
    }
    Some(best.clone())
}

pub(crate) struct PublicRelays {
    candidates: RelayMap,
    selected: Option<RelayUrl>,
}

impl PublicRelays {
    pub(crate) fn discovery(&self) -> iroh::endpoint::NetReportConfig {
        let mut config = iroh::endpoint::NetReportConfig::default();
        config.qad_servers = Some(self.candidates.clone());
        config
    }

    pub(crate) async fn rank() -> Self {
        let candidates = RelayMode::Default.relay_map();
        let times = latencies(&candidates, DnsResolver::new()).await;
        let selected = fastest(&times, None);
        if let Some(url) = &selected {
            eprintln!(
                "Public relay {url} selected ({:.0} ms median ping)",
                times[url].as_secs_f64() * 1000.
            );
        }
        Self {
            candidates,
            selected,
        }
    }

    pub(crate) fn mode(&self) -> RelayMode {
        self.selected
            .clone()
            .map_or(RelayMode::Default, |url| RelayMode::custom([url]))
    }

    pub(crate) fn monitor(mut self, ep: Endpoint) {
        tokio::spawn(async move {
            let mut status = ep.home_relay_status();
            let mut interfaces = ep.watch_network_interfaces();
            let mut reports = ep.net_report();
            let mut last_observations = None;
            loop {
                let connected =
                    |items: &[iroh::endpoint::RelayStatus]| items.iter().any(|r| r.is_connected());
                let mut deadline = Instant::now()
                    + if connected(&status.get()) {
                        RECHECK
                    } else {
                        RETRY
                    };
                loop {
                    tokio::select! {
                        _ = ep.closed() => return,
                        _ = sleep_until(deadline) => break,
                        update = interfaces.updated() => {
                            if update.is_err() { return; }
                            deadline = deadline.min(Instant::now() + Duration::from_secs(1));
                        }
                        update = reports.updated() => {
                            let Ok(report) = update else { return };
                            if let Some(report) = report {
                                let v4 = report.observed_udp_addresses.keys().filter(|ip| ip.is_ipv4()).count();
                                let v6 = report.observed_udp_addresses.len() - v4;
                                let observations = (v4, report.mapping_varies_by_dest_ipv4, v6, report.mapping_varies_by_dest_ipv6);
                                if last_observations != Some(observations) {
                                    let mapping = |v| match v { Some(true) => "varies", Some(false) => "consistent", None => "unknown" };
                                    eprintln!("UDP discovery: IPv4 observers={v4} mapping={}, IPv6 observers={v6} mapping={}", mapping(observations.1), mapping(observations.3));
                                    last_observations = Some(observations);
                                }
                            }
                        }
                        change = status.updated() => match change {
                            Ok(items) if !connected(&items) => deadline = deadline.min(Instant::now() + RETRY),
                            Ok(_) => {},
                            Err(_) => return,
                        },
                    }
                }
                let Ok(dns) = ep.dns_resolver() else { return };
                let times = tokio::select! {
                    _ = ep.closed() => return,
                    times = latencies(&self.candidates, dns.clone()) => times,
                };
                let next = fastest(&times, self.selected.as_ref());
                if next == self.selected {
                    continue;
                }
                // Install alternatives first. When every probe fails, leave all defaults
                // available for iroh's normal reconnect and direct-address discovery.
                for relay in self.candidates.relays::<Vec<_>>() {
                    if next.as_ref().is_none_or(|u| u == &relay.url) {
                        ep.insert_relay(relay.url.clone(), relay).await;
                    }
                }
                if let Some(url) = &next {
                    for old in self.candidates.urls::<Vec<_>>() {
                        if &old != url {
                            ep.remove_relay(&old).await;
                        }
                    }
                    eprintln!(
                        "Public relay {url} selected ({:.0} ms median ping)",
                        times[url].as_secs_f64() * 1000.
                    );
                }
                self.selected = next;
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn prefer_lowest_measured_rtt_and_recover_from_loss() {
        let a: RelayUrl = "https://a.example".parse().unwrap();
        let b: RelayUrl = "https://b.example".parse().unwrap();
        let mut times = BTreeMap::from([
            (a.clone(), Duration::from_millis(195)),
            (b.clone(), Duration::from_millis(150)),
        ]);
        assert_eq!(fastest(&times, Some(&a)), Some(b.clone()));
        times.insert(a.clone(), Duration::from_millis(155));
        assert_eq!(fastest(&times, Some(&a)), Some(a.clone()));
        times.remove(&a);
        assert_eq!(fastest(&times, Some(&a)), Some(b));
        assert_eq!(fastest(&BTreeMap::new(), Some(&a)), None);
    }
    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn independent_udp_observers_share_the_data_socket_and_preserve_home() -> Result<()> {
        use iroh::endpoint::presets;
        use iroh_relay::{
            RelayConfig, RelayQuicConfig,
            server::{self, CertConfig, QuicConfig, Server, ServerConfig, TlsConfig},
        };
        use std::net::{IpAddr, Ipv4Addr, SocketAddr};
        use tokio::net::UdpSocket;
        async fn observer(ip: Ipv4Addr) -> Result<(Server, RelayConfig)> {
            let (_, server_config) = server::testing::self_signed_tls_certs_and_config();
            let mut relay = server::RelayConfig::new((ip, 0));
            relay.tls = Some(TlsConfig::new(
                (ip, 0),
                CertConfig::Manual { server_config },
            ));
            let mut config = ServerConfig::default();
            config.relay = Some(relay);
            config.quic = Some(QuicConfig::new((ip, 0)));
            let server = Server::spawn(config).await?;
            let url = format!("https://{}", server.https_addr().unwrap()).parse()?;
            let qad = Some(RelayQuicConfig::new(server.quic_addr().unwrap().port()));
            Ok((server, RelayConfig::new(url, qad)))
        }
        let home_ip = Ipv4Addr::LOCALHOST;
        let other_ip = Ipv4Addr::new(127, 0, 0, 2);
        let (home_server, home) = observer(home_ip).await?;
        let (other_server, other) = observer(other_ip).await?;
        let blocked = UdpSocket::bind((home_ip, 0)).await?;
        // Translate the source port only for the second destination. The actual QAD
        // exchange and its authenticated address report pass through unchanged.
        let inside = UdpSocket::bind((other_ip, 0)).await?;
        let outside = UdpSocket::bind((home_ip, 0)).await?;
        let translated = outside.local_addr()?;
        let router_port = inside.local_addr()?.port();
        outside.connect(other_server.quic_addr().unwrap()).await?;
        let router = tokio::spawn(async move {
            let mut peer: Option<SocketAddr> = None;
            let mut outgoing = vec![0; 65_535];
            let mut incoming = vec![0; 65_535];
            loop {
                tokio::select! {
                    result = inside.recv_from(&mut outgoing) => {
                        let (len, source) = result.unwrap();
                        peer = Some(source);
                        outside.send(&outgoing[..len]).await.unwrap();
                    }
                    result = outside.recv(&mut incoming) => {
                        let len = result.unwrap();
                        if let Some(peer) = peer { inside.send_to(&incoming[..len], peer).await.unwrap(); }
                    }
                }
            }
        });
        for expected_variation in [Some(false), Some(true), None] {
            let mut primary = home.clone();
            let mut secondary = other.clone();
            match expected_variation {
                None => primary.quic = Some(RelayQuicConfig::new(blocked.local_addr()?.port())),
                Some(true) => secondary.quic = Some(RelayQuicConfig::new(router_port)),
                Some(false) => {}
            }
            let ranked = PublicRelays {
                candidates: RelayMap::from_iter([primary.clone(), secondary]),
                selected: Some(home.url.clone()),
            };
            let ep = Endpoint::builder(presets::Minimal)
                .relay_mode(RelayMode::Custom(RelayMap::from(primary)))
                .net_report_config(ranked.discovery())
                .ca_tls_config(CaTlsConfig::insecure_skip_verify())
                .bind()
                .await?;
            timeout(Duration::from_secs(14), ep.online()).await?;
            let report = timeout(Duration::from_secs(14), ep.net_report().initialized()).await?;
            let observed = &report.observed_udp_addresses;
            let port = ep
                .bound_sockets()
                .into_iter()
                .find(|a| a.is_ipv4())
                .unwrap()
                .port();
            assert_eq!(
                observed.get(&IpAddr::V4(home_ip)).map(SocketAddr::port),
                expected_variation.map(|_| port),
                "{report:?}"
            );
            let secondary = observed[&IpAddr::V4(other_ip)];
            if expected_variation == Some(true) {
                assert_eq!(secondary, translated);
                assert_ne!(secondary.port(), port);
            } else {
                assert_eq!(secondary.port(), port);
            }
            assert_eq!(report.mapping_varies_by_dest_ipv4, expected_variation);
            assert_eq!(report.preferred_relay.as_ref(), Some(&home.url));
            assert_eq!(ep.addr().relay_urls().collect::<Vec<_>>(), vec![&home.url]);
            ep.close().await;
        }
        router.abort();
        home_server.shutdown().await?;
        other_server.shutdown().await?;
        Ok(())
    }

    #[tokio::test]
    async fn disconnected_home_moves_to_the_surviving_relay() -> Result<()> {
        use iroh::endpoint::presets;
        use iroh_relay::server::{RelayConfig, Server, ServerConfig};
        use std::net::Ipv4Addr;

        async fn local() -> Result<(Server, RelayUrl)> {
            let mut config = ServerConfig::default();
            config.relay = Some(RelayConfig::new((Ipv4Addr::LOCALHOST, 0)));
            let server = Server::spawn(config).await?;
            let url = format!("http://{}", server.http_addr().unwrap()).parse()?;
            Ok((server, url))
        }

        let (first, a) = local().await?;
        let (second, b) = local().await?;
        let candidates = RelayMap::from_iter([a.clone(), b.clone()]);
        let times = latencies(&candidates, DnsResolver::new()).await;
        assert_eq!(
            times.len(),
            2,
            "both real relay connections must answer pings"
        );
        let ranked = PublicRelays {
            candidates,
            selected: Some(a.clone()),
        };
        let ep = Endpoint::builder(presets::Minimal)
            .relay_mode(ranked.mode())
            .bind()
            .await?;
        ranked.monitor(ep.clone());
        timeout(Duration::from_secs(8), ep.online()).await?;
        assert!(ep.addr().relay_urls().any(|u| u == &a));
        first.shutdown().await?;
        timeout(Duration::from_secs(40), async {
            let mut changes = ep.watch_addr();
            while !changes.get().relay_urls().any(|u| u == &b) {
                changes.updated().await?;
            }
            Ok::<_, anyhow::Error>(())
        })
        .await??;
        ep.close().await;
        second.shutdown().await?;
        Ok(())
    }
}
