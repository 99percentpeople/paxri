//! Owner-scoped, gateway-hosted interactive terminals.
use crate::{Frame, Size};
use anyhow::{Context, Result, bail, ensure};
use paxri_core::{PeerSession, StreamKind};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::{sync::Arc, time::Duration};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    sync::Notify,
    time::timeout,
};

pub const HISTORY_LIMIT: usize = 256 * 1024;
pub const SESSION_LIMIT: usize = paxri_core::limits::DEFAULT_MAX_TERMINAL_SESSIONS;
const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
const INITIAL_PROBE_TIMEOUT: Duration = Duration::from_secs(4);
const KEEPALIVE_PROBE_TIMEOUT: Duration = Duration::from_secs(6);
const KEEPALIVE_INTERVAL: Duration = Duration::from_secs(10);

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

/// A definitive response from the gateway, as opposed to a lost stream or timeout.
#[derive(Debug)]
pub struct Rejected(String);

impl std::fmt::Display for Rejected {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl std::error::Error for Rejected {}

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
    request_with_timeout(peer, action, DEFAULT_REQUEST_TIMEOUT).await
}

pub async fn request_with_timeout<S: PeerSession>(
    peer: S,
    action: Request,
    limit: Duration,
) -> Result<(S::Stream, Vec<Key>)> {
    timeout(limit, async {
        let mut stream = peer.open(StreamKind::ManagedTerminal).await?;
        write(&mut stream, &action).await?;
        match read::<Result<Vec<Key>, String>>(&mut stream)
            .await
            .context("the peer gateway must support managed terminals")?
        {
            Ok(keys) => Ok((stream, keys)),
            Err(message) => Err(Rejected(message).into()),
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

/// Read replay history while requiring forward progress within `idle_timeout` per chunk.
pub async fn read_history_with_timeout(
    input: &mut (impl AsyncRead + Unpin),
    idle_timeout: Duration,
) -> Result<Vec<u8>> {
    let length = timeout(idle_timeout, input.read_u32())
        .await
        .context("terminal history header timed out")?? as usize;
    ensure!(length <= HISTORY_LIMIT, "invalid terminal history length");
    let mut bytes = vec![0; length];
    let mut offset = 0;
    while offset < length {
        let read = timeout(idle_timeout, input.read(&mut bytes[offset..]))
            .await
            .context("terminal history timed out")??;
        ensure!(read > 0, "terminal history ended unexpectedly");
        offset += read;
    }
    Ok(bytes)
}

// Tag 4 is confined to managed streams. The gateway only sends it in response
// to an opted-in client, and never forwards it to the child process.
pub async fn read_frame(input: &mut (impl AsyncRead + Unpin)) -> Result<Option<Frame>> {
    match input.read_u8().await? {
        4 => Ok(None),
        tag => Ok(Some(Frame::read_body(tag, input).await?)),
    }
}

pub async fn write_heartbeat(output: &mut (impl AsyncWrite + Unpin)) -> Result<()> {
    output.write_u8(4).await?;
    output.flush().await?;
    Ok(())
}

pub enum AttachmentEvent {
    Ready,
    Output(Vec<u8>),
}

/// Verify both directions before enabling input, then keep checking even when
/// the shell is idle or is producing continuous output on a one-way connection.
pub async fn attachment<F: Future<Output = Result<()>>>(
    stream: impl AsyncRead + AsyncWrite + Unpin,
    input: tokio::sync::mpsc::UnboundedReceiver<Frame>,
    heartbeat: bool,
    event: impl Fn(AttachmentEvent) -> F,
) -> Result<u32> {
    attachment_with_probe(stream, input, heartbeat, None, event).await
}

/// Like [`attachment`], with an optional notifier that triggers an immediate round-trip probe.
pub async fn attachment_with_probe<F: Future<Output = Result<()>>>(
    stream: impl AsyncRead + AsyncWrite + Unpin,
    mut input: tokio::sync::mpsc::UnboundedReceiver<Frame>,
    heartbeat: bool,
    probe_now: Option<Arc<Notify>>,
    event: impl Fn(AttachmentEvent) -> F,
) -> Result<u32> {
    use tokio::{sync::mpsc, time::sleep};

    let (mut reader, mut writer) = tokio::io::split(stream);
    let (probe, mut probes) = mpsc::channel(1);
    let acknowledged = Notify::new();
    let health = async {
        let mut ready = false;
        loop {
            if heartbeat {
                let limit = if ready {
                    KEEPALIVE_PROBE_TIMEOUT
                } else {
                    INITIAL_PROBE_TIMEOUT
                };
                timeout(limit, async {
                    probe.send(()).await?;
                    acknowledged.notified().await;
                    anyhow::Ok(())
                })
                .await
                .context("Terminal stream did not acknowledge the probe")??;
            }
            if !ready {
                event(AttachmentEvent::Ready).await?;
                ready = true;
            }
            if !heartbeat {
                return std::future::pending().await;
            }
            tokio::select! {
                _ = sleep(KEEPALIVE_INTERVAL) => {}
                _ = async {
                    match &probe_now {
                        Some(notify) => notify.notified().await,
                        None => std::future::pending().await,
                    }
                } => {}
            }
        }
    };
    let send = async {
        loop {
            tokio::select! {
                Some(()) = probes.recv() => write_heartbeat(&mut writer).await?,
                frame = input.recv() => match frame {
                    Some(frame) => frame.write(&mut writer).await?,
                    None => bail!("Terminal closed"),
                },
            }
        }
    };
    let receive = async {
        loop {
            match read_frame(&mut reader).await? {
                None => acknowledged.notify_one(),
                Some(Frame::Data(bytes)) => event(AttachmentEvent::Output(bytes)).await?,
                Some(Frame::Exit(code)) => return Ok(code),
                Some(Frame::Resize(_)) => bail!("Unexpected terminal resize from peer"),
            }
        }
    };
    tokio::select! { result = receive => result, result = send => result, result = health => result }
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

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::{io::duplex, sync::mpsc};

    #[tokio::test(start_paused = true)]
    async fn output_without_a_probe_reply_does_not_mark_a_restored_terminal_ready() {
        let (viewer, mut gateway) = duplex(256);
        let (_input, keys) = mpsc::unbounded_channel();
        let (events, mut observed) = mpsc::unbounded_channel();
        let task = tokio::spawn(async move {
            attachment(viewer, keys, true, |e| {
                std::future::ready(events.send(e).map_err(Into::into))
            })
            .await
        });
        assert!(read_frame(&mut gateway).await.unwrap().is_none());
        Frame::Data(b"live output".to_vec())
            .write(&mut gateway)
            .await
            .unwrap();
        assert!(matches!(
            observed.recv().await,
            Some(AttachmentEvent::Output(_))
        ));
        assert!(
            task.await
                .unwrap()
                .unwrap_err()
                .to_string()
                .contains("acknowledge")
        );
        assert!(observed.recv().await.is_none());
    }

    #[tokio::test(start_paused = true)]
    async fn a_stalled_attachment_times_out_and_a_fresh_one_accepts_input() {
        for respond in [false, true] {
            let (viewer, mut gateway) = duplex(256);
            let (input, keys) = mpsc::unbounded_channel();
            let (events, mut observed) = mpsc::unbounded_channel();
            let task = tokio::spawn(async move {
                attachment(viewer, keys, true, |e| {
                    std::future::ready(events.send(e).map_err(Into::into))
                })
                .await
            });
            assert!(read_frame(&mut gateway).await.unwrap().is_none());
            write_heartbeat(&mut gateway).await.unwrap();
            assert!(matches!(
                observed.recv().await,
                Some(AttachmentEvent::Ready)
            ));
            assert!(read_frame(&mut gateway).await.unwrap().is_none());
            if !respond {
                assert!(task.await.unwrap().is_err());
                continue;
            }
            write_heartbeat(&mut gateway).await.unwrap();
            input.send(Frame::Data(b"after resume".to_vec())).unwrap();
            assert!(
                matches!(read_frame(&mut gateway).await.unwrap(), Some(Frame::Data(b)) if b == b"after resume")
            );
            Frame::Exit(0).write(&mut gateway).await.unwrap();
            assert_eq!(task.await.unwrap().unwrap(), 0);
        }
    }

    #[tokio::test(start_paused = true)]
    async fn read_history_with_timeout_accepts_slow_chunks_and_fails_on_stall() {
        let (mut viewer, mut gateway) = duplex(64);
        let writer = tokio::spawn(async move {
            gateway.write_u32(6).await.unwrap();
            tokio::time::sleep(Duration::from_secs(3)).await;
            gateway.write_all(b"abc").await.unwrap();
            tokio::time::sleep(Duration::from_secs(3)).await;
            gateway.write_all(b"def").await.unwrap();
        });
        let history = read_history_with_timeout(&mut viewer, Duration::from_secs(5))
            .await
            .unwrap();
        writer.await.unwrap();
        assert_eq!(history, b"abcdef");

        let (mut viewer, mut gateway) = duplex(64);
        gateway.write_u32(6).await.unwrap();
        gateway.write_all(b"ab").await.unwrap();
        let err = read_history_with_timeout(&mut viewer, Duration::from_secs(5))
            .await
            .unwrap_err();
        assert!(err.is::<tokio::time::error::Elapsed>());
    }

    #[tokio::test(start_paused = true)]
    async fn on_demand_probe_verifies_live_stream_and_detects_stalled_stream_immediately() {
        let (viewer, mut gateway) = duplex(256);
        let (_input, keys) = mpsc::unbounded_channel();
        let (events, mut observed) = mpsc::unbounded_channel();
        let probe_now = Arc::new(Notify::new());
        let trigger = probe_now.clone();
        let task = tokio::spawn(async move {
            attachment_with_probe(viewer, keys, true, Some(trigger), |e| {
                std::future::ready(events.send(e).map_err(Into::into))
            })
            .await
        });
        assert!(read_frame(&mut gateway).await.unwrap().is_none());
        write_heartbeat(&mut gateway).await.unwrap();
        assert!(matches!(
            observed.recv().await,
            Some(AttachmentEvent::Ready)
        ));

        // Trigger an immediate liveness check well before the 10s keepalive interval.
        tokio::time::sleep(Duration::from_secs(1)).await;
        probe_now.notify_one();
        assert!(
            timeout(Duration::from_secs(1), read_frame(&mut gateway))
                .await
                .unwrap()
                .unwrap()
                .is_none()
        );
        write_heartbeat(&mut gateway).await.unwrap();

        // A second on-demand probe without a reply fails after KEEPALIVE_PROBE_TIMEOUT.
        tokio::time::sleep(Duration::from_secs(1)).await;
        probe_now.notify_one();
        assert!(
            timeout(Duration::from_secs(1), read_frame(&mut gateway))
                .await
                .unwrap()
                .unwrap()
                .is_none()
        );
        let err = task.await.unwrap().unwrap_err();
        assert!(err.is::<tokio::time::error::Elapsed>());
    }
}
