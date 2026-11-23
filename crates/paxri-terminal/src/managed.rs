//! Owner-scoped, gateway-hosted interactive terminals.
use crate::Size;
use anyhow::{Context, Result, bail, ensure};
use paxri_core::{PeerSession, StreamKind};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

pub const HISTORY_LIMIT: usize = 256 * 1024;
pub const SESSION_LIMIT: usize = 16;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Key {
    pub number: usize,
    pub nonce: u64,
}

impl Key {
    pub fn new(number: usize) -> Result<Self> {
        Ok(Self {
            number,
            nonce: getrandom::u64().context("generating session identifier")?,
        })
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub enum Request {
    List,
    Create(Key, Size),
    Attach(Key, Size),
    End(Key),
}

pub async fn write<T: Serialize>(w: &mut (impl AsyncWrite + Unpin), value: &T) -> Result<()> {
    let data = serde_json::to_vec(value)?;
    ensure!(data.len() <= 4096, "terminal management message too large");
    w.write_u16(data.len() as u16).await?;
    w.write_all(&data).await?;
    w.flush().await?;
    Ok(())
}

pub async fn read<T: DeserializeOwned>(r: &mut (impl AsyncRead + Unpin)) -> Result<T> {
    let len = usize::from(r.read_u16().await?);
    ensure!(
        (1..=4096).contains(&len),
        "invalid terminal management message size"
    );
    let mut buf = vec![0; len];
    r.read_exact(&mut buf).await?;
    Ok(serde_json::from_slice(&buf)?)
}

pub async fn request<S: PeerSession>(peer: S, action: Request) -> Result<(S::Stream, Vec<Key>)> {
    tokio::time::timeout(std::time::Duration::from_secs(15), async {
        let mut stream = peer.open(StreamKind::ManagedTerminal).await?;
        write(&mut stream, &action).await?;
        match read::<Result<Vec<Key>, String>>(&mut stream)
            .await
            .context("the peer gateway must support managed terminals")?
        {
            Ok(keys) => Ok((stream, keys)),
            Err(message) => bail!("{message}"),
        }
    })
    .await
    .context("terminal request timed out")?
}

pub async fn read_history(input: &mut (impl AsyncRead + Unpin)) -> Result<Vec<u8>> {
    let length = input.read_u32().await? as usize;
    ensure!(length <= HISTORY_LIMIT, "invalid terminal history length");
    let mut bytes = vec![0; length];
    input.read_exact(&mut bytes).await?;
    Ok(bytes)
}

/// Dropping the viewer detaches. Only an explicit end signal asks the host to kill the shell.
pub async fn client(
    stream: impl AsyncRead + AsyncWrite + Unpin,
    input: async_channel::Receiver<Vec<u8>>,
    sizes: async_channel::Receiver<portable_pty::PtySize>,
    output: &async_channel::Sender<crate::pty::Output>,
    end: tokio::sync::oneshot::Receiver<()>,
) -> Result<u32> {
    super::run_client(stream, input, sizes, output, async {
        if end.await.is_err() {
            std::future::pending::<()>().await;
        }
    })
    .await
}
