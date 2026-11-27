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
    sync::{oneshot, watch},
    task::JoinHandle,
    time::{sleep, timeout},
};
use paxri_core::{
    PeerSession, StreamKind,
    reverse::{self as wire, ListenerId},
};
use paxri_native::gateway::{NativeSession, NativeTransport};

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
        let mut tcp = paxri_core::connect_tcp(&target).await?;
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
    pub fn start(
        transport: Arc<NativeTransport>,
        peer: String,
        listen: SocketAddr,
        target: String,
        routes: Routes,
    ) -> Self {
        let (state_tx, state) = watch::channel(BindingState::default());
        let (target_tx, targets) = watch::channel(target);
        let (stop_tx, mut stop) = oneshot::channel();
        let task = tokio::spawn(async move {
            let mut phases = transport.subscribe_phase_changes();
            let mut delay = 1;
            let mut initial_probe = true;
            loop {
                let session = match transport.online_session(&peer) {
                    Ok(session) => {
                        initial_probe = false;
                        session
                    }
                    Err(offline) => {
                        delay = 1;
                        let error = if std::mem::take(&mut initial_probe) {
                            match tokio::select! {
                                biased;
                                _ = &mut stop => break,
                                result = transport.probe_session(&peer) => result,
                            } {
                                Ok(session) => session,
                                Err(err) => {
                                    state_tx.send_replace(BindingState {
                                        address: None,
                                        error: Some(format!("{err:#}")),
                                    });
                                    tokio::select! {
                                        biased;
                                        _ = &mut stop => break,
                                        changed = phases.changed() => if changed.is_err() { break; },
                                    }
                                    continue;
                                }
                            }
                        } else {
                            state_tx.send_replace(BindingState {
                                address: None,
                                error: Some(format!("{offline:#}")),
                            });
                            tokio::select! {
                                biased;
                                _ = &mut stop => break,
                                changed = phases.changed() => if changed.is_err() { break; },
                            }
                            continue;
                        };
                        error
                    }
                };
                let bound = tokio::select! {
                    biased;
                    _ = &mut stop => break,
                    result = bind(session, listen, &routes, &targets) => result,
                };
                let error = match bound {
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
                                let _ = timeout(Duration::from_secs(2), control.read_u8()).await;
                                break;
                            }
                            _ = control.read_u8() => anyhow::anyhow!("Peer closed the listener"),
                        }
                    }
                    Err(error) => error,
                };
                if transport.online_session(&peer).is_err() {
                    continue;
                }
                state_tx.send_replace(BindingState {
                    address: None,
                    error: Some(format!("{error:#}")),
                });
                tokio::select! {
                    biased;
                    _ = &mut stop => break,
                    _ = sleep(Duration::from_secs(delay)) => {}
                    changed = phases.changed() => if changed.is_err() { break; },
                }
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

async fn bind(
    session: NativeSession,
    listen: SocketAddr,
    routes: &Routes,
    targets: &watch::Receiver<String>,
) -> Result<(
    <NativeSession as PeerSession>::Stream,
    SocketAddr,
    Registration,
)> {
    timeout(STREAM_TIMEOUT, async {
        let guard = routes.register(session.peer_id(), targets.clone())?;
        let mut stream = session.open(StreamKind::Listen).await?;
        let (mut rx, mut tx) = tokio::io::split(&mut stream);
        let bound = wire::request(&mut tx, &mut rx, listen, guard.id).await?;
        Ok((stream, bound, guard))
    })
    .await
    .context("Remote listener request timed out")?
}
