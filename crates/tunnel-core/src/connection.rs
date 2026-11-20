//! Current application paths reported by a transport to the local gateway.
use crate::StreamKind;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

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

impl PeerConnection {
    pub fn description(&self) -> String {
        let path = match self.path {
            PathKind::Direct => "Direct",
            PathKind::Relay => "Relay",
            PathKind::Selecting => "Selecting path",
            PathKind::Other => "Other",
        };
        let mut parts = vec![path.to_owned()];
        if let Some(address) = &self.address {
            parts.push(address.clone());
        }
        if let Some(ms) = self.rtt_ms {
            parts.push(if ms == 0 {
                "<1 ms RTT".into()
            } else {
                format!("{ms} ms RTT")
            });
        }
        for (kind, count) in &self.streams {
            parts.push(if *count == 1 {
                kind.label().to_owned()
            } else {
                format!("{} ×{count}", kind.label())
            });
        }
        parts.join(" · ")
    }
}
