//! Human-readable reports for the management commands.
mod table;

use table::Report;
use paxri_control::{Direction, PairingStatus, PeerConnection, PeerStatus, Status};

fn connection_row(link: &PeerConnection) -> [String; 4] {
    let path = link.path.label();
    let path = match link.protocol.as_str() {
        "" | "Shared connection" => path.to_owned(),
        protocol => format!("{path} ({protocol})"),
    };
    let rtt = link.rtt_ms.map_or_else(|| "-".into(), |ms| ms.to_string());
    let streams = link.stream_labels().collect::<Vec<_>>().join(", ");
    [
        path,
        link.address.as_deref().unwrap_or("-").into(),
        rtt,
        if streams.is_empty() {
            "-".into()
        } else {
            streams
        },
    ]
}

pub(super) fn status(status: &Status) -> String {
    let mut report = Report::new();
    report.heading("Gateway");
    let mut fields = vec![
        ("Name", status.display_name()),
        ("Device ID", &status.device_id),
    ];
    let config = status
        .config_path
        .as_deref()
        .map(|path| path.to_string_lossy());
    if let Some(path) = config.as_deref() {
        fields.push(("Config", path));
    }
    report.fields(&fields);

    let mut peers = Vec::new();
    let mut connections = Vec::new();
    let mut short_ids = status.short_peer_ids();
    for (name, _) in status.peers_by_last_change() {
        let paths = status.connections_for(name);
        peers.push([
            name.clone(),
            status.peer_platform(name).label().into(),
            status.peer_state(name).into(),
            short_ids
                .remove(name.as_str())
                .unwrap_or_else(|| "-".into()),
        ]);
        for link in paths.unwrap_or_default() {
            let [path, address, rtt, streams] = connection_row(link);
            connections.push([name.clone(), path, address, rtt, streams]);
        }
    }
    report.table(
        "Paired devices",
        ["NAME", "SYSTEM", "STATE", "SHORT ID"],
        peers,
    );
    if !connections.is_empty() {
        report.table(
            "Connections",
            ["PEER", "PATH", "ADDRESS", "RTT (ms)", "STREAMS"],
            connections,
        );
    }
    forward_table(&mut report, status);
    report.text
}

pub(super) fn peer(snapshot: &PeerStatus) -> String {
    let mut report = Report::new();
    report.heading("Paired device");
    report.fields(&[
        ("Name", &snapshot.peer),
        ("System", snapshot.platform.label()),
        ("Device ID", &snapshot.device_id),
        ("State", &snapshot.state),
    ]);
    if let Some(paths) = snapshot.connections.as_ref().filter(|links| !links.is_empty()) {
        let rows = paths.iter()
            .map(|link| {
                let (sent, received) = match link.traffic {
                    Some(t) => (t.sent_bytes.to_string(), t.received_bytes.to_string()),
                    None => ("-".into(), "-".into()),
                };
                let [path, address, rtt, streams] = connection_row(link);
                [path, address, sent, received, rtt, streams]
            })
            .collect();
        report.table(
            "Connections",
            ["PATH", "ADDRESS", "SENT (B)", "RECEIVED (B)", "RTT (ms)", "STREAMS"],
            rows,
        );
    }
    report.text
}

fn forward_table(report: &mut Report, status: &Status) {
    let local = status.forwards.iter().map(|(id, rule)| ("This device", id, rule));
    let shared = status.peer_forwards.iter().flat_map(|d| &d.rules)
        .flat_map(|(owner, rules)| rules.iter().map(move |(id, rule)| (owner.as_str(), id, rule)));
    let rows = local.chain(shared)
        .map(|(owner, id, rule)| {
            let direction = match rule.direction {
                Direction::Local => "Forward",
                Direction::Remote => "Reverse",
            };
            [
                id.clone(),
                rule.name.as_deref().unwrap_or("-").into(),
                owner.to_owned(),
                direction.into(),
                rule.peer.clone(),
                rule.bound.as_ref().unwrap_or(&rule.listen).clone(),
                rule.target.clone(),
                rule.state().into(),
            ]
        })
        .collect();
    report.table(
        "Forwarding rules",
        [
            "ID",
            "NAME",
            "OWNER",
            "DIRECTION",
            "PEER",
            "LISTEN",
            "DESTINATION",
            "STATE",
        ],
        rows,
    );
}

pub(super) fn forwards(status: &Status) -> String {
    let mut report = Report::new();
    forward_table(&mut report, status);
    if let Some(directory) = &status.peer_forwards {
        for (peer, error) in &directory.errors {
            report.line(&format!("{peer}: {error}"));
        }
    }
    report.text
}

pub(super) fn pairing(pairing: &PairingStatus) -> String {
    let mut report = Report::new();
    if !pairing.incoming.is_empty() {
        let short_ids = paxri_config::peer_id::unique_prefixes(
            pairing.incoming.iter().map(|peer| peer.device_id.as_str()),
        );
        let rows = pairing
            .incoming
            .iter()
            .map(|peer| {
                let short = short_ids
                    .get(&peer.device_id.to_ascii_lowercase())
                    .cloned()
                    .unwrap_or_else(|| {
                        paxri_config::peer_id::default_prefix(&peer.device_id).to_owned()
                    });
                [peer.name.clone(), peer.platform.label().into(), short]
            })
            .collect();
        report.table(
            "Incoming pairing requests",
            ["NAME", "SYSTEM", "SHORT ID"],
            rows,
        );
    }
    if !pairing.pending_unpairs.is_empty() {
        report.line(&format!("Awaiting unpair confirmation: {}", pairing.pending_unpairs.join(", ")));
    }
    if let Some(id) = &pairing.outgoing {
        report.line(&format!(
            "Waiting for approval from {}",
            paxri_config::peer_id::default_prefix(id)
        ));
    }
    if let Some(message) = &pairing.message {
        report.line(message);
    }
    if report.text.is_empty() {
        report.line("No pairing in progress.");
    }
    report.text
}
