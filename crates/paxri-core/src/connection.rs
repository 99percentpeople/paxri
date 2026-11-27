//! Current application paths reported by a transport to the local gateway.
use crate::StreamKind;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub fn unix_time_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

/// Explicit lifecycle phase of a paired peer's shared transport connection.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnectionPhase {
    #[default]
    Disconnected,
    Initializing,
    Connected,
    Reconnecting,
    Stopping,
}

/// Event driving transitions of [`ConnectionPhase`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConnectionEvent {
    BeginInitialize,
    BeginReconnect,
    Established,
    BeginStop,
    Closed,
}

impl ConnectionPhase {
    pub fn is_connected(self) -> bool {
        matches!(self, Self::Connected)
    }

    pub fn label(self, presence: bool) -> &'static str {
        match self {
            Self::Disconnected if presence => "Offline",
            Self::Disconnected => "Disconnected",
            Self::Initializing => "Connecting…",
            Self::Connected => "Connected",
            Self::Reconnecting => "Reconnecting…",
            Self::Stopping => "Stopping…",
        }
    }

    /// Apply a single transport lifecycle event, returning the next valid phase if allowed.
    pub fn apply(self, event: ConnectionEvent) -> Option<Self> {
        let next = match (self, event) {
            (Self::Stopping, ConnectionEvent::Closed) => Self::Disconnected,
            (Self::Stopping, _) => return None,
            (_, ConnectionEvent::BeginStop) => Self::Stopping,
            (Self::Disconnected, ConnectionEvent::BeginInitialize) => Self::Initializing,
            (
                Self::Disconnected | Self::Initializing | Self::Connected | Self::Reconnecting,
                ConnectionEvent::BeginReconnect,
            ) => Self::Reconnecting,
            (
                Self::Disconnected | Self::Initializing | Self::Connected | Self::Reconnecting,
                ConnectionEvent::Established,
            ) => Self::Connected,
            (_, ConnectionEvent::Closed) => Self::Disconnected,
            _ => return None,
        };
        Some(next)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PeerConnection {
    pub protocol: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connection_id: Option<u64>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub streams: BTreeMap<StreamKind, usize>,
    pub path: PathKind,
    pub address: Option<String>,
    pub rtt_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub traffic: Option<ConnectionTraffic>,
}

/// Transport bytes from the local gateway's perspective since the shared connection opened.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct ConnectionTraffic {
    pub connected_at_ms: i64,
    pub sent_bytes: u64,
    pub received_bytes: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct TrafficRates {
    pub upload_bytes_per_second: u64,
    pub download_bytes_per_second: u64,
}

impl ConnectionTraffic {
    pub fn rates_since(self, previous: Self, elapsed: Duration) -> Option<TrafficRates> {
        if self.connected_at_ms != previous.connected_at_ms || elapsed.is_zero() {
            return None;
        }
        let rate = |bytes: u64| (bytes as f64 / elapsed.as_secs_f64()).round() as u64;
        Some(TrafficRates {
            upload_bytes_per_second: rate(self.sent_bytes.checked_sub(previous.sent_bytes)?),
            download_bytes_per_second: rate(
                self.received_bytes.checked_sub(previous.received_bytes)?,
            ),
        })
    }
}

pub fn format_bytes(bytes: u64) -> String {
    let mut value = bytes as f64;
    for unit in ["B", "KiB", "MiB", "GiB", "TiB", "PiB", "EiB"] {
        if value < 1024. || unit == "EiB" {
            return if unit == "B" {
                format!("{bytes} B")
            } else {
                format!("{value:.1} {unit}")
            };
        }
        value /= 1024.;
    }
    unreachable!()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PathKind {
    Direct,
    Relay,
    Selecting,
    #[serde(other)]
    Other,
}

impl PathKind {
    pub fn label(self) -> &'static str {
        match self {
            PathKind::Direct => "Direct",
            PathKind::Relay => "Relay",
            PathKind::Selecting => "Selecting path",
            PathKind::Other => "Other",
        }
    }
}

/// Label for the current local observation of a peer's authenticated connections.
pub fn peer_state(connections: Option<&[PeerConnection]>, presence: bool) -> &'static str {
    match connections {
        None => "Unavailable",
        Some([]) if presence => "Offline",
        Some([]) => "Disconnected",
        Some(_) => "Connected",
    }
}

impl PeerConnection {
    pub fn rtt_label(&self) -> Option<String> {
        self.rtt_ms.map(|ms| {
            if ms == 0 {
                "<1 ms RTT".into()
            } else {
                format!("{ms} ms RTT")
            }
        })
    }

    pub fn stream_labels(&self) -> impl Iterator<Item = String> + '_ {
        self.stream_labels_matching(|_| true)
    }

    pub fn stream_labels_matching(
        &self,
        include: impl Fn(StreamKind) -> bool,
    ) -> impl Iterator<Item = String> {
        self.streams
            .iter()
            .filter(move |(kind, _)| include(**kind))
            .map(|(kind, count)| {
                if *count == 1 {
                    kind.label().to_owned()
                } else {
                    format!("{} ×{count}", kind.label())
                }
            })
    }

    pub fn description(&self) -> String {
        let mut parts = vec![self.path.label().to_owned()];
        parts.extend(self.address.iter().cloned());
        parts.extend(self.rtt_label());
        parts.extend(self.stream_labels());
        parts.join(" · ")
    }
}

#[cfg(test)]
mod tests {
    use super::{ConnectionEvent, ConnectionPhase};

    #[test]
    fn connection_phase_transitions_enforce_stopping_barrier_and_lifecycle_order() {
        let phase = ConnectionPhase::Disconnected;
        let phase = phase.apply(ConnectionEvent::BeginInitialize).unwrap();
        assert_eq!(phase, ConnectionPhase::Initializing);
        let phase = phase.apply(ConnectionEvent::Established).unwrap();
        assert_eq!(phase, ConnectionPhase::Connected);
        let phase = phase.apply(ConnectionEvent::BeginReconnect).unwrap();
        assert_eq!(phase, ConnectionPhase::Reconnecting);
        let phase = phase.apply(ConnectionEvent::Established).unwrap();
        assert_eq!(phase, ConnectionPhase::Connected);
        let phase = phase.apply(ConnectionEvent::BeginStop).unwrap();
        assert_eq!(phase, ConnectionPhase::Stopping);
        // Stopping blocks new dials or installations until Closed finishes teardown.
        assert_eq!(phase.apply(ConnectionEvent::BeginInitialize), None);
        assert_eq!(phase.apply(ConnectionEvent::Established), None);
        assert_eq!(phase.apply(ConnectionEvent::BeginReconnect), None);
        assert_eq!(
            phase.apply(ConnectionEvent::Closed),
            Some(ConnectionPhase::Disconnected)
        );
    }
}
