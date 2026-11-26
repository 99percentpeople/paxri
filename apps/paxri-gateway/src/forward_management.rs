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
    let result = {
        let mut state = gateway.lock().await;
        apply(&mut state, peer_id, action).await
    };
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

async fn apply(state: &mut Gateway, id: &str, action: PeerForward) -> Result<forwarding::Rules> {
    anyhow::ensure!(state.config.is_paired(id), "Peer is not paired");
    let rule_id = match &action {
        PeerForward::List => return Ok(state.forward_status(Some(id))),
        PeerForward::Create {
            name,
            listen,
            target,
            direction,
        } => {
            let name = normalize_name(name.clone())?;
            let (peer, _) = state
                .config
                .peers
                .iter()
                .find(|(_, p)| p.endpoint_id.eq_ignore_ascii_case(id))
                .context("Peer is not paired")?;
            state
                .change_settings(Command::SaveForward {
                    id: String::new(),
                    edit: false,
                    expected_peer: None,
                    rule: Forward {
                        name,
                        peer: peer.clone(),
                        listen: *listen,
                        target: target.clone(),
                        direction: direction.opposite(),
                    },
                })
                .await?;
            return Ok(state.forward_status(Some(id)));
        }
        PeerForward::Edit { id, .. }
        | PeerForward::Remove { id }
        | PeerForward::SetEnabled { id, .. } => id,
    };
    // Derive the other endpoint from the saved rule and authenticated transport identity.
    let peer = state
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
        .clone();
    match action {
        PeerForward::Edit {
            id,
            name,
            listen,
            target,
            direction,
        } => {
            let name = normalize_name(name)?;
            state
                .change_settings(Command::SaveForward {
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
                })
                .await?;
        }
        PeerForward::SetEnabled { id, enabled } => {
            state.set_forward(&id, enabled, Some(&peer)).await?
        }
        PeerForward::Remove { id } => {
            state
                .change_settings(Command::RemoveForward {
                    id,
                    expected_peer: Some(peer),
                })
                .await?
        }
        PeerForward::List | PeerForward::Create { .. } => unreachable!(),
    }
    Ok(state.forward_status(Some(id)))
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
    let mut state = gateway.lock().await;
    let mut status = state.status();
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
    Ok(gateway.lock().await.status())
}
