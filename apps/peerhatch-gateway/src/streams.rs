//! Paired application services over any backend that provides reliable duplex streams.
use crate::{
    reverse::{Listeners, Routes},
    terminal::Terminals,
};
use std::sync::Arc;
use tokio::{
    sync::{Semaphore, mpsc, watch},
    task::JoinSet,
};
use peerhatch_config::Config;
use peerhatch_core::{PeerSession, StreamKind, session::IncomingStream};

pub(super) fn is_paired(cfg: &Config, id: &str) -> bool {
    cfg.peers
        .values()
        .any(|p| p.endpoint_id.eq_ignore_ascii_case(id))
}

pub async fn serve<S: PeerSession>(
    mut incoming: mpsc::Receiver<IncomingStream<S>>,
    config: watch::Receiver<Arc<Config>>,
    routes: Routes,
) {
    let limit = Arc::new(Semaphore::new(256));
    let terminals = Arc::new(Terminals::new(config.clone()));
    let listeners = Arc::new(Listeners::new(config.clone(), limit.clone()));
    let mut tasks = JoinSet::new();
    loop {
        tokio::select! {
            _ = tasks.join_next(), if !tasks.is_empty() => {}
            msg = incoming.recv() => {
                let Some(IncomingStream {session, kind, stream}) = msg else { break };
                let Ok(permit) = limit.clone().try_acquire_owned() else { continue };
                let mut config = config.clone();
                let terminals = terminals.clone();
                let listeners = listeners.clone();
                let routes = routes.clone();
                tasks.spawn(async move {
                    let _permit = permit;
                    let peer = session.peer_id();
                    if !is_paired(&config.borrow(), &peer) {
                        return;
                    }
                    let result = match kind {
                        StreamKind::Services => peerhatch_core::serve(|| config.borrow().clone(), &peer, stream).await,
                        StreamKind::Terminal | StreamKind::Command => terminals.serve(peer.clone(), kind, stream).await,
                        StreamKind::Files => {
                            let allowed = crate::account::is_standard_user();
                            match allowed {
                                Ok(allowed) => tokio::select! {
                                    biased;
                                    _ = config.wait_for(|cfg| !is_paired(cfg, &peer)) => Ok(()),
                                    result = peerhatch_control::files::serve(stream, allowed) => result,
                                },
                                Err(e) => Err(e),
                            }
                        }
                        StreamKind::Listen => listeners.serve(session, stream).await,
                        StreamKind::ForwardedTcp => routes.forward(&peer, stream).await,
                    };
                    if let Err(e) = result {
                        eprintln!("{} stream from {peer}: {e:#}", kind.label());
                    }
                });
            }
        }
    }
}
