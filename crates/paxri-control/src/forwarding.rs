//! Only forwarding operations are accepted on a peer management stream.
use super::{Direction, ForwardStatus, read, write};
use anyhow::{Context, Result};
use paxri_core::{PeerSession, StreamKind};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, net::SocketAddr, time::Duration};
use tokio::io::{AsyncRead, AsyncWrite};

pub type Rules = BTreeMap<String, ForwardStatus>;

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct ForwardDirectory {
    pub rules: BTreeMap<String, Rules>,
    pub errors: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum PeerForward {
    List,
    Create {
        name: String,
        listen: SocketAddr,
        target: String,
        direction: Direction,
    },
    Edit {
        name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        new_name: Option<String>,
        listen: SocketAddr,
        target: String,
        direction: Direction,
    },
    SetEnabled {
        name: String,
        enabled: bool,
    },
    Remove {
        name: String,
    },
}

pub async fn receive(stream: &mut (impl AsyncRead + Unpin)) -> Result<PeerForward> {
    tokio::time::timeout(Duration::from_secs(5), read(stream)).await?
}

pub async fn reply(stream: &mut (impl AsyncWrite + Unpin), result: Result<Rules>) -> Result<()> {
    let result = result.map_err(|error| format!("{error:#}"));
    tokio::time::timeout(Duration::from_secs(5), write(stream, &result)).await?
}

pub async fn exchange(
    stream: &mut (impl AsyncRead + AsyncWrite + Unpin),
    op: &PeerForward,
) -> Result<Rules> {
    write(stream, op).await?;
    let result: Result<Rules, String> = read(stream).await?;
    result.map_err(anyhow::Error::msg)
}

/// Query the peer over its authenticated session and express returned routes from the caller's side.
pub async fn query(owner: &str, session: impl PeerSession, op: PeerForward) -> Result<Rules> {
    let renaming = matches!(&op, PeerForward::Edit { new_name: Some(_), .. });
    let mut rules = tokio::time::timeout(Duration::from_secs(8), async {
        let mut stream = session.open(StreamKind::ForwardRules).await?;
        exchange(&mut stream, &op).await
    })
    .await
    .context("Peer forwarding request timed out")?
    .map_err(|error| {
        if renaming && error.to_string().contains("unknown field `new_name`") {
            anyhow::anyhow!("Update {owner}’s gateway to rename forwarding rules")
        } else {
            error
        }
    })
    .with_context(|| format!("Could not manage forwards on {owner}"))?;
    for rule in rules.values_mut() {
        rule.direction = rule.direction.opposite();
        rule.peer = owner.to_owned();
    }
    Ok(rules)
}
