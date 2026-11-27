use crate::Gateway;
use anyhow::{Context, Result};
use paxri_config::Forward;
use paxri_control::{Command, ForwardDirectory, PeerForward, Status, forwarding};
use paxri_core::PeerSession;
use std::{collections::BTreeSet, sync::Arc};
use tokio::{
    io::{AsyncRead, AsyncWrite},
    sync::Mutex,
    task::JoinSet,
};

pub(super) async fn serve(
    gateway: Arc<Mutex<Gateway>>,
    peer_id: &str,
    mut stream: impl AsyncRead + AsyncWrite + Unpin,
) -> Result<()> {
    let action = forwarding::receive(&mut stream).await?;
    let result = apply(&gateway, peer_id, action).await;
    forwarding::reply(&mut stream, result).await
}

fn normalize_name(name: Option<String>) -> Result<Option<String>> {
    let Some(name) = name else {
        return Ok(None);
    };
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    anyhow::ensure!(
        crate::settings::valid_forward_name(trimmed),
        "Invalid rule name"
    );
    Ok(Some(trimmed.to_owned()))
}

async fn apply(
    gateway: &Mutex<Gateway>,
    id: &str,
    action: PeerForward,
) -> Result<forwarding::Rules> {
    let peer = {
        let state = gateway.lock().await;
        anyhow::ensure!(state.config.is_paired(id), "Peer is not paired");
        match &action {
            PeerForward::List => return Ok(state.forward_status(Some(id))),
            PeerForward::Create { .. } => state
                .config
                .peers
                .iter()
                .find(|(_, p)| p.endpoint_id.eq_ignore_ascii_case(id))
                .context("Peer is not paired")?
                .0
                .clone(),
            PeerForward::Edit { id: rule_id, .. }
            | PeerForward::Remove { id: rule_id }
            | PeerForward::SetEnabled { id: rule_id, .. } => state
                .config
                .forwards
                .get(rule_id)
                .filter(|rule| {
                    state.config.peers[&rule.peer]
                        .endpoint_id
                        .eq_ignore_ascii_case(id)
                })
                .context("Unknown forwarding rule for this pairing")?
                .peer
                .clone(),
        }
    };
    match action {
        PeerForward::Create {
            name,
            listen,
            target,
            direction,
        } => {
            let name = normalize_name(name)?;
            Gateway::change_settings_for_peer(
                gateway,
                Command::SaveForward {
                    id: String::new(),
                    edit: false,
                    expected_peer: None,
                    rule: Forward {
                        name,
                        peer,
                        listen,
                        target,
                        direction: direction.opposite(),
                    },
                },
            )
            .await?;
        }
        PeerForward::Edit {
            id,
            name,
            listen,
            target,
            direction,
        } => {
            let name = normalize_name(name)?;
            Gateway::change_settings(
                gateway,
                Command::SaveForward {
                    id,
                    edit: true,
                    expected_peer: Some(peer.clone()),
                    rule: Forward {
                        name,
                        peer,
                        listen,
                        target,
                        direction: direction.opposite(),
                    },
                },
            )
            .await?;
        }
        PeerForward::SetEnabled { id, enabled } => {
            Gateway::set_forward(gateway, &id, enabled, Some(&peer)).await?
        }
        PeerForward::Remove { id } => {
            Gateway::change_settings(
                gateway,
                Command::RemoveForward {
                    id,
                    expected_peer: Some(peer),
                },
            )
            .await?
        }
        PeerForward::List => unreachable!(),
    }
    Ok(gateway.lock().await.forward_status(Some(id)))
}

pub(super) async fn list(gateway: &Mutex<Gateway>) -> Status {
    let mut jobs = JoinSet::new();
    let mut seen = BTreeSet::new();
    {
        let state = gateway.lock().await;
        for (name, peer) in &state.config.peers {
            let platform = state.peer_platform(peer).unwrap_or_default();
            let capabilities = state.device_info_for(peer).map(|info| info.capabilities);
            if platform.supports_forward_rules(capabilities)
                && let Ok(session) = state.transport.online_session(name)
                && seen.insert(session.peer_id())
            {
                let owner = name.clone();
                let id = peer.endpoint_id.clone();
                jobs.spawn(async move {
                    let result = forwarding::query(&owner, session, PeerForward::List).await;
                    (owner, id, result)
                });
            }
        }
    }
    let mut results = Vec::new();
    while let Some(job) = jobs.join_next().await {
        if let Ok(item) = job {
            results.push(item);
        }
    }
    let mut status = Gateway::status_snapshot(gateway).await;
    let state = gateway.lock().await;
    let mut directory = ForwardDirectory::default();
    for (owner, id, result) in results {
        if status.peers.get(&owner) != Some(&id)
            || !status.peer_supports_forward_rules(&owner)
            || state.transport.online_session(&owner).is_err()
        {
            continue;
        }
        match result {
            Ok(rules) => {
                directory.rules.insert(owner, rules);
            }
            Err(error) => {
                directory.errors.insert(owner, format!("{error:#}"));
            }
        }
    }
    drop(state);
    status.peer_forwards = Some(directory);
    status
}

pub(super) async fn change(
    gateway: &Mutex<Gateway>,
    owner: &str,
    action: PeerForward,
) -> Result<Status> {
    let session = gateway.lock().await.transport.online_session(owner)?;
    forwarding::query(owner, session, action).await?;
    Ok(Gateway::status_snapshot(gateway).await)
}

pub(crate) fn peer_forward_count(config: &paxri_config::Config, endpoint_id: &str) -> usize {
    config
        .forwards
        .values()
        .filter(|existing| {
            config
                .peers
                .get(&existing.peer)
                .is_some_and(|p| p.endpoint_id.eq_ignore_ascii_case(endpoint_id))
        })
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn peer_forward_count_aggregates_aliases_for_same_identity() {
        let id_a = "aa".repeat(32);
        let id_b = "bb".repeat(32);
        let cfg = paxri_config::parse(&format!(
            "\
version = 1
[gateway.limits]
max_forwards = 10
max_forwards_per_peer = 2
[peers.a1]
endpoint_id = '{id_a}'
[peers.a2]
endpoint_id = '{id_a}'
[peers.b]
endpoint_id = '{id_b}'
[forwards.r1]
listen = '127.0.0.1:0'
peer = 'a1'
target = '127.0.0.1:8080'
[forwards.r2]
listen = '127.0.0.1:0'
peer = 'a2'
target = '127.0.0.1:8081'
[forwards.r3]
listen = '127.0.0.1:0'
peer = 'b'
target = '127.0.0.1:8082'
"
        ))
        .unwrap();
        assert_eq!(peer_forward_count(&cfg, &id_a.to_uppercase()), 2);
        assert_eq!(peer_forward_count(&cfg, &id_b), 1);
        assert_eq!(cfg.gateway.limits.effective_max_forwards_per_peer(), 2);
        assert!(peer_forward_count(&cfg, &id_a) >= cfg.gateway.limits.effective_max_forwards_per_peer());
    }
}
