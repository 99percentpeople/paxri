//! The authenticated, multiplexed connection exposed by a P2P backend.
use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use std::future::Future;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

pub const PROTOCOL: &[u8] = b"p2p-tunnel/session/1";

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[repr(u8)]
pub enum StreamKind {
    Services = 1,
    Terminal = 2,
    Command = 3,
    Listen = 4,
    ForwardedTcp = 5,
    Files = 6,
    ForwardRules = 7,
}

impl StreamKind {
    pub async fn read(stream: &mut (impl AsyncRead + Unpin)) -> Result<Self> {
        Ok(match stream.read_u8().await? {
            1 => Self::Services,
            2 => Self::Terminal,
            3 => Self::Command,
            4 => Self::Listen,
            5 => Self::ForwardedTcp,
            6 => Self::Files,
            7 => Self::ForwardRules,
            n => bail!("Unknown peer stream type {n}"),
        })
    }

    pub async fn write(self, stream: &mut (impl AsyncWrite + Unpin)) -> Result<()> {
        stream.write_u8(self as u8).await?;
        stream.flush().await?;
        Ok(())
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Services => "Peer services",
            Self::Terminal => "Terminal",
            Self::Command => "Command",
            Self::Listen => "Remote listener",
            Self::ForwardedTcp => "Forwarded TCP",
            Self::Files => "File transfer",
            Self::ForwardRules => "Forwarding rules",
        }
    }
}

pub trait PeerSession: Clone + Send + Sync + 'static {
    type Stream: AsyncRead + AsyncWrite + Unpin + Send + 'static;
    fn peer_id(&self) -> String;
    fn open(&self, kind: StreamKind) -> impl Future<Output = Result<Self::Stream>> + Send;
}

/// The backend delivers streams opened by the peer on both dialed and accepted sessions.
pub struct IncomingStream<S: PeerSession> {
    pub session: S,
    pub kind: StreamKind,
    pub stream: S::Stream,
}

pub trait Transport: Send + Sync + 'static {
    type Session: PeerSession;
    fn session(&self, peer: &str) -> impl Future<Output = Result<Self::Session>> + Send;

    fn open(
        &self,
        peer: &str,
        kind: StreamKind,
    ) -> impl Future<Output = Result<<Self::Session as PeerSession>::Stream>> + Send {
        async move { self.session(peer).await?.open(kind).await }
    }
}
