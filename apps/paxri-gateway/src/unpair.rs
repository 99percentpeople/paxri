//! Revoke locally before contacting the other gateway. The pending notice survives restarts.
use super::{Gateway, Listener, pairing::random_id};
use anyhow::{Result, ensure};
use paxri_config::PendingUnpair;
use paxri_control::Status;
use paxri_native::gateway::pairing::UnpairAck;
use std::{collections::BTreeSet, sync::Arc, time::Duration};
use tokio::{sync::Mutex, task::JoinSet};

async fn stop_listeners(listeners: Vec<Listener>) {
    let mut stopping = JoinSet::new();
    for listener in listeners {
        stopping.spawn(listener.stop());
    }
    while stopping.join_next().await.is_some() {}
}

async fn remove_identity(
    gateway: &Mutex<Gateway>,
    id: &str,
    pending: Option<PendingUnpair>,
    message: Option<String>,
) -> Result<()> {
    let endpoint = id.parse()?;
    let removed_listeners = Gateway::update_config(
        gateway,
        |g| {
            let aliases: BTreeSet<_> = g
                .config
                .peers
                .iter()
                .filter(|(_, p)| p.endpoint_id.eq_ignore_ascii_case(id))
                .map(|(name, _)| name.as_str())
                .collect();
            let rules: Vec<_> = g
                .config
                .forwards
                .iter()
                .filter(|(_, r)| aliases.contains(r.peer.as_str()))
                .map(|(name, _)| name.clone())
                .collect();
            let (text, config) =
                g.file
                    .prepare_unpair(&aliases, &rules, pending.as_ref().map(|p| (id, p)))?;
            Ok(Some((text, config, rules)))
        },
        |g, rules| {
            g.transport.forget_peer(endpoint);
            g.pairing.forget(id);
            if let Some(message) = message {
                g.pairing.message = Some(message);
            }
            rules
                .into_iter()
                .filter_map(|rule| g.listeners.remove(&rule))
                .collect::<Vec<_>>()
        },
    )
    .await?
    .unwrap_or_default();
    // Stop removed listeners concurrently after releasing gateway.lock().
    stop_listeners(removed_listeners).await;
    Ok(())
}

async fn clear_unpair_notice(
    gateway: &Mutex<Gateway>,
    id: &str,
    expected_notice: Option<&PendingUnpair>,
    message: Option<String>,
) -> Result<bool> {
    let cleared = Gateway::update_config(
        gateway,
        |g| {
            if let Some(expected) = expected_notice
                && g.config.pending_unpairs.get(id) != Some(expected)
            {
                return Ok(None);
            }
            let (text, config) = g.file.confirm_unpair(id)?;
            Ok(Some((text, config, ())))
        },
        |g, ()| {
            if let Some(message) = message {
                g.pairing.message = Some(message);
            }
        },
    )
    .await?;
    Ok(cleared.is_some())
}

pub(super) async fn receive_unpair(
    gateway: &Mutex<Gateway>,
    id: &str,
    generation: Option<&str>,
) -> Result<UnpairAck> {
    let message = {
        let g = gateway.lock().await;
        let mut names = g
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
        let display_name = g.peer_device_name(first).unwrap_or_else(|| name.clone());
        format!("{display_name} unpaired from this device.")
    };
    remove_identity(gateway, id, None, Some(message)).await?;
    Ok(UnpairAck::Cleared)
}

async fn deliver(
    gateway: &Mutex<Gateway>,
    id: &str,
    notice: &PendingUnpair,
) -> Result<Option<UnpairAck>> {
    enum Action {
        Skip,
        ClearSuperseded,
        Notify(Arc<paxri_native::gateway::NativeTransport>),
    }
    let action = {
        let g = gateway.lock().await;
        if g.config.pending_unpairs.get(id) != Some(notice) {
            Action::Skip
        } else if g.config.is_paired(id) {
            Action::ClearSuperseded
        } else {
            Action::Notify(g.transport.clone())
        }
    };
    let transport = match action {
        Action::Skip => return Ok(None),
        Action::ClearSuperseded => {
            // A locally restored pairing also supersedes any notice left in the file.
            clear_unpair_notice(gateway, id, Some(notice), None).await?;
            return Ok(Some(UnpairAck::NewerPairing));
        }
        Action::Notify(transport) => transport,
    };
    let ack = transport.notify_unpair(id, notice).await?;
    if clear_unpair_notice(
        gateway,
        id,
        Some(notice),
        Some(result_message(&notice.name, Some(ack))),
    )
    .await?
    {
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
        let g = gateway.lock().await;
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
        (id, notice, online)
    };
    let pending_message = result_message(&notice.name, None);
    remove_identity(gateway, &id, Some(notice.clone()), Some(pending_message)).await?;
    let ack = if online {
        deliver(gateway, &id, &notice).await.ok().flatten()
    } else {
        None
    };
    let mut status = Gateway::status_snapshot(gateway).await;
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
