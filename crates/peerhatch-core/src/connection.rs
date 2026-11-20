//! Current application paths reported by a transport to the local gateway.
use crate::StreamKind;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};

pub fn unix_time_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PeerConnection {
    pub protocol: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub streams: BTreeMap<StreamKind, usize>,
    pub path: PathKind,
    pub address: Option<String>,
    pub rtt_ms: Option<u64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
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
        self.streams.iter().map(|(kind, count)| {
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
