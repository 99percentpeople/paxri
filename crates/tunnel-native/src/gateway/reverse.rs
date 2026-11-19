use super::NativeTransport;
use crate::CONNECT_TIMEOUT;
use anyhow::{Context, Result, bail, ensure};
use iroh::{
    endpoint::{Connection, RecvStream, SendStream},
    protocol::{AcceptError, ProtocolHandler},
};
use std::{net::SocketAddr, sync::Arc, time::Duration};
use tokio::{
    io::{AsyncReadExt, copy_bidirectional, join},
    net::{TcpListener, TcpStream},
    sync::{Semaphore, oneshot, watch},
    task::{JoinHandle, JoinSet},
    time::{sleep, timeout},
};
use tunnel_config::Config;
use tunnel_core::reverse::{self as wire, INCOMING};

async fn relay(tcp: &mut TcpStream, rx: RecvStream, tx: SendStream) -> Result<()> {
    let mut stream = join(rx, tx);
    copy_bidirectional(tcp, &mut stream).await?;
    // The listener may already be paused. Keep the last connection handle alive
    // until QUIC has delivered this flow's final bytes and FIN.
    let (_, tx) = stream.into_inner();
    tx.stopped().await?;
    Ok(())
}

#[derive(Debug)]
pub(super) struct Listeners {
    config: watch::Receiver<Arc<Config>>,
    listeners: Semaphore,
    streams: Arc<Semaphore>,
}

impl Listeners {
    pub(super) fn new(config: watch::Receiver<Arc<Config>>, streams: Arc<Semaphore>) -> Self {
        Self {
            config,
            listeners: Semaphore::new(64),
            streams,
        }
    }

    async fn serve(&self, conn: Connection) -> Result<()> {
        let peer = conn.remote_id().to_string();
        let mut config = self.config.clone();
        if !config
            .borrow()
            .peers
            .values()
            .any(|p| p.endpoint_id.eq_ignore_ascii_case(&peer))
        {
            conn.close(1u32.into(), b"peer not allowed");
            return Ok(());
        }
        let (mut send, mut recv) = timeout(CONNECT_TIMEOUT, conn.accept_bi()).await??;
        let listen = wire::receive(&mut recv).await?;
        let permitted = |cfg: &Config| {
            cfg.peers.values().any(|p| {
                p.endpoint_id.eq_ignore_ascii_case(&peer) && p.ports.listen.contains(&listen)
            })
        };
        let binding = async {
            ensure!(
                permitted(&config.borrow_and_update()),
                "DENIED: peer has no permission to listen on {listen}"
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
                wire::respond(&mut send, Err(format!("{e:#}"))).await?;
                send.finish()?;
                let _ = timeout(CONNECT_TIMEOUT, send.stopped()).await;
                return Ok(());
            }
        };
        wire::respond(&mut send, Ok(listener.local_addr()?)).await?;
        let mut flows = JoinSet::new();
        loop {
            tokio::select! {
                biased;
                _ = recv.read_u8() => break,
                changed = config.changed() => {
                    if changed.is_err() || !permitted(&config.borrow_and_update()) { break; }
                }
                _ = flows.join_next(), if !flows.is_empty() => {}
                accepted = listener.accept() => {
                    let (mut tcp, _) = match accepted {
                        Ok(pair) => pair,
                        Err(e) => { eprintln!("accept on {listen}: {e}"); sleep(Duration::from_millis(250)).await; continue; }
                    };
                    let Ok(permit) = self.streams.clone().try_acquire_owned() else { continue };
                    let conn = conn.clone();
                    flows.spawn(async move {
                        let _permit = permit;
                        let (mut tx, rx) = timeout(CONNECT_TIMEOUT, conn.open_bi()).await??;
                        // Announce even silent TCP clients so server-first protocols work.
                        tx.write_all(&[INCOMING]).await?;
                        relay(&mut tcp, rx, tx).await
                    });
                }
            }
        }
        drop(listener);
        drop(slot);
        send.finish()?;
        let _ = timeout(CONNECT_TIMEOUT, send.stopped()).await;
        while flows.join_next().await.is_some() {}
        Ok(())
    }
}

impl ProtocolHandler for Listeners {
    async fn accept(&self, conn: Connection) -> Result<(), AcceptError> {
        self.serve(conn)
            .await
            .map_err(|e| AcceptError::from_boxed(e.into()))
    }
}

#[derive(Clone, Default)]
pub struct BindingState {
    pub address: Option<SocketAddr>,
    pub error: Option<String>,
}

pub struct RemoteForward {
    state: watch::Receiver<BindingState>,
    target: watch::Sender<String>,
    stop: oneshot::Sender<()>,
    task: JoinHandle<()>,
}

impl RemoteForward {
    pub fn start(
        transport: Arc<NativeTransport>,
        peer: String,
        listen: SocketAddr,
        target: String,
        streams: Arc<Semaphore>,
    ) -> Self {
        let (state_tx, state) = watch::channel(BindingState::default());
        let (target_tx, targets) = watch::channel(target);
        let (stop_tx, mut stop) = oneshot::channel();
        let task = tokio::spawn(async move {
            let mut delay = 1;
            loop {
                let session = tokio::select! {
                    biased;
                    _ = &mut stop => break,
                    result = RemoteSession::bind(&transport, &peer, listen) => result,
                };
                let error = match session {
                    Ok(mut session) => {
                        delay = 1;
                        state_tx.send_replace(BindingState {
                            address: Some(session.bound),
                            error: None,
                        });
                        tokio::select! {
                            biased;
                            _ = &mut stop => {
                                // The peer acknowledges the FIN after releasing the listening socket.
                                let _ = session.control_tx.finish();
                                if timeout(Duration::from_secs(2), session.control_rx.read_u8()).await.is_err() {
                                    session.conn.close(0u32.into(), b"listener stopped");
                                }
                                break;
                            }
                            result = session.forward(&targets, &streams) => match result { Err(e) => e, Ok(()) => break },
                        }
                    }
                    Err(error) => error,
                };
                state_tx.send_replace(BindingState {
                    address: None,
                    error: Some(format!("{error:#}")),
                });
                tokio::select! { _ = &mut stop => break, _ = sleep(Duration::from_secs(delay)) => {} }
                delay = (delay * 2).min(15);
            }
        });
        Self {
            state,
            target: target_tx,
            stop: stop_tx,
            task,
        }
    }

    pub fn state(&self) -> BindingState {
        self.state.borrow().clone()
    }
    pub fn set_target(&self, target: String) {
        self.target.send_replace(target);
    }
    pub async fn stop(self) {
        let _ = self.stop.send(());
        let _ = self.task.await;
    }
}

struct RemoteSession {
    conn: Connection,
    control_tx: SendStream,
    control_rx: RecvStream,
    bound: SocketAddr,
}

impl RemoteSession {
    async fn bind(transport: &NativeTransport, peer: &str, listen: SocketAddr) -> Result<Self> {
        let dest = transport.peer(peer)?;
        timeout(CONNECT_TIMEOUT, async {
            let conn = transport
                .endpoint
                .connect(dest.address.clone(), wire::PROTOCOL)
                .await?;
            let (mut tx, mut rx) = conn.open_bi().await?;
            let bound = wire::request(&mut tx, &mut rx, listen).await?;
            Ok(Self {
                conn,
                control_tx: tx,
                control_rx: rx,
                bound,
            })
        })
        .await
        .context("Remote listener request timed out")?
    }

    async fn forward(
        &mut self,
        targets: &watch::Receiver<String>,
        streams: &Arc<Semaphore>,
    ) -> Result<()> {
        loop {
            let (tx, mut rx) = tokio::select! {
                biased;
                _ = self.control_rx.read_u8() => bail!("Peer closed the listener"),
                streams = self.conn.accept_bi() => streams?,
            };
            let Ok(permit) = streams.clone().try_acquire_owned() else {
                continue;
            };
            let target = targets.borrow().clone();
            let conn = self.conn.clone();
            tokio::spawn(async move {
                let (_permit, _connection) = (permit, conn);
                let run = async {
                    ensure!(
                        timeout(CONNECT_TIMEOUT, rx.read_u8()).await?? == INCOMING,
                        "Invalid incoming stream"
                    );
                    let mut tcp =
                        timeout(Duration::from_secs(6), TcpStream::connect(&target)).await??;
                    relay(&mut tcp, rx, tx).await
                };
                if let Err(e) = run.await {
                    eprintln!("remote forward to {target}: {e:#}");
                }
            });
        }
    }
}
