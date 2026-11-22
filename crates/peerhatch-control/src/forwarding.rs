//! Only forwarding operations are accepted on a peer management stream.
use super::{Direction, ForwardStatus, read, write};
use anyhow::Result;
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
    Edit {
        name: String,
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
