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
