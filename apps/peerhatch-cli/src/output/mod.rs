//! Human-readable reports for the management commands.
mod table;

use table::Report;
use peerhatch_control::{Direction, PairingStatus, PeerConnection, Status};

fn connection_row(link: &PeerConnection) -> [String; 4] {
    let path = link.path.label();
    let path = match link.protocol.as_str() {
        "" | "Shared connection" => path.to_owned(),
        protocol => format!("{path} ({protocol})"),
    };
    let rtt = match link.rtt_ms {
        Some(0) => "<1 ms".into(),
        Some(ms) => format!("{ms} ms"),
        None => "-".into(),
    };
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
            ["PEER", "PATH", "ADDRESS", "RTT", "STREAMS"],
            connections,
        );
    }
    forward_table(&mut report, status);
    report.text
}

pub(super) fn peer(status: &Status, name: &str, id: &str) -> String {
    let mut report = Report::new();
    let paths = status.connections_for(name);
    report.heading("Paired device");
    report.fields(&[
        ("Name", name),
        ("System", status.peer_platform(name).label()),
        ("Device ID", id),
        ("State", status.peer_state(name)),
    ]);
    if let Some(paths) = paths.filter(|p| !p.is_empty()) {
        report.table(
            "Connections",
            ["PATH", "ADDRESS", "RTT", "STREAMS"],
            paths.iter().map(connection_row).collect(),
        );
    }
    report.text
}

fn forward_table(report: &mut Report, status: &Status) {
    let local = status.forwards.iter().map(|(name, rule)| ("This device", name, rule));
    let shared = status.peer_forwards.iter().flat_map(|d| &d.rules)
        .flat_map(|(owner, rules)| rules.iter().map(move |(name, rule)| (owner.as_str(), name, rule)));
    let rows = local.chain(shared)
        .map(|(owner, name, rule)| {
            let direction = match rule.direction {
                Direction::Local => "Forward",
                Direction::Remote => "Reverse",
            };
            [
                name.clone(),
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
        let rows = pairing
            .incoming
            .iter()
            .map(|peer| {
                [
                    peer.name.clone(),
                    peer.platform.label().into(),
                    peer.device_id.clone(),
                ]
            })
            .collect();
        report.table(
            "Incoming pairing requests",
            ["NAME", "SYSTEM", "DEVICE ID"],
            rows,
        );
    }
    if !pairing.pending_unpairs.is_empty() {
        report.line(&format!("Awaiting unpair confirmation: {}", pairing.pending_unpairs.join(", ")));
    }
    if let Some(id) = &pairing.outgoing {
        report.line(&format!("Waiting for approval from {id}"));
    }
    if let Some(message) = &pairing.message {
        report.line(message);
    }
    if report.text.is_empty() {
        report.line("No pairing in progress.");
    }
    report.text
}
