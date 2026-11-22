use crate::STREAM_TIMEOUT;
use anyhow::{Context, Result, ensure};
use std::{
    collections::BTreeMap,
    net::SocketAddr,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, copy_bidirectional},
    net::{TcpListener, TcpStream},
    sync::{Semaphore, oneshot, watch},
    task::{JoinHandle, JoinSet},
    time::{sleep, timeout},
};
use peerhatch_config::Config;
use peerhatch_core::{
    PeerSession, StreamKind, Transport,
    reverse::{self as wire, ListenerId},
};

type Target = (String, watch::Receiver<String>);
#[derive(Clone, Default)]
pub(crate) struct Routes(Arc<Mutex<BTreeMap<ListenerId, Target>>>);

struct Registration {
    id: ListenerId,
    routes: Routes,
}
impl Drop for Registration {
    fn drop(&mut self) {
        self.routes
            .0
            .lock()
            .expect("reverse routes")
            .remove(&self.id);
    }
}
impl Routes {
    fn register(&self, peer: String, targets: watch::Receiver<String>) -> Result<Registration> {
        let mut id = [0; 16];
        getrandom::fill(&mut id)
            .map_err(|e| anyhow::anyhow!("generate remote listener ID: {e}"))?;
        let mut routes = self.0.lock().expect("reverse routes");
        ensure!(!routes.contains_key(&id), "duplicate remote listener ID");
        routes.insert(id, (peer, targets));
        Ok(Registration {
            id,
            routes: self.clone(),
        })
    }
    pub async fn forward(
        &self,
        peer: &str,
        mut stream: impl AsyncRead + AsyncWrite + Unpin,
    ) -> Result<()> {
        let mut id = [0; 16];
        timeout(STREAM_TIMEOUT, stream.read_exact(&mut id)).await??;
        let target = self
            .0
            .lock()
            .expect("reverse routes")
            .get(&id)
            .filter(|(id, _)| id == peer)
            .map(|(_, target)| target.borrow().clone())
            .context("peer has no active reverse listener with this ID")?;
        let mut tcp = timeout(Duration::from_secs(6), TcpStream::connect(&target)).await??;
        copy_bidirectional(&mut tcp, &mut stream).await?;
        Ok(())
    }
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

    pub async fn serve<S: PeerSession>(&self, session: S, stream: S::Stream) -> Result<()> {
        let peer = session.peer_id();
        let mut config = self.config.clone();
        let (mut recv, mut send) = tokio::io::split(stream);
        let (listen, id) = wire::receive(&mut recv).await?;
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
                wire::respond(&mut send, Err(format!("{e:#}"))).await?;
                send.shutdown().await?;
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
    pub fn start<T: Transport>(
        transport: Arc<T>,
        peer: String,
        listen: SocketAddr,
        target: String,
        routes: Routes,
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
                    result = bind(&*transport, &peer, listen, &routes, &targets) => result,
                };
                let error = match session {
                    Ok((mut control, bound, _registration)) => {
                        delay = 1;
                        state_tx.send_replace(BindingState {
                            address: Some(bound),
                            error: None,
                        });
                        tokio::select! {
                            biased;
                            _ = &mut stop => {
                                // The peer acknowledges the FIN after releasing the listening socket.
                                let _ = control.shutdown().await;
                                let _ = timeout(Duration::from_secs(2),control.read_u8()).await;
                                break;
                            }
                            _ = control.read_u8() => anyhow::anyhow!("Peer closed the listener"),
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

async fn bind<T: Transport>(
    transport: &T,
    peer: &str,
    listen: SocketAddr,
    routes: &Routes,
    targets: &watch::Receiver<String>,
) -> Result<(
    <T::Session as PeerSession>::Stream,
    SocketAddr,
    Registration,
)> {
    timeout(STREAM_TIMEOUT, async {
        let session = transport.session(peer).await?;
        let guard = routes.register(session.peer_id(), targets.clone())?;
        let mut stream = session.open(StreamKind::Listen).await?;
        let (mut rx, mut tx) = tokio::io::split(&mut stream);
        let bound = wire::request(&mut tx, &mut rx, listen, guard.id).await?;
        Ok((stream, bound, guard))
    })
    .await
    .context("Remote listener request timed out")?
}
