use super::{Gateway, settings::PeerUpdate};
use anyhow::{Context, Result, ensure};
use paxri_control::{PairingPeer, PairingStatus};
use paxri_native::{
    EndpointTicket,
    gateway::pairing::{Contact, Decision, Invitation, JoinRequest},
};
use std::{
    collections::BTreeMap,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::{
    sync::{Mutex, oneshot},
    task::AbortHandle,
};

pub(super) fn random_id() -> Result<String> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).map_err(|e| anyhow::anyhow!("Generate pairing identifier: {e}"))?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

struct Pending {
    request_id: String,
    peer: Contact,
    replies: Vec<oneshot::Sender<Decision>>,
}

struct Outgoing {
    id: String,
    request_id: String,
    task: AbortHandle,
}

#[derive(Default)]
pub(super) struct Pairings {
    token: Option<String>,
    pending: BTreeMap<String, Pending>,
    // A reconnect of the same request can recover its reply without another prompt.
    replies: BTreeMap<(String, String), (Instant, Decision)>,
    outgoing: Option<Outgoing>,
    pub(super) message: Option<String>,
}

impl Pairings {
    fn prune(&mut self) {
        self.pending.retain(|_, p| {
            p.replies.retain(|r| !r.is_closed());
            !p.replies.is_empty()
        });
        self.replies.retain(|_, (until, _)| *until > Instant::now());
    }

    pub(super) fn status(&mut self, config: &paxri_config::Config) -> PairingStatus {
        self.prune();
        PairingStatus {
            pending_unpairs: config
                .pending_unpairs
                .values()
                .map(|p| p.name.clone())
                .collect(),
            code: config
                .pairing_code
                .as_deref()
                .filter(|_| self.token.is_some())
                .map(str::to_owned),
            incoming: self
                .pending
                .iter()
                .map(|(id, p)| PairingPeer {
                    request_id: p.request_id.clone(),
                    name: p.peer.name.clone(),
                    device_id: id.clone(),
                    platform: p.peer.platform,
                })
                .collect(),
            outgoing: self.outgoing.as_ref().map(|o| o.id.clone()),
            message: self.message.clone(),
        }
    }

    pub(super) fn receive(&mut self, incoming: JoinRequest) {
        self.prune();
        let JoinRequest {
            token,
            peer,
            reply,
            request_id,
        } = incoming;
        if self.token.as_deref() != Some(&token) {
            let _ = reply.send(Decision::Rejected(
                "Invalid pairing code. Copy the current code from the other device.".into(),
            ));
            return;
        }
        let id = peer.id().to_string();
        if let Some((_, decision)) = self.replies.get(&(id.clone(), request_id.clone())) {
            let _ = reply.send(decision.clone());
            return;
        }
        let pending = self.pending.entry(id).or_insert_with(|| Pending {
            request_id: request_id.clone(),
            peer,
            replies: Vec::new(),
        });
        if pending.request_id == request_id {
            pending.replies.push(reply);
        } else {
            let _ = reply.send(Decision::Rejected(
                "A request from this device is already pending".into(),
            ));
        }
    }

    pub(super) fn forget(&mut self, id: &str) {
        if let Some(request) = self.pending.remove(id) {
            for reply in request.replies {
                let _ = reply.send(Decision::Rejected("Device unpaired".into()));
            }
        }
        self.replies.retain(|(peer, _), _| peer != id);
        if self
            .outgoing
            .as_ref()
            .is_some_and(|request| request.id == id)
        {
            let _ = self.cancel();
        }
    }

    pub(super) fn cancel(&mut self) -> Result<()> {
        self.outgoing
            .take()
            .context("No outgoing pairing request")?
            .task
            .abort();
        self.message = Some("Pairing request canceled.".into());
        Ok(())
    }

    fn resolve_pending(
        &self,
        selector: &str,
        request_id: Option<&str>,
    ) -> Result<(&str, &Pending)> {
        let (id, pending) =
            paxri_config::peer_id::resolve_id(&self.pending, selector, |p| &p.peer.name)?;
        ensure!(
            request_id.is_none_or(|r| r == pending.request_id),
            "This pairing request is no longer pending"
        );
        Ok((id.as_str(), pending))
    }
}

impl Gateway {
    pub(super) async fn init_pairing(gateway: &Mutex<Self>) -> Result<()> {
        let (saved_code, transport_id, transport, direct_only) = {
            let g = gateway.lock().await;
            (
                g.config.pairing_code.clone(),
                g.transport.id(),
                g.transport.clone(),
                g.direct_only,
            )
        };
        if let Some(code) = saved_code {
            let saved = Invitation::parse(&code)?;
            ensure!(
                saved.ticket.endpoint_addr().id.to_string() == transport_id,
                "Saved pairing code belongs to another device. Refresh the code."
            );
            gateway.lock().await.pairing.token = Some(saved.token);
            return Ok(());
        }
        let ticket = transport.initial_pairing_ticket(direct_only).await?;
        Self::refresh_pairing(gateway, &ticket).await
    }

    pub(super) async fn refresh_pairing(
        gateway: &Mutex<Self>,
        ticket: &EndpointTicket,
    ) -> Result<()> {
        let token = random_id()?;
        let code = Invitation::encode(&ticket.to_string(), &token);
        let has_addr = !ticket.endpoint_addr().is_empty();
        Self::update_config(
            gateway,
            |g| {
                ensure!(
                    !g.direct_only || has_addr,
                    "No address available for the pairing code"
                );
                let (text, config) = g.file.prepare_pairing_code(&code)?;
                Ok(Some((text, config, token)))
            },
            |g, token| {
                for (_, p) in std::mem::take(&mut g.pairing.pending) {
                    for reply in p.replies {
                        let _ =
                            reply.send(Decision::Rejected("The pairing code was refreshed".into()));
                    }
                }
                g.pairing.replies.clear();
                g.pairing.token = Some(token);
                g.pairing.message = None;
            },
        )
        .await?;
        Ok(())
    }

    pub(super) async fn update_peer_ticket(
        gateway: &Mutex<Self>,
        ticket: EndpointTicket,
    ) -> Result<()> {
        let id = ticket.endpoint_addr().id.to_string();
        let text = ticket.to_string();
        Self::update_config(
            gateway,
            |g| {
                let Some((name, peer)) = g
                    .config
                    .peers
                    .iter()
                    .find(|(_, p)| p.endpoint_id.eq_ignore_ascii_case(&id))
                else {
                    return Ok(None);
                };
                if peer.ticket.as_ref() == Some(&text) {
                    return Ok(None);
                }
                let name = name.clone();
                let (updated, config) =
                    g.file
                        .prepare_peer(&name, &id, &text, None, PeerUpdate::Address)?;
                Ok(Some((updated, config, (name, ticket))))
            },
            |g, (name, ticket)| {
                g.transport.set_peer(name, ticket);
            },
        )
        .await?;
        Ok(())
    }

    fn contact(&self) -> Contact {
        self.transport.contact(crate::effective_device_name(&self.config))
    }

    async fn save_peer(
        gateway: &Mutex<Self>,
        peer: &Contact,
        request_id: &str,
    ) -> Result<String> {
        let id = peer.id().to_string();
        let ticket_text = peer.ticket.to_string();
        let peer_clone = peer.clone();
        let display = Self::update_config(
            gateway,
            |g| {
                ensure!(id != g.transport.id(), "Cannot pair with this device");
                let peers = &g.config.peers;
                let alias = if let Some((name, _)) = peers
                    .iter()
                    .find(|(_, p)| p.endpoint_id.eq_ignore_ascii_case(&id))
                {
                    name.clone()
                } else {
                    let name = peer_clone.name.trim();
                    let mut alias = name.to_owned();
                    for suffix in 2.. {
                        if !peers.contains_key(&alias) {
                            break;
                        }
                        alias = format!("{name}-{suffix}");
                    }
                    alias
                };
                let (text, config) = g.file.prepare_peer(
                    &alias,
                    &id,
                    &ticket_text,
                    Some(peer_clone.platform),
                    PeerUpdate::Pair(Some(request_id)),
                )?;
                Ok(Some((text, config, (alias, peer_clone))))
            },
            |g, (alias, peer)| {
                g.transport.set_peer(alias.clone(), peer.ticket.clone());
                g.transport.remember_contact(&peer);
                g.peer_device_name(&g.config.peers[&alias])
                    .unwrap_or(alias)
            },
        )
        .await?
        .context("Failed to persist paired device")?;
        Ok(display)
    }

    pub(super) async fn answer_pairing(
        gateway: &Mutex<Self>,
        id: &str,
        request_id: Option<&str>,
        accept: bool,
    ) -> Result<()> {
        let (resolved_id, peer, generation, local_contact) = {
            let mut g = gateway.lock().await;
            g.pairing.prune();
            let (resolved_id, pending) = g.pairing.resolve_pending(id, request_id)?;
            (
                resolved_id.to_owned(),
                pending.peer.clone(),
                pending.request_id.clone(),
                g.contact(),
            )
        };
        let (decision, message) = if accept {
            let name = Self::save_peer(gateway, &peer, &generation).await?;
            (
                Decision::Accepted(local_contact),
                format!("Paired with {name}."),
            )
        } else {
            (
                Decision::Rejected("Pairing request declined".into()),
                format!("Declined pairing with {}.", peer.name),
            )
        };
        let mut g = gateway.lock().await;
        if let Some(pending) = g.pairing.pending.remove(&resolved_id) {
            for reply in pending.replies {
                let _ = reply.send(decision.clone());
            }
            g.pairing.replies.insert(
                (resolved_id, pending.request_id),
                (Instant::now() + Duration::from_secs(90), decision),
            );
        }
        g.pairing.message = Some(message);
        Ok(())
    }

    pub(super) fn join_pairing(&mut self, code: &str, gateway: &Arc<Mutex<Self>>) -> Result<()> {
        ensure!(
            self.pairing.outgoing.is_none(),
            "Already waiting for a pairing request"
        );
        let invite = Invitation::parse(code)?;
        let id = invite.ticket.endpoint_addr().id.to_string();
        ensure!(id != self.transport.id(), "Cannot pair with this device");
        let local = self.contact();
        let transport = self.transport.clone();
        let gateway = Arc::downgrade(gateway);
        let request_id = random_id()?;
        let attempt = request_id.clone();
        let task = tokio::spawn(async move {
            let mut result = transport.request_pairing(&invite, &local, &attempt).await;
            if result.is_err() {
                result = transport.request_pairing(&invite, &local, &attempt).await;
            }
            let Some(gateway) = gateway.upgrade() else {
                return;
            };
            let still_current = gateway
                .lock()
                .await
                .pairing
                .outgoing
                .as_ref()
                .is_some_and(|o| o.request_id == attempt);
            if !still_current {
                return;
            }
            let saved = match result {
                Ok(Decision::Accepted(peer)) => Self::save_peer(&gateway, &peer, &attempt).await,
                Ok(Decision::Rejected(reason)) => Err(anyhow::anyhow!("{reason}")),
                Err(e) => Err(e),
            };
            let mut g = gateway.lock().await;
            if !g
                .pairing
                .outgoing
                .as_ref()
                .is_some_and(|o| o.request_id == attempt)
            {
                return;
            }
            g.pairing.outgoing = None;
            g.pairing.message = Some(match saved {
                Ok(name) => format!("Paired with {name}."),
                Err(e) => format!("Pairing failed: {e:#}"),
            });
        });
        self.pairing.outgoing = Some(Outgoing {
            id,
            request_id,
            task: task.abort_handle(),
        });
        self.pairing.message = None;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use paxri_config::{DeviceCapabilities, DevicePlatform};

    fn pending(name: &str, request_id: &str) -> Result<(String, Pending)> {
        let dir = tempfile::tempdir()?;
        let key = paxri_native::identity::init(&dir.path().join("device.key"))?;
        let id = key.public().to_string();
        let (tx, _rx) = oneshot::channel();
        Ok((
            id,
            Pending {
                request_id: request_id.into(),
                peer: Contact {
                    name: name.into(),
                    ticket: EndpointTicket::new(key.public().into()),
                    platform: DevicePlatform::Android,
                    capabilities: DeviceCapabilities::default(),
                },
                replies: vec![tx],
            },
        ))
    }

    #[test]
    fn resolve_pending_accepts_full_and_unique_short_ids() -> Result<()> {
        let mut pairings = Pairings::default();
        let (first_id, first) = pending("Phone", "req1")?;
        let (second_id, second) = pending("Laptop", "req2")?;
        pairings.pending.insert(first_id.clone(), first);
        pairings.pending.insert(second_id.clone(), second);

        assert_eq!(
            pairings.resolve_pending(&first_id[..4], None)?.0,
            first_id
        );
        assert_eq!(
            pairings
                .resolve_pending(&second_id[..8].to_uppercase(), Some("req2"))?
                .0,
            second_id
        );
        assert_eq!(
            pairings.resolve_pending(&first_id, Some("req1"))?.0,
            first_id
        );
        assert!(pairings.resolve_pending(&first_id[..8], Some("stale")).is_err());
        assert!(pairings.resolve_pending(&first_id[..3], None).is_err());
        Ok(())
    }

}
