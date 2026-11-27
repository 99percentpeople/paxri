//! Paired application services over any backend that provides reliable duplex streams.
use crate::{
    reverse::Routes,
    terminal::{SessionTools, Terminals},
};
use std::sync::Arc;
use tokio::{
    sync::{Semaphore, mpsc, watch},
    task::JoinSet,
};
use paxri_config::Config;
use paxri_core::{reverse::Listeners, PeerSession, StreamKind, session::IncomingStream};

pub(super) fn is_paired(cfg: &Config, id: &str) -> bool {
    cfg.peers
        .values()
        .any(|p| p.endpoint_id.eq_ignore_ascii_case(id))
}

pub async fn serve<S: PeerSession>(
    mut incoming: mpsc::Receiver<IncomingStream<S>>,
    config: watch::Receiver<Arc<Config>>,
    routes: Routes,
    gateway: Arc<tokio::sync::Mutex<crate::Gateway>>,
    tools: SessionTools,
) {
    let (transport, keep_alive, limits) = {
        let state = gateway.lock().await;
        (
            state.transport.clone(),
            state.session_keep_alive.subscribe(),
            state.config.gateway.limits,
        )
    };
    let limit = Arc::new(Semaphore::new(limits.max_concurrent_streams));
    let terminals = Arc::new(Terminals::new(
        config.clone(),
        tools,
        limits.max_terminal_sessions,
    ));
    let managed = Arc::new(crate::terminal::managed::Registry::new(transport, keep_alive));
    let listeners = Arc::new(Listeners::with_limit(
        config.clone(),
        limit.clone(),
        limits.max_reverse_listeners,
    ));
    let mut tasks = JoinSet::new();
    loop {
        tokio::select! {
            _ = tasks.join_next(), if !tasks.is_empty() => {}
            msg = incoming.recv() => {
                let Some(IncomingStream {session, kind, stream}) = msg else { break };
                let Ok(permit) = limit.clone().try_acquire_owned() else { continue };
                let mut config = config.clone();
                let terminals = terminals.clone();
                let managed = managed.clone();
                let listeners = listeners.clone();
                let routes = routes.clone();
                let gateway = gateway.clone();
                tasks.spawn(async move {
                    let _permit = permit;
                    let peer = session.peer_id();
                    if !is_paired(&config.borrow(), &peer) {
                        return;
                    }
                    let result = match kind {
                        StreamKind::Services => paxri_core::serve(|| config.borrow().clone(), &peer, stream).await,
                        StreamKind::ManagedTerminal => managed.serve(&terminals, peer.clone(), stream).await,
                        StreamKind::Terminal => Err(anyhow::anyhow!("Interactive shells require managed terminals")),
                        StreamKind::Command | StreamKind::Pipe => terminals.serve(peer.clone(), kind, stream).await,
                        StreamKind::Files => {
                            let allowed = crate::account::is_standard_user();
                            match allowed {
                                Ok(allowed) => tokio::select! {
                                    biased;
                                    _ = config.wait_for(|cfg| !is_paired(cfg, &peer)) => Ok(()),
                                    result = paxri_control::files::serve(stream, allowed) => result,
                                },
                                Err(e) => Err(e),
                            }
                        }
                        StreamKind::ForwardRules => crate::forward_management::serve(gateway, &peer, stream).await,
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
