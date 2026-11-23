//! A gateway announces itself by admitting its ordinary authenticated shared session.
use super::NativeTransport;
use futures_util::{StreamExt, stream::FuturesUnordered};
use iroh::{EndpointAddr, EndpointId, Watcher};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
    time::Duration,
};
use tokio::time::{Instant, sleep_until};

#[derive(Clone, Copy)]
struct Attempt {
    at: Instant,
    delay: Duration,
}

impl Attempt {
    fn now() -> Self {
        Self {
            at: Instant::now(),
            delay: Duration::ZERO,
        }
    }

    fn retry(self) -> Self {
        let delay = (self.delay * 2).clamp(Duration::from_secs(2), Duration::from_secs(60));
        Self {
            at: Instant::now() + delay,
            delay,
        }
    }
}

impl NativeTransport {
    pub fn refresh_presence(&self) {
        self.presence_refresh.notify_one();
    }

    fn presence_targets(&self) -> BTreeMap<EndpointId, EndpointAddr> {
        let config = self.config.borrow();
        let peers = self.peers.read().expect("peer map");
        config
            .peers
            .keys()
            .filter_map(|name| {
                let addr = peers.get(name)?;
                Some((addr.id, addr.clone()))
            })
            .collect()
    }

    pub async fn announce_presence(self: Arc<Self>) {
        let mut config = self.subscribe_config();
        let mut changes = self.subscribe_peer_changes();
        let mut address = self.endpoint.watch_addr();
        let mut interfaces = self.endpoint.watch_network_interfaces();
        let mut relays = self
            .endpoint
            .home_relay_status()
            .map(|list| list.iter().any(|r| r.is_connected()));
        let mut relay_ready = relays.get();
        let mut local_addr = address.get();
        let mut targets = self.presence_targets();
        let mut queued: BTreeMap<_, _> = targets.keys().map(|&id| (id, Attempt::now())).collect();
        let mut active = BTreeSet::new();
        let mut attempts = FuturesUnordered::new();
        loop {
            let mut wake_at = None;
            // One in-flight announcement per identity. Keep a new event that arrives
            // during a dial so we can use a changed network or refreshed peer ticket.
            while attempts.len() < 4 {
                let Some((&id, &attempt)) = queued
                    .iter()
                    .filter(|(id, _)| !active.contains(*id))
                    .min_by_key(|(_, attempt)| attempt.at)
                else {
                    break;
                };
                if attempt.at > Instant::now() {
                    wake_at = Some(attempt.at);
                    break;
                }
                queued.remove(&id);
                let Some(addr) = targets.get(&id) else {
                    continue;
                };
                if self.connections.live(id).is_some() {
                    continue;
                }
                active.insert(id);
                let transport = &self;
                let addr = addr.clone();
                attempts.push(async move {
                    let _ = transport.connections.get(&transport.endpoint, addr).await;
                    (id, attempt)
                });
            }

            let announce_all = tokio::select! {
                _ = self.endpoint.closed() => return,
                _ = self.presence_refresh.notified() => true,
                _ = async {
                    match wake_at {
                        Some(at) => sleep_until(at).await,
                        None => std::future::pending().await,
                    }
                } => false,
                Some((id, attempt)) = attempts.next(), if !attempts.is_empty() => {
                    active.remove(&id);
                    if targets.contains_key(&id) && self.connections.live(id).is_none() {
                        queued.entry(id).or_insert_with(|| attempt.retry());
                    }
                    false
                }
                event = interfaces.updated() => {
                    if event.is_err() { return; }
                    true
                }
                event = address.updated() => {
                    let Ok(addr) = event else { return };
                    let changed = local_addr != addr;
                    local_addr = addr;
                    changed
                }
                event = relays.updated() => {
                    let Ok(ready) = event else { return };
                    let restored = ready && !relay_ready;
                    relay_ready = ready;
                    restored
                }
                event = config.changed() => {
                    if event.is_err() { return; }
                    let next = self.presence_targets();
                    queued.retain(|id, _| next.contains_key(id));
                    for (id, addr) in &next {
                        if targets.get(id) != Some(addr) {
                            queued.insert(*id, Attempt::now());
                        }
                    }
                    targets = next;
                    false
                }
                event = changes.changed() => {
                    if event.is_err() { return; }
                    for &id in targets.keys() {
                        if self.connections.live(id).is_some() {
                            queued.remove(&id);
                        } else if !active.contains(&id) {
                            // An unrelated peer changing state must not reset this peer's backoff.
                            queued.entry(id).or_insert_with(Attempt::now);
                        }
                    }
                    false
                }
            };
            if announce_all {
                let now = Attempt::now();
                queued.extend(targets.keys().map(|&id| (id, now)));
            }
        }
    }
}
