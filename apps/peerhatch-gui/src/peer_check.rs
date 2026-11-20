use crate::Dashboard;
use gpui::Context;
use std::time::{Duration, Instant};
use peerhatch_control::Command;

const RETRY_DELAY: Duration = Duration::from_secs(30);

pub(crate) enum PeerCheck {
    Pending,
    Finished {
        retry_at: Instant,
        error: Option<String>,
    },
}

impl PeerCheck {
    fn blocks_retry(&self, now: Instant) -> bool {
        match self {
            Self::Pending => true,
            Self::Finished { retry_at, .. } => now < *retry_at,
        }
    }
}

impl Dashboard {
    /// Called by a visible device page as gateway status changes.
    pub(crate) fn check_visible_peer(&mut self, peer: &str, cx: &mut Context<Self>) {
        let Some(status) = &self.status else { return };
        let Some(id) = status.peers.get(peer) else {
            return;
        };
        let Some(paths) = status.connections_for(peer) else {
            return;
        };
        if !paths.is_empty() {
            if matches!(self.peer_checks.get(id), Some(PeerCheck::Finished { .. })) {
                self.peer_checks.remove(id);
            }
            return;
        }

        let now = Instant::now();
        if self
            .peer_checks
            .get(id)
            .is_some_and(|check| check.blocks_retry(now))
        {
            return;
        }
        self.peer_checks.retain(|_, check| check.blocks_retry(now));
        let id = id.clone();
        self.peer_checks.insert(id.clone(), PeerCheck::Pending);

        // A remote dial can take 25 seconds. Give it its own IPC request so
        // gateway polling and edits can proceed while it is in flight.
        let address = self
            .address
            .clone()
            .unwrap_or_else(peerhatch_control::default_address);
        let command = Command::CheckPeer {
            peer: peer.to_owned(),
        };
        let job = self
            .runtime
            .spawn(async move { peerhatch_control::request(&address, command).await.map(|_| ()) });
        cx.spawn(async move |this, cx| {
            let error = match job.await {
                Ok(reply) => reply.err().map(|e| format!("{e:#}")),
                Err(e) => Some(e.to_string()),
            };
            let _ = this.update(cx, |dashboard, cx| {
                let connected = error.is_none();
                dashboard.peer_checks.insert(
                    id,
                    PeerCheck::Finished {
                        retry_at: Instant::now() + RETRY_DELAY,
                        error,
                    },
                );
                if connected {
                    // Read current paths and configuration together after any concurrent edits.
                    dashboard.send(Command::Status, cx);
                }
                cx.notify();
            });
        })
        .detach();
    }
}
