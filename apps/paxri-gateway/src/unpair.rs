//! Revoke locally before contacting the other gateway. The pending notice survives restarts.
use super::{Gateway, pairing::random_id};
use anyhow::{Result, ensure};
use paxri_config::PendingUnpair;
use paxri_control::Status;
use paxri_native::gateway::pairing::UnpairAck;
use std::{collections::BTreeSet, sync::Arc, time::Duration};
use tokio::sync::Mutex;

impl Gateway {
    async fn remove_identity(&mut self, id: &str, pending: Option<&PendingUnpair>) -> Result<()> {
        let endpoint = id.parse()?;
        let aliases: BTreeSet<_> = self
            .config
            .peers
            .iter()
            .filter(|(_, p)| p.endpoint_id.eq_ignore_ascii_case(id))
            .map(|(name, _)| name.as_str())
            .collect();
        let rules: Vec<_> = self
            .config
            .forwards
            .iter()
            .filter(|(_, r)| aliases.contains(r.peer.as_str()))
            .map(|(name, _)| name.clone())
            .collect();
        let (text, config) =
            self.file
                .prepare_unpair(&aliases, &rules, pending.map(|p| (id, p)))?;
        self.file.persist(text)?;
        self.apply_config(config);
        self.transport.forget_peer(endpoint);
        self.pairing.forget(id);
        for rule in rules {
            if let Some(listener) = self.listeners.remove(&rule) {
                listener.stop().await;
            }
        }
        Ok(())
    }

    fn clear_unpair_notice(&mut self, id: &str) -> Result<()> {
        let (text, config) = self.file.confirm_unpair(id)?;
        self.file.persist(text)?;
        self.apply_config(config);
        Ok(())
    }

    pub(super) async fn receive_unpair(
        &mut self,
        id: &str,
        generation: Option<&str>,
    ) -> Result<UnpairAck> {
        let mut names = self
            .config
            .peers
            .iter()
            .filter(|(_, peer)| peer.endpoint_id.eq_ignore_ascii_case(id));
        let Some((name, first)) = names.next() else {
            return Ok(UnpairAck::Cleared);
        };
        // A delayed notice for an earlier pairing must never remove a newly approved relationship.
        let mut generations = first
            .pairing_id
            .as_deref()
            .into_iter()
            .chain(names.filter_map(|(_, p)| p.pairing_id.as_deref()));
        if generations.next() != generation || generations.any(|id| Some(id) != generation) {
            return Ok(UnpairAck::NewerPairing);
        }
        let display_name = self.peer_device_name(first).unwrap_or_else(|| name.clone());
        let message = format!("{display_name} unpaired from this device.");
        self.remove_identity(id, None).await?;
        self.pairing.message = Some(message);
        Ok(UnpairAck::Cleared)
    }
}

async fn deliver(
    gateway: &Mutex<Gateway>,
    id: &str,
    notice: &PendingUnpair,
) -> Result<Option<UnpairAck>> {
    let transport = {
        let mut g = gateway.lock().await;
        if g.config.pending_unpairs.get(id) != Some(notice) {
            return Ok(None);
        }
        if g.config.is_paired(id) {
            // A locally restored pairing also supersedes any notice left in the file.
            g.clear_unpair_notice(id)?;
            return Ok(Some(UnpairAck::NewerPairing));
        }
        g.transport.clone()
    };
    let ack = transport.notify_unpair(id, notice).await?;
    let mut g = gateway.lock().await;
    if g.config.pending_unpairs.get(id) == Some(notice) {
        g.clear_unpair_notice(id)?;
        g.pairing.message = Some(result_message(&notice.name, Some(ack)));
        return Ok(Some(ack));
    }
    Ok(None)
}

fn result_message(name: &str, ack: Option<UnpairAck>) -> String {
    match ack {
        Some(UnpairAck::Cleared) => format!("Unpaired {name} on both devices."),
        Some(UnpairAck::NewerPairing) => {
            format!("Removed the old pairing with {name}. The peer kept its newer pairing.")
        }
        None => format!("Unpaired {name} locally. Waiting for the other gateway to confirm."),
    }
}

pub(super) async fn remove(
    gateway: &Mutex<Gateway>,
    selector: &str,
    expected_id: Option<&str>,
) -> Result<Status> {
    let (id, notice, online) = {
        let mut g = gateway.lock().await;
        let (name, peer) = g.config.resolve_peer(selector)?;
        let id = peer.endpoint_id.to_ascii_lowercase();
        ensure!(
            expected_id.is_none_or(|expected| expected.eq_ignore_ascii_case(&id)),
            "This device name now belongs to a different identity."
        );
        let online = g.transport.online_session(name).is_ok();
        let notice = PendingUnpair {
            name: g.peer_device_name(peer).unwrap_or_else(|| name.clone()),
            ticket: g.transport.peer_ticket(name)?.to_string(),
            pairing_id: g
                .config
                .peers
                .values()
                .filter(|p| p.endpoint_id.eq_ignore_ascii_case(&id))
                .find_map(|p| p.pairing_id.clone()),
            request_id: random_id()?,
        };
        g.remove_identity(&id, Some(&notice)).await?;
        g.pairing.message = Some(result_message(&notice.name, None));
        (id, notice, online)
    };
    let ack = if online {
        deliver(gateway, &id, &notice).await.ok().flatten()
    } else {
        None
    };
    let mut status = gateway.lock().await.status();
    status.pairing.message = Some(result_message(&notice.name, ack));
    Ok(status)
}

pub(super) async fn retry_pending(gateway: Arc<Mutex<Gateway>>) {
    let mut changes = gateway.lock().await.transport.subscribe_config();
    loop {
        if changes
            .wait_for(|cfg| !cfg.pending_unpairs.is_empty())
            .await
            .is_err()
        {
            return;
        }
        // Allow the immediate online exchange to finish, and pace retries for offline devices.
        tokio::time::sleep(Duration::from_secs(15)).await;
        let pending = changes.borrow_and_update().pending_unpairs.clone();
        for (id, notice) in pending {
            let _ = deliver(&gateway, &id, &notice).await;
        }
        if !changes.borrow().pending_unpairs.is_empty() {
            tokio::time::sleep(Duration::from_secs(45)).await;
        }
    }
}
