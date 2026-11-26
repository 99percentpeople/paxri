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

fn peer_rule_name(state: &Gateway, id: &str, requested: &str) -> Result<String> {
    if requested.is_empty() {
        return Ok(String::new());
    }
    anyhow::ensure!(
        crate::settings::valid_forward_name(requested),
        "Invalid rule name"
    );
    let owned_by_peer = |name: &str| {
        state
            .config
            .forwards
            .get(name)
            .is_some_and(|rule| {
                state.config.peers[&rule.peer]
                    .endpoint_id
                    .eq_ignore_ascii_case(id)
            })
    };
    if owned_by_peer(requested) {
        anyhow::bail!("Forward {requested} already exists");
    }
    if !state.config.forwards.contains_key(requested) {
        return Ok(requested.to_owned());
    }
    // Another peer already uses this rule name on the gateway; disambiguate without colliding.
    let name = (2..)
        .map(|n| format!("{requested}-{n}"))
        .find(|candidate| !state.config.forwards.contains_key(candidate))
        .expect("available forward ID");
    Ok(name)
}

async fn apply(state: &mut Gateway, id: &str, action: PeerForward) -> Result<forwarding::Rules> {
    anyhow::ensure!(state.config.is_paired(id), "Peer is not paired");
    let name = match &action {
        PeerForward::List => return Ok(state.forward_status(Some(id))),
        PeerForward::Create {
            name,
            listen,
            target,
            direction,
        } => {
            let name = peer_rule_name(state, id, name)?;
            let (peer, _) = state
                .config
                .peers
                .iter()
                .find(|(_, p)| p.endpoint_id.eq_ignore_ascii_case(id))
                .context("Peer is not paired")?;
            state
                .change_settings(Command::SaveForward {
                    name,
                    new_name: None,
                    edit: false,
                    expected_peer: None,
                    rule: Forward {
                        peer: peer.clone(),
                        listen: *listen,
                        target: target.clone(),
                        direction: direction.opposite(),
                    },
                })
                .await?;
            return Ok(state.forward_status(Some(id)));
        }
        PeerForward::Edit { name, .. }
        | PeerForward::Remove { name }
        | PeerForward::SetEnabled { name, .. } => name,
    };
    // Derive the other endpoint from the saved rule and authenticated transport identity.
    let peer = state
        .config
        .forwards
        .get(name)
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
            name,
            new_name,
            listen,
            target,
            direction,
        } => {
            let new_name = match new_name {
                Some(next) if next != name => Some(peer_rule_name(state, id, &next)?),
                _ => None,
            };
            state
                .change_settings(Command::SaveForward {
                    name,
                    new_name,
                    edit: true,
                    expected_peer: Some(peer.clone()),
                    rule: Forward {
                        peer,
                        listen,
                        target,
                        direction: direction.opposite(),
                    },
                })
                .await?;
        }
        PeerForward::SetEnabled { name, enabled } => {
            state.set_forward(&name, enabled, Some(&peer)).await?
        }
        PeerForward::Remove { name } => {
            state
                .change_settings(Command::RemoveForward {
                    name,
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
