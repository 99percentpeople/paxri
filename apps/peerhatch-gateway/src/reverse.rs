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
    net::TcpStream,
    sync::{oneshot, watch},
    task::JoinHandle,
    time::{sleep, timeout},
};
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
