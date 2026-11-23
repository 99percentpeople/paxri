//! A gateway announces itself by admitting its ordinary authenticated shared session.
use super::NativeTransport;
use futures_util::{StreamExt, stream::FuturesUnordered};
use iroh::{EndpointAddr, EndpointId, Watcher};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

impl NativeTransport {
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
        let mut previous = changes.borrow_and_update().clone();
        let mut address = self.endpoint.watch_addr();
        let mut interfaces = self.endpoint.watch_network_interfaces();
        let mut relays = self
            .endpoint
            .home_relay_status()
            .map(|list| list.iter().any(|r| r.is_connected()));
        let mut relay_ready = relays.get();
        let mut local_addr = address.get();
        let mut targets = self.presence_targets();
        let mut queued: BTreeSet<_> = targets.keys().copied().collect();
        let mut active = BTreeSet::new();
        let mut attempts = FuturesUnordered::new();
        loop {
            // One in-flight announcement per identity. Keep a new event that arrives
            // during a dial so we can use a changed network or refreshed peer ticket.
            while attempts.len() < 4 {
                let Some(id) = queued.iter().find(|id| !active.contains(*id)).copied() else {
                    break;
                };
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
                    id
                });
            }

            let announce_all = tokio::select! {
                _ = self.endpoint.closed() => return,
                Some(id) = attempts.next(), if !attempts.is_empty() => {
                    active.remove(&id);
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
                    for (id, addr) in &next {
                        if targets.get(id) != Some(addr) {
                            queued.insert(*id);
                        }
                    }
                    targets = next;
                    false
                }
                event = changes.changed() => {
                    if event.is_err() { return; }
                    let current = changes.borrow_and_update().clone();
                    for (key, time) in current.iter() {
                        if previous.get(key) != Some(time)
                            && let Ok(id) = key.parse()
                            && targets.contains_key(&id) && self.connections.live(id).is_none()
                        {
                            queued.insert(id);
                        }
                    }
                    previous = current;
                    false
                }
            };
            if announce_all {
                queued.extend(targets.keys().copied());
            }
        }
    }
}
