use super::Gateway;
use anyhow::{Context, Result, ensure};
use std::{
    collections::BTreeMap,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::{
    sync::{Mutex, oneshot},
    task::AbortHandle,
};
use tunnel_control::{PairingPeer, PairingStatus};
use tunnel_native::{
    EndpointTicket,
    gateway::pairing::{Contact, Decision, Incoming, Invitation},
};

fn random_id() -> Result<String> {
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

    pub(super) fn status(&mut self, code: Option<&str>) -> PairingStatus {
        self.prune();
        PairingStatus {
            code: code.filter(|_| self.token.is_some()).map(str::to_owned),
            incoming: self
                .pending
                .iter()
                .map(|(id, p)| PairingPeer {
                    request_id: p.request_id.clone(),
                    name: p.peer.name.clone(),
                    device_id: id.clone(),
                    platform: p.peer.platform.unwrap_or_default(),
                })
                .collect(),
            outgoing: self.outgoing.as_ref().map(|o| o.id.clone()),
            message: self.message.clone(),
        }
    }

    pub(super) fn receive(&mut self, incoming: Incoming) {
        self.prune();
        let Incoming {
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

    pub(super) fn cancel(&mut self) -> Result<()> {
        self.outgoing
            .take()
            .context("No outgoing pairing request")?
            .task
            .abort();
        self.message = Some("Pairing request canceled.".into());
        Ok(())
    }
}

impl Gateway {
    pub(super) async fn init_pairing(&mut self) -> Result<()> {
        if let Some(code) = &self.config.pairing_code {
            let saved = Invitation::parse(code)?;
            ensure!(
                saved.ticket.endpoint_addr().id.to_string() == self.transport.id(),
                "Saved pairing code belongs to another device. Refresh the code."
            );
            self.pairing.token = Some(saved.token);
            return Ok(());
        }
        let ticket = self
            .transport
            .initial_pairing_ticket(self.direct_only)
            .await?;
        self.refresh_pairing(&ticket)
    }

    pub(super) fn refresh_pairing(&mut self, ticket: &EndpointTicket) -> Result<()> {
        ensure!(
            !self.direct_only || !ticket.endpoint_addr().is_empty(),
            "No address available for the pairing code"
        );
        let token = random_id()?;
        let code = Invitation::encode(&ticket.to_string(), &token);
        let (text, config) = self.file.prepare_pairing_code(&code)?;
        self.file.persist(text)?;
        self.apply_config(config);
        for (_, p) in std::mem::take(&mut self.pairing.pending) {
            for reply in p.replies {
                let _ = reply.send(Decision::Rejected("The pairing code was refreshed".into()));
            }
        }
        self.pairing.replies.clear();
        self.pairing.token = Some(token);
        self.pairing.message = None;
        Ok(())
    }

    fn contact(&self) -> Contact {
        let name = self
            .config
            .device_name
            .clone()
            .unwrap_or_else(|| gethostname::gethostname().to_string_lossy().into_owned());
        self.transport.contact(name)
    }

    fn save_peer(&mut self, peer: &Contact) -> Result<String> {
        let id = peer.id().to_string();
        ensure!(id != self.transport.id(), "Cannot pair with this device");
        let peers = &self.config.peers;
        let alias = if let Some((name, _)) = peers
            .iter()
            .find(|(_, p)| p.endpoint_id.eq_ignore_ascii_case(&id))
        {
            name.clone()
        } else {
            let name = peer.name.trim();
            let mut alias = name.to_owned();
            for suffix in 2.. {
                if !peers.contains_key(&alias) {
                    break;
                }
                alias = format!("{name}-{suffix}");
            }
            alias
        };
        let (text, config) =
            self.file
                .prepare_peer(&alias, &id, &peer.ticket.to_string(), peer.platform)?;
        self.file.persist(text)?;
        self.transport.set_peer(alias.clone(), peer.ticket.clone());
        self.apply_config(config);
        Ok(alias)
    }

    pub(super) fn answer_pairing(
        &mut self,
        id: &str,
        request_id: Option<&str>,
        accept: bool,
    ) -> Result<()> {
        self.pairing.prune();
        let peer = self
            .pairing
            .pending
            .get(id)
            .filter(|p| request_id.is_none_or(|r| r == p.request_id))
            .context("This pairing request is no longer pending")?
            .peer
            .clone();
        let (decision, message) = if accept {
            let alias = self.save_peer(&peer)?;
            (
                Decision::Accepted(self.contact()),
                format!("Paired with {alias}."),
            )
        } else {
            (
                Decision::Rejected("Pairing request declined".into()),
                format!("Declined pairing with {}.", peer.name),
            )
        };
        let pending = self.pairing.pending.remove(id).expect("pending request");
        for reply in pending.replies {
            let _ = reply.send(decision.clone());
        }
        self.pairing.replies.insert(
            (id.into(), pending.request_id),
            (Instant::now() + Duration::from_secs(90), decision),
        );
        self.pairing.message = Some(message);
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
            let mut g = gateway.lock().await;
            if !g
                .pairing
                .outgoing
                .as_ref()
                .is_some_and(|o| o.request_id == attempt)
            {
                return;
            }
            let message = match result.and_then(|decision| match decision {
                Decision::Accepted(peer) => g.save_peer(&peer),
                Decision::Rejected(reason) => anyhow::bail!("{reason}"),
            }) {
                Ok(alias) => format!("Paired with {alias}."),
                Err(e) => format!("Pairing failed: {e:#}"),
            };
            g.pairing.outgoing = None;
            g.pairing.message = Some(message);
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
