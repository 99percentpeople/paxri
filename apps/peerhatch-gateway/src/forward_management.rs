use crate::Gateway;
use anyhow::{Context, Result};
use peerhatch_config::Forward;
use peerhatch_control::{Command, ForwardDirectory, PeerForward, Status, forwarding};
use peerhatch_core::{PeerSession, StreamKind};
use std::{collections::BTreeSet, sync::Arc, time::Duration};
use tokio::{
    io::{AsyncRead, AsyncWrite},
    sync::Mutex,
    task::JoinSet,
    time::timeout,
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

async fn apply(state: &mut Gateway, id: &str, action: PeerForward) -> Result<forwarding::Rules> {
    anyhow::ensure!(state.config.is_paired(id), "Peer is not paired");
    let name = match &action {
        PeerForward::List => return Ok(state.forward_status(Some(id))),
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
            listen,
            target,
            direction,
        } => {
            state
                .change_settings(Command::SaveForward {
                    name,
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
        PeerForward::List => unreachable!(),
    }
    Ok(state.forward_status(Some(id)))
}

async fn query(
    owner: &str,
    session: impl PeerSession,
    op: PeerForward,
) -> Result<forwarding::Rules> {
    let mut rules = timeout(Duration::from_secs(8), async {
        let mut stream = session.open(StreamKind::ForwardRules).await?;
        forwarding::exchange(&mut stream, &op).await
    })
    .await
    .context("Peer forwarding request timed out")?
    .with_context(|| format!("Could not manage forwards on {owner}"))?;
    for rule in rules.values_mut() {
        rule.direction = rule.direction.opposite();
        rule.peer = owner.to_owned();
    }
    Ok(rules)
}

pub(super) async fn list(gateway: &Mutex<Gateway>) -> Status {
    let mut jobs = JoinSet::new();
    let mut seen = BTreeSet::new();
    {
        let state = gateway.lock().await;
        for (name, peer) in &state.config.peers {
            if let Ok(session) = state.transport.online_session(name)
                && seen.insert(session.peer_id())
            {
                let owner = name.clone();
                let id = peer.endpoint_id.clone();
                jobs.spawn(async move {
                    let result = query(&owner, session, PeerForward::List).await;
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
        if status.peers.get(&owner) != Some(&id) || state.transport.online_session(&owner).is_err()
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
    query(owner, session, action).await?;
    Ok(gateway.lock().await.status())
}
