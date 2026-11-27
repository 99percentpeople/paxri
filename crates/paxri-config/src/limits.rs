//! Unified resource limits and optional `[gateway.limits]` configuration.
use serde::{Deserialize, Serialize};

pub const DEFAULT_MAX_PEERS: usize = 64;
pub const MAX_PEERS_CEILING: usize = 256;

pub const DEFAULT_MAX_TERMINAL_SESSIONS: usize = 16;
pub const MAX_TERMINAL_SESSIONS_CEILING: usize = 128;

pub const DEFAULT_MAX_FORWARDS: usize = 64;
pub const MAX_FORWARDS_CEILING: usize = 256;

pub const DEFAULT_MAX_FORWARDS_PER_PEER: usize = 32;
pub const MAX_FORWARDS_PER_PEER_CEILING: usize = MAX_FORWARDS_CEILING;

pub const DEFAULT_MAX_REVERSE_LISTENERS: usize = 64;
pub const MAX_REVERSE_LISTENERS_CEILING: usize = 256;

pub const DEFAULT_MAX_CONCURRENT_STREAMS: usize = 256;
pub const MAX_CONCURRENT_STREAMS_CEILING: usize = 1024;

pub const DEFAULT_MAX_MANAGEMENT_CONNECTIONS: usize = 32;
pub const DEFAULT_MAX_PENDING_PAIRINGS: usize = 32;

/// Optional `[gateway.limits]` quotas loaded from `node.toml`.
///
/// Every key defaults to its `DEFAULT_*` constant and is bounded by a hard safety ceiling.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Limits {
    pub max_peers: usize,
    pub max_terminal_sessions: usize,
    pub max_forwards: usize,
    pub max_forwards_per_peer: usize,
    pub max_reverse_listeners: usize,
    pub max_concurrent_streams: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_peers: DEFAULT_MAX_PEERS,
            max_terminal_sessions: DEFAULT_MAX_TERMINAL_SESSIONS,
            max_forwards: DEFAULT_MAX_FORWARDS,
            max_forwards_per_peer: DEFAULT_MAX_FORWARDS_PER_PEER,
            max_reverse_listeners: DEFAULT_MAX_REVERSE_LISTENERS,
            max_concurrent_streams: DEFAULT_MAX_CONCURRENT_STREAMS,
        }
    }
}

impl Limits {
    pub fn validate(&self) -> Result<(), String> {
        check_range("gateway.limits.max_peers", self.max_peers, MAX_PEERS_CEILING)?;
        check_range(
            "gateway.limits.max_terminal_sessions",
            self.max_terminal_sessions,
            MAX_TERMINAL_SESSIONS_CEILING,
        )?;
        check_range(
            "gateway.limits.max_forwards",
            self.max_forwards,
            MAX_FORWARDS_CEILING,
        )?;
        check_range(
            "gateway.limits.max_forwards_per_peer",
            self.max_forwards_per_peer,
            MAX_FORWARDS_PER_PEER_CEILING,
        )?;
        check_range(
            "gateway.limits.max_reverse_listeners",
            self.max_reverse_listeners,
            MAX_REVERSE_LISTENERS_CEILING,
        )?;
        check_range(
            "gateway.limits.max_concurrent_streams",
            self.max_concurrent_streams,
            MAX_CONCURRENT_STREAMS_CEILING,
        )?;
        Ok(())
    }

    /// Per-peer forward rule cap never exceeds the global forward rule cap.
    pub fn effective_max_forwards_per_peer(&self) -> usize {
        self.max_forwards_per_peer.min(self.max_forwards)
    }
}

fn check_range(key: &str, value: usize, max: usize) -> Result<(), String> {
    if (1..=max).contains(&value) {
        Ok(())
    } else {
        Err(format!("{key} must be between 1 and {max} (got {value})"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_and_bounds_validate() {
        let defaults = Limits::default();
        assert!(defaults.validate().is_ok());
        assert_eq!(defaults.max_peers, DEFAULT_MAX_PEERS);
        assert_eq!(defaults.max_terminal_sessions, DEFAULT_MAX_TERMINAL_SESSIONS);
        assert_eq!(defaults.max_forwards, DEFAULT_MAX_FORWARDS);
        assert_eq!(defaults.effective_max_forwards_per_peer(), DEFAULT_MAX_FORWARDS_PER_PEER);

        let tight = Limits {
            max_forwards: 8,
            ..Limits::default()
        };
        assert!(tight.validate().is_ok());
        assert_eq!(tight.effective_max_forwards_per_peer(), 8);

        for invalid in [
            Limits { max_peers: 0, ..Limits::default() },
            Limits { max_peers: MAX_PEERS_CEILING + 1, ..Limits::default() },
            Limits { max_terminal_sessions: 0, ..Limits::default() },
            Limits { max_terminal_sessions: MAX_TERMINAL_SESSIONS_CEILING + 1, ..Limits::default() },
            Limits { max_forwards: 0, ..Limits::default() },
            Limits { max_forwards: MAX_FORWARDS_CEILING + 1, ..Limits::default() },
            Limits { max_forwards_per_peer: 0, ..Limits::default() },
            Limits { max_forwards_per_peer: MAX_FORWARDS_PER_PEER_CEILING + 1, ..Limits::default() },
            Limits { max_reverse_listeners: 0, ..Limits::default() },
            Limits { max_reverse_listeners: MAX_REVERSE_LISTENERS_CEILING + 1, ..Limits::default() },
            Limits { max_concurrent_streams: 0, ..Limits::default() },
            Limits { max_concurrent_streams: MAX_CONCURRENT_STREAMS_CEILING + 1, ..Limits::default() },
        ] {
            assert!(invalid.validate().is_err());
        }
    }
}
