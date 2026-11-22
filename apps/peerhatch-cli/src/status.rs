//! Measure a named peer on demand using the local gateway's connection counters.
use crate::output;
use anyhow::{Result, ensure};
use peerhatch_control::{Command, PeerConnection, Status, TrafficRates, request};
use serde::Serialize;
use std::{
    path::Path,
    time::{Duration, Instant},
};

#[derive(Serialize)]
pub(super) struct MeasuredConnection<'a> {
    #[serde(flatten)]
    pub link: &'a PeerConnection,
    pub rates: Option<TrafficRates>,
}

fn measure(
    link: &PeerConnection,
    previous: &[PeerConnection],
    elapsed: Duration,
) -> Option<TrafficRates> {
    let current = link.traffic?;
    previous
        .iter()
        .filter(|old| {
            old.protocol == link.protocol && old.path == link.path && old.address == link.address
        })
        .find_map(|old| current.rates_since(old.traffic?, elapsed))
}

pub(super) async fn show_peer(
    ipc: &Path,
    status: Status,
    selector: &str,
    check: bool,
    json: bool,
) -> Result<()> {
    let (name, id) = status.resolve_peer(selector)?;
    let checked;
    let first = if check {
        checked = request(
            ipc,
            Command::CheckPeer {
                peer: name.to_owned(),
            },
        )
        .await?;
        &checked
    } else {
        &status
    };
    let previous = first.connections_for(name).unwrap_or_default();
    let start = Instant::now();
    let next;
    let current = if previous.iter().any(|link| link.traffic.is_some()) {
        tokio::time::sleep(Duration::from_secs(1)).await;
        next = request(ipc, Command::Status).await?;
        &next
    } else {
        first
    };
    let elapsed = start.elapsed();
    ensure!(
        current.device_id == status.device_id
            && current.peers.get(name).map(String::as_str) == Some(id),
        "Pairing changed while reading status for {name}"
    );
    let connections: Option<Vec<_>> = current.connections_for(name).map(|links| {
        links
            .iter()
            .map(|link| MeasuredConnection {
                link,
                rates: measure(link, previous, elapsed),
            })
            .collect()
    });
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "peer": name, "device_id": id, "platform": current.peer_platform(name), "connections": connections,
            }))?
        );
    } else {
        print!(
            "{}",
            output::peer(
                current,
                name,
                id,
                connections.as_deref().unwrap_or_default()
            )
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use peerhatch_control::{ConnectionTraffic, PathKind};

    #[test]
    fn rates_match_the_same_route_and_connection_across_snapshots() {
        let link = |address: &str, session, sent_bytes, received_bytes| PeerConnection {
            protocol: "Shared connection".into(),
            streams: Default::default(),
            path: PathKind::Direct,
            address: Some(address.into()),
            rtt_ms: Some(12),
            traffic: Some(ConnectionTraffic {
                connected_at_ms: session,
                sent_bytes,
                received_bytes,
            }),
        };
        let previous = [link("other", 1, 1, 2), link("peer", 1, 100, 200)];
        let elapsed = Duration::from_secs(2);
        let current = link("peer", 1, 600, 400);
        let rates = measure(&current, &previous, elapsed).unwrap();
        assert_eq!(
            (
                rates.upload_bytes_per_second,
                rates.download_bytes_per_second
            ),
            (250, 100)
        );
        assert_eq!(
            measure(&link("peer", 2, 800, 700), &previous, elapsed),
            None
        );
        assert_eq!(
            measure(&link("new route", 1, 800, 700), &previous, elapsed),
            None
        );
        assert_eq!(measure(&link("peer", 1, 50, 700), &previous, elapsed), None);
    }
}
