//! One control stream per remote listener, plus a peer-opened stream per accepted socket.
use super::{read_frame, write_frame};
use crate::{PeerSession, StreamKind};
use anyhow::{Context, Result, bail, ensure};
use paxri_config::Config;
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use std::{sync::Arc, time::Duration};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, copy_bidirectional};
use tokio::{
    net::TcpListener,
    sync::{Semaphore, watch},
    task::JoinSet,
    time::sleep,
};

pub type ListenerId = [u8; 16];

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Listen {
    listen: SocketAddr,
    id: ListenerId,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Reply {
    Bound(SocketAddr),
    Error(String),
}

pub async fn request(
    send: &mut (impl AsyncWrite + Unpin),
    recv: &mut (impl AsyncRead + Unpin),
    listen: SocketAddr,
    id: ListenerId,
) -> Result<SocketAddr> {
    write_frame(send, &Listen { listen, id }).await?;
    match read_frame(recv).await? {
        Reply::Bound(addr) => {
            ensure!(
                addr.ip() == listen.ip()
                    && addr.port() != 0
                    && (listen.port() == 0 || listen.port() == addr.port()),
                "Peer returned an unexpected listen address"
            );
            Ok(addr)
        }
        Reply::Error(error) => bail!("{error}"),
    }
}

pub async fn receive(recv: &mut (impl AsyncRead + Unpin)) -> Result<(SocketAddr, ListenerId)> {
    let request = read_frame::<Listen>(recv).await?;
    Ok((request.listen, request.id))
}

pub async fn respond(
    send: &mut (impl AsyncWrite + Unpin),
    result: Result<SocketAddr, String>,
) -> Result<()> {
    let reply = match result {
        Ok(addr) => Reply::Bound(addr),
        Err(reason) => Reply::Error(reason),
    };
    write_frame(send, &reply).await
}

#[derive(Debug)]
pub struct Listeners {
    config: watch::Receiver<Arc<Config>>,
    listeners: Semaphore,
    streams: Arc<Semaphore>,
}

impl Listeners {
    pub fn new(config: watch::Receiver<Arc<Config>>, streams: Arc<Semaphore>) -> Self {
        Self {
            config,
            listeners: Semaphore::new(64),
            streams,
        }
    }

    pub async fn serve<S: PeerSession>(&self, session: S, stream: S::Stream) -> Result<()> {
        let peer = session.peer_id();
        let mut config = self.config.clone();
        let (mut recv, mut send) = tokio::io::split(stream);
        let (listen, id) = receive(&mut recv).await?;
        let paired = |cfg: &Config| cfg.is_paired(&peer);
        let binding = async {
            ensure!(
                paired(&config.borrow_and_update()),
                "DENIED: peer is not paired"
            );
            let slot = self
                .listeners
                .try_acquire()
                .context("Remote listener limit reached")?;
            let listener = TcpListener::bind(listen)
                .await
                .with_context(|| format!("Bind {listen}"))?;
            Ok::<_, anyhow::Error>((listener, slot))
        }
        .await;
        let (listener, slot) = match binding {
            Ok(pair) => pair,
            Err(e) => {
                respond(&mut send, Err(format!("{e:#}"))).await?;
                send.shutdown().await?;
                return Ok(());
            }
        };
        respond(&mut send, Ok(listener.local_addr()?)).await?;
        let mut flows = JoinSet::new();
        loop {
            tokio::select! {
                biased;
                _ = recv.read_u8() => break,
                changed = config.changed() => {
                    if changed.is_err() || !paired(&config.borrow_and_update()) { break; }
                }
                _ = flows.join_next(), if !flows.is_empty() => {}
                accepted = listener.accept() => {
                    let (mut tcp, _) = match accepted {
                        Ok(pair) => pair,
                        Err(e) => { eprintln!("accept on {listen}: {e}"); sleep(Duration::from_millis(250)).await; continue; }
                    };
                    let Ok(permit) = self.streams.clone().try_acquire_owned() else { continue };
                    let session = session.clone();
                    flows.spawn(async move {
                        let _permit = permit;
                        let mut stream=session.open(StreamKind::ForwardedTcp).await?;
                        // Identify the locally authorized target before forwarding server-first TCP.
                        stream.write_all(&id).await?;
                        stream.flush().await?;
                        copy_bidirectional(&mut tcp,&mut stream).await?;
                        Ok::<(),anyhow::Error>(())
                    });
                }
            }
        }
        drop(listener);
        drop(slot);
        send.shutdown().await?;
        while flows.join_next().await.is_some() {}
        Ok(())
    }
}
