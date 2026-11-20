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
use tunnel_config::Config;
use tunnel_core::{PeerSession, StreamKind, session::IncomingStream};

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
                let config = config.clone();
                let terminals = terminals.clone();
                let listeners = listeners.clone();
                let routes = routes.clone();
                tasks.spawn(async move {
                    let _permit = permit;
                    let peer = session.peer_id();
                    if !config.borrow().peers.values().any(|p| p.endpoint_id.eq_ignore_ascii_case(&peer)) {
                        return;
                    }
                    let result = match kind {
                        StreamKind::Services => tunnel_core::serve(|| config.borrow().clone(), &peer, stream).await,
                        StreamKind::Terminal | StreamKind::Command => terminals.serve(peer.clone(), kind, stream).await,
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
