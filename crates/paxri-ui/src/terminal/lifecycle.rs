//! Event-driven hierarchical lifecycle state machine for peer connections and their sessions.
//!
//! Each peer owns one [`PeerLifecycle`] whose outer [`PeerConnectionState`] governs
//! every child [`SessionState`]. All transitions are driven exclusively through
//! [`LifecycleEvent`] and produce explicit [`LifecycleEffect`] values for the host UI
//! and runtime to execute.

use paxri_core::connection::ConnectionPhase;
use std::collections::BTreeMap;

/// Outer per-peer connection state that owns and constrains its child terminal sessions.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PeerConnectionState {
    #[default]
    Disconnected,
    Initializing,
    Connected {
        connection_id: u64,
    },
    Reconnecting,
    Stopping,
}

impl PeerConnectionState {
    pub fn from_phase(phase: ConnectionPhase, connection_id: Option<u64>) -> Self {
        match (phase, connection_id) {
            (ConnectionPhase::Connected, Some(connection_id)) => {
                Self::Connected { connection_id }
            }
            (ConnectionPhase::Connected, None) => Self::Disconnected,
            (ConnectionPhase::Initializing, _) => Self::Initializing,
            (ConnectionPhase::Reconnecting, _) => Self::Reconnecting,
            (ConnectionPhase::Stopping, _) => Self::Stopping,
            (ConnectionPhase::Disconnected, _) => Self::Disconnected,
        }
    }

    pub fn phase(self) -> ConnectionPhase {
        match self {
            Self::Disconnected => ConnectionPhase::Disconnected,
            Self::Initializing => ConnectionPhase::Initializing,
            Self::Connected { .. } => ConnectionPhase::Connected,
            Self::Reconnecting => ConnectionPhase::Reconnecting,
            Self::Stopping => ConnectionPhase::Stopping,
        }
    }

    pub fn connection_id(self) -> Option<u64> {
        match self {
            Self::Connected { connection_id } => Some(connection_id),
            _ => None,
        }
    }

    pub fn is_connected(self) -> bool {
        matches!(self, Self::Connected { .. })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SessionState {
    /// Creating a new host session (`Request::Create`); not yet confirmed on the host.
    Initializing,
    /// Attaching to a host-confirmed session (`Request::Attach`, or `Create` once confirmed).
    Attaching,
    /// Normal active bidirectional terminal session.
    Active,
    /// Host-confirmed session whose stream was lost and is re-attaching.
    Reconnecting,
    /// Explicitly stopping a host-confirmed session.
    Stopping,
    /// Terminal session ended (exited, rejected, canceled, or failed to initialize).
    Ended(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionAction {
    /// Abort local initialization without sending a remote stop (`Initializing`).
    Cancel,
    /// Ask the host to terminate the session (`Attaching`, `Active`, `Reconnecting`).
    End,
    /// Dismiss the local session entry (`Stopping`, `Ended`).
    Close,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OfflineTransition {
    None,
    Detach,
    Ended,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StopTransition {
    Dismiss,
    SendExit,
    AbortAndSendEnd,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StreamEnd {
    Exited(u32),
    Rejected(String),
    ConnectionReplaced,
    TimedOut,
    Disconnected(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EndTransition {
    Ignored,
    Ended,
    DismissAndRemoveHosted,
    RetryNow,
    RetryAfterBackoff,
}

impl SessionState {
    pub fn new(attach: bool) -> Self {
        if attach {
            Self::Attaching
        } else {
            Self::Initializing
        }
    }

    pub fn is_initializing(&self) -> bool {
        matches!(self, Self::Initializing)
    }

    pub fn is_active(&self) -> bool {
        matches!(self, Self::Active)
    }

    pub fn is_open(&self) -> bool {
        !matches!(self, Self::Ended(_))
    }

    pub fn is_ended(&self) -> bool {
        matches!(self, Self::Ended(_))
    }

    pub fn is_stopping(&self) -> bool {
        matches!(self, Self::Stopping)
    }

    pub fn is_reconnecting(&self) -> bool {
        matches!(self, Self::Reconnecting)
    }

    pub fn is_connecting(&self) -> bool {
        matches!(
            self,
            Self::Initializing | Self::Attaching | Self::Reconnecting
        )
    }

    /// Whether the remote gateway is known to host this session key.
    pub fn is_confirmed_on_host(&self) -> bool {
        matches!(
            self,
            Self::Attaching | Self::Active | Self::Reconnecting | Self::Stopping
        )
    }

    pub fn action(&self) -> SessionAction {
        match self {
            Self::Initializing => SessionAction::Cancel,
            Self::Attaching | Self::Active | Self::Reconnecting => SessionAction::End,
            Self::Stopping | Self::Ended(_) => SessionAction::Close,
        }
    }

    pub fn subtitle(&self, number: usize, connection: PeerConnectionState) -> String {
        match self {
            Self::Initializing => format!("Session {number} · Starting…"),
            Self::Attaching => format!("Session {number} · Connecting…"),
            Self::Active => format!("Session {number}"),
            Self::Reconnecting => match connection {
                PeerConnectionState::Connected { .. } => {
                    format!("Session {number} · Reconnecting…")
                }
                PeerConnectionState::Initializing | PeerConnectionState::Reconnecting => {
                    format!("Session {number} · Reconnecting device…")
                }
                PeerConnectionState::Stopping => format!("Session {number} · Connection stopping"),
                PeerConnectionState::Disconnected => format!("Session {number} · Device offline"),
            },
            Self::Stopping => match connection {
                PeerConnectionState::Connected { .. } => format!("Session {number} · Stopping…"),
                _ => format!("Session {number} · Device offline · Stop pending"),
            },
            Self::Ended(reason) => format!("Session {number} · {reason}"),
        }
    }

    /// Record that the host accepted session creation and now owns the PTY.
    pub fn on_host_confirmed(&mut self) -> bool {
        if matches!(self, Self::Initializing) {
            *self = Self::Attaching;
            true
        } else {
            false
        }
    }

    /// Record that bidirectional I/O on the terminal stream is ready.
    pub fn on_connected(&mut self) -> bool {
        if matches!(
            self,
            Self::Initializing | Self::Attaching | Self::Reconnecting
        ) {
            *self = Self::Active;
            true
        } else {
            false
        }
    }

    /// Transition when the peer's shared connection leaves `Connected`.
    pub fn on_connection_lost(&mut self, reason: &str) -> OfflineTransition {
        match self {
            Self::Initializing => {
                *self = Self::Ended(reason.to_owned());
                OfflineTransition::Ended
            }
            Self::Attaching | Self::Active | Self::Reconnecting => {
                *self = Self::Reconnecting;
                OfflineTransition::Detach
            }
            Self::Stopping => OfflineTransition::Detach,
            Self::Ended(_) => OfflineTransition::None,
        }
    }

    /// Transition when the user requests cancelling, ending, or closing the session.
    pub fn on_user_stop(&mut self) -> StopTransition {
        match self {
            Self::Initializing => {
                *self = Self::Ended("Canceled".into());
                StopTransition::Dismiss
            }
            Self::Active => {
                *self = Self::Stopping;
                StopTransition::SendExit
            }
            Self::Attaching | Self::Reconnecting => {
                *self = Self::Stopping;
                StopTransition::AbortAndSendEnd
            }
            Self::Stopping | Self::Ended(_) => StopTransition::Dismiss,
        }
    }

    /// Transition when the in-flight stream or control task ends.
    pub fn on_stream_end(&mut self, end: StreamEnd, peer_online: bool) -> EndTransition {
        match self {
            Self::Ended(_) => EndTransition::Ignored,
            Self::Initializing => {
                let reason = match end {
                    StreamEnd::Exited(code) => format!("Exited · {code}"),
                    StreamEnd::Rejected(reason) => reason,
                    StreamEnd::ConnectionReplaced => {
                        if peer_online {
                            "Connection lost while starting".into()
                        } else {
                            "Device offline".into()
                        }
                    }
                    StreamEnd::TimedOut => {
                        if peer_online {
                            "Connection timed out".into()
                        } else {
                            "Device offline".into()
                        }
                    }
                    StreamEnd::Disconnected(reason) => {
                        if peer_online {
                            reason
                        } else {
                            "Device offline".into()
                        }
                    }
                };
                *self = Self::Ended(reason);
                EndTransition::Ended
            }
            Self::Stopping => match end {
                StreamEnd::Exited(_) => EndTransition::DismissAndRemoveHosted,
                StreamEnd::Rejected(reason) => {
                    *self = Self::Ended(reason);
                    EndTransition::Ended
                }
                StreamEnd::ConnectionReplaced if peer_online => EndTransition::RetryNow,
                StreamEnd::ConnectionReplaced
                | StreamEnd::TimedOut
                | StreamEnd::Disconnected(_) => EndTransition::RetryAfterBackoff,
            },
            Self::Attaching | Self::Active | Self::Reconnecting => match end {
                StreamEnd::Exited(code) => {
                    *self = Self::Ended(format!("Exited · {code}"));
                    EndTransition::Ended
                }
                StreamEnd::Rejected(reason) => {
                    *self = Self::Ended(reason);
                    EndTransition::Ended
                }
                StreamEnd::ConnectionReplaced if peer_online => {
                    *self = Self::Reconnecting;
                    EndTransition::RetryNow
                }
                StreamEnd::ConnectionReplaced
                | StreamEnd::TimedOut
                | StreamEnd::Disconnected(_) => {
                    *self = Self::Reconnecting;
                    EndTransition::RetryAfterBackoff
                }
            },
        }
    }
}

/// Child session state owned inside [`PeerLifecycle`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ManagedSession {
    pub state: SessionState,
    pub generation: u64,
    /// The parent transport connection instance this session's stream is bound to.
    /// Always `None` unless the parent connection is `Connected { connection_id }`.
    pub bound_connection: Option<u64>,
    pub in_flight: bool,
    pub retry_ready: bool,
}

/// Unified event input for [`PeerLifecycle`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LifecycleEvent {
    /// Outer transport connection state changed.
    ConnectionChanged(PeerConnectionState),
    /// User opened a new (`attach = false`) or existing (`attach = true`) session.
    OpenSession { id: u64, attach: bool },
    /// Host confirmed `Request::Create` succeeded (PTY exists on host).
    HostConfirmed { id: u64, generation: u64 },
    /// Bidirectional stream & initial probe ready.
    SessionConnected { id: u64, generation: u64 },
    /// In-flight stream or control task finished.
    StreamEnded {
        id: u64,
        generation: u64,
        end: StreamEnd,
    },
    /// Host confirmed the session no longer exists.
    SessionGone { id: u64, generation: u64 },
    /// User tapped Cancel / End / Close.
    UserStop { id: u64 },
    /// User explicitly dismissed a local session entry.
    Dismiss { id: u64 },
    /// User focused / re-entered a terminal view.
    SessionFocused { id: u64, stalled: bool },
    /// Network interfaces refreshed or app resumed.
    NetworkRefreshed,
    /// Backoff retry timer fired for a session.
    RetryTimerElapsed { id: u64 },
    /// Dispatch any pending reconnect/stop tasks while visible.
    DispatchPending {
        active_session: Option<u64>,
        visible: bool,
    },
    /// Peer was unpaired or removed.
    PeerRemoved,
}

/// Explicit side-effect produced by [`PeerLifecycle::handle`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LifecycleEffect {
    SpawnCreate {
        id: u64,
        generation: u64,
        connection_id: u64,
    },
    SpawnAttach {
        id: u64,
        generation: u64,
        connection_id: u64,
    },
    SpawnEnd {
        id: u64,
        generation: u64,
        connection_id: u64,
    },
    SendExit {
        id: u64,
    },
    ProbeStream {
        id: u64,
    },
    AbortJob {
        id: u64,
    },
    ClearComposing {
        id: u64,
    },
    StartInput {
        id: u64,
    },
    StopInput {
        id: u64,
    },
    ScheduleRetry {
        id: u64,
    },
    DismissSession {
        id: u64,
        remove_hosted: bool,
        toast_gone: bool,
    },
    SyncDefaults,
    DispatchPending,
    RefreshPresence,
}

/// Hierarchical per-peer state machine: outer [`PeerConnectionState`] owns and constrains
/// all child [`ManagedSession`] instances.
#[derive(Clone, Debug, Default)]
pub struct PeerLifecycle {
    connection: PeerConnectionState,
    sessions: BTreeMap<u64, ManagedSession>,
}

impl PeerLifecycle {
    pub fn connection(&self) -> PeerConnectionState {
        self.connection
    }

    pub fn session(&self, id: u64) -> Option<&ManagedSession> {
        self.sessions.get(&id)
    }

    pub fn session_state(&self, id: u64) -> Option<&SessionState> {
        self.sessions.get(&id).map(|s| &s.state)
    }

    pub fn sessions(&self) -> &BTreeMap<u64, ManagedSession> {
        &self.sessions
    }

    pub fn has_open_sessions(&self) -> bool {
        self.sessions.values().any(|s| s.state.is_open())
    }

    pub fn handle(&mut self, event: LifecycleEvent) -> Vec<LifecycleEffect> {
        let mut effects = Vec::new();
        match event {
            LifecycleEvent::ConnectionChanged(next) => {
                if next == self.connection {
                    return effects;
                }
                let prev = self.connection;
                self.connection = next;
                match next {
                    PeerConnectionState::Disconnected
                    | PeerConnectionState::Initializing
                    | PeerConnectionState::Reconnecting
                    | PeerConnectionState::Stopping => {
                        let reason = match next {
                            PeerConnectionState::Stopping => "Connection stopping",
                            PeerConnectionState::Reconnecting => "Connection lost while starting",
                            _ => "Device offline",
                        };
                        for (&id, session) in &mut self.sessions {
                            let had_job = session.in_flight || session.bound_connection.is_some();
                            session.in_flight = false;
                            session.bound_connection = None;
                            match session.state.on_connection_lost(reason) {
                                OfflineTransition::Ended => {
                                    session.retry_ready = false;
                                    session.generation += 1;
                                    effects.push(LifecycleEffect::AbortJob { id });
                                    effects.push(LifecycleEffect::StopInput { id });
                                }
                                OfflineTransition::Detach => {
                                    session.retry_ready = true;
                                    if had_job {
                                        session.generation += 1;
                                        effects.push(LifecycleEffect::AbortJob { id });
                                    }
                                }
                                OfflineTransition::None => {}
                            }
                        }
                    }
                    PeerConnectionState::Connected { connection_id } => {
                        let replaced_old = prev.connection_id().filter(|&old| old != connection_id);
                        let mut needs_dispatch = false;
                        for (&id, session) in &mut self.sessions {
                            if replaced_old.is_some()
                                && session.bound_connection == replaced_old
                            {
                                session.in_flight = false;
                                session.bound_connection = None;
                                session.generation += 1;
                                match session.state.on_connection_lost("Connection lost while starting") {
                                    OfflineTransition::Ended => {
                                        session.retry_ready = false;
                                        effects.push(LifecycleEffect::AbortJob { id });
                                        effects.push(LifecycleEffect::StopInput { id });
                                    }
                                    OfflineTransition::Detach => {
                                        session.retry_ready = true;
                                        needs_dispatch = true;
                                        effects.push(LifecycleEffect::AbortJob { id });
                                    }
                                    OfflineTransition::None => {}
                                }
                            } else if !session.in_flight
                                && (session.state.is_reconnecting() || session.state.is_stopping())
                            {
                                session.retry_ready = true;
                                needs_dispatch = true;
                            }
                        }
                        if needs_dispatch {
                            effects.push(LifecycleEffect::DispatchPending);
                        }
                    }
                }
            }
            LifecycleEvent::OpenSession { id, attach } => {
                let Some(connection_id) = self.connection.connection_id() else {
                    return effects;
                };
                self.sessions.insert(
                    id,
                    ManagedSession {
                        state: SessionState::new(attach),
                        generation: 0,
                        bound_connection: Some(connection_id),
                        in_flight: true,
                        retry_ready: false,
                    },
                );
                if attach {
                    effects.push(LifecycleEffect::SpawnAttach {
                        id,
                        generation: 0,
                        connection_id,
                    });
                } else {
                    effects.push(LifecycleEffect::SpawnCreate {
                        id,
                        generation: 0,
                        connection_id,
                    });
                }
                effects.push(LifecycleEffect::SyncDefaults);
            }
            LifecycleEvent::HostConfirmed { id, generation } => {
                if let Some(session) = self.sessions.get_mut(&id)
                    && session.generation == generation
                    && session.state.on_host_confirmed()
                {
                    effects.push(LifecycleEffect::SyncDefaults);
                }
            }
            LifecycleEvent::SessionConnected { id, generation } => {
                let current_conn = self.connection.connection_id();
                if let Some(session) = self.sessions.get_mut(&id)
                    && session.generation == generation
                    && current_conn.is_some()
                    && session.bound_connection == current_conn
                    && session.state.on_connected()
                {
                    effects.push(LifecycleEffect::StartInput { id });
                    effects.push(LifecycleEffect::SyncDefaults);
                    effects.push(LifecycleEffect::DispatchPending);
                }
            }
            LifecycleEvent::StreamEnded {
                id,
                generation,
                end,
            } => {
                let Some(session) = self.sessions.get_mut(&id) else {
                    return effects;
                };
                if session.generation != generation {
                    return effects;
                }
                session.in_flight = false;
                session.bound_connection = None;
                let online = self.connection.is_connected();
                match session.state.on_stream_end(end, online) {
                    EndTransition::Ignored => {}
                    EndTransition::DismissAndRemoveHosted => {
                        self.sessions.remove(&id);
                        effects.push(LifecycleEffect::DismissSession {
                            id,
                            remove_hosted: true,
                            toast_gone: false,
                        });
                    }
                    EndTransition::Ended => {
                        session.retry_ready = false;
                        effects.push(LifecycleEffect::ClearComposing { id });
                        effects.push(LifecycleEffect::StopInput { id });
                    }
                    EndTransition::RetryNow => {
                        session.retry_ready = true;
                        effects.push(LifecycleEffect::ClearComposing { id });
                        effects.push(LifecycleEffect::DispatchPending);
                    }
                    EndTransition::RetryAfterBackoff => {
                        session.retry_ready = false;
                        effects.push(LifecycleEffect::ClearComposing { id });
                        effects.push(LifecycleEffect::ScheduleRetry { id });
                    }
                }
            }
            LifecycleEvent::SessionGone { id, generation } => {
                let Some(session) = self.sessions.get(&id) else {
                    return effects;
                };
                if session.generation != generation {
                    return effects;
                }
                let toast_gone = !session.state.is_stopping();
                self.sessions.remove(&id);
                effects.push(LifecycleEffect::DismissSession {
                    id,
                    remove_hosted: true,
                    toast_gone,
                });
            }
            LifecycleEvent::UserStop { id } => {
                let Some(session) = self.sessions.get_mut(&id) else {
                    return effects;
                };
                match session.state.on_user_stop() {
                    StopTransition::Dismiss => {
                        self.sessions.remove(&id);
                        effects.push(LifecycleEffect::AbortJob { id });
                        effects.push(LifecycleEffect::DismissSession {
                            id,
                            remove_hosted: false,
                            toast_gone: false,
                        });
                    }
                    StopTransition::SendExit => {
                        effects.push(LifecycleEffect::SendExit { id });
                        effects.push(LifecycleEffect::DispatchPending);
                    }
                    StopTransition::AbortAndSendEnd => {
                        session.in_flight = false;
                        session.bound_connection = None;
                        session.retry_ready = true;
                        session.generation += 1;
                        effects.push(LifecycleEffect::AbortJob { id });
                        effects.push(LifecycleEffect::DispatchPending);
                    }
                }
            }
            LifecycleEvent::Dismiss { id } => {
                if self.sessions.remove(&id).is_some() {
                    effects.push(LifecycleEffect::AbortJob { id });
                    effects.push(LifecycleEffect::DismissSession {
                        id,
                        remove_hosted: false,
                        toast_gone: false,
                    });
                }
            }
            LifecycleEvent::SessionFocused { id, stalled } => {
                let online = self.connection.is_connected();
                let Some(session) = self.sessions.get_mut(&id) else {
                    return effects;
                };
                if session.state.is_active() {
                    effects.push(LifecycleEffect::StartInput { id });
                    effects.push(LifecycleEffect::ProbeStream { id });
                } else if session.state.is_initializing() {
                    if !online {
                        session.state = SessionState::Ended("Device offline".into());
                        session.in_flight = false;
                        session.bound_connection = None;
                        session.retry_ready = false;
                        session.generation += 1;
                        effects.push(LifecycleEffect::AbortJob { id });
                        effects.push(LifecycleEffect::StopInput { id });
                    }
                } else if matches!(
                    session.state,
                    SessionState::Attaching | SessionState::Reconnecting
                ) {
                    if session.in_flight && stalled {
                        session.state = SessionState::Reconnecting;
                        session.in_flight = false;
                        session.bound_connection = None;
                        session.retry_ready = true;
                        session.generation += 1;
                        effects.push(LifecycleEffect::AbortJob { id });
                        effects.push(LifecycleEffect::DispatchPending);
                    } else if !session.in_flight {
                        session.retry_ready = true;
                        effects.push(LifecycleEffect::DispatchPending);
                    }
                    if !online {
                        effects.push(LifecycleEffect::RefreshPresence);
                    }
                } else if session.state.is_stopping() {
                    if !session.in_flight {
                        session.retry_ready = true;
                        effects.push(LifecycleEffect::DispatchPending);
                    }
                    if !online {
                        effects.push(LifecycleEffect::RefreshPresence);
                    }
                }
            }
            LifecycleEvent::NetworkRefreshed => {
                let mut needs_dispatch = false;
                for (&id, session) in &mut self.sessions {
                    if session.state.is_active() {
                        effects.push(LifecycleEffect::ProbeStream { id });
                    } else if session.state.is_reconnecting() || session.state.is_stopping() {
                        session.retry_ready = true;
                        needs_dispatch = true;
                    }
                }
                if needs_dispatch {
                    effects.push(LifecycleEffect::DispatchPending);
                }
            }
            LifecycleEvent::RetryTimerElapsed { id } => {
                if let Some(session) = self.sessions.get_mut(&id)
                    && (session.state.is_reconnecting() || session.state.is_stopping())
                {
                    session.retry_ready = true;
                    effects.push(LifecycleEffect::DispatchPending);
                }
            }
            LifecycleEvent::DispatchPending {
                active_session,
                visible,
            } => {
                if !visible {
                    return effects;
                }
                let Some(connection_id) = self.connection.connection_id() else {
                    return effects;
                };
                let defer_background_reconnect = active_session
                    .and_then(|id| self.sessions.get(&id))
                    .is_some_and(|s| s.state.is_connecting());
                let mut order: Vec<u64> = Vec::with_capacity(self.sessions.len());
                if let Some(id) = active_session
                    && self.sessions.contains_key(&id)
                {
                    order.push(id);
                }
                for &id in self.sessions.keys() {
                    if Some(id) != active_session {
                        order.push(id);
                    }
                }
                for id in order {
                    let Some(session) = self.sessions.get_mut(&id) else {
                        continue;
                    };
                    if session.in_flight || !session.retry_ready {
                        continue;
                    }
                    match session.state {
                        SessionState::Reconnecting => {
                            if defer_background_reconnect && Some(id) != active_session {
                                continue;
                            }
                            session.in_flight = true;
                            session.retry_ready = false;
                            session.bound_connection = Some(connection_id);
                            effects.push(LifecycleEffect::SpawnAttach {
                                id,
                                generation: session.generation,
                                connection_id,
                            });
                        }
                        SessionState::Stopping => {
                            session.in_flight = true;
                            session.retry_ready = false;
                            session.bound_connection = Some(connection_id);
                            effects.push(LifecycleEffect::SpawnEnd {
                                id,
                                generation: session.generation,
                                connection_id,
                            });
                        }
                        _ => {}
                    }
                }
            }
            LifecycleEvent::PeerRemoved => {
                self.connection = PeerConnectionState::Stopping;
                let ids: Vec<u64> = self.sessions.keys().copied().collect();
                self.sessions.clear();
                for id in ids {
                    effects.push(LifecycleEffect::AbortJob { id });
                    effects.push(LifecycleEffect::DismissSession {
                        id,
                        remove_hosted: false,
                        toast_gone: false,
                    });
                }
            }
        }
        effects
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_session_is_blocked_unless_parent_connection_is_connected() {
        let mut peer = PeerLifecycle::default();
        for phase in [
            PeerConnectionState::Disconnected,
            PeerConnectionState::Initializing,
            PeerConnectionState::Reconnecting,
            PeerConnectionState::Stopping,
        ] {
            let _ = peer.handle(LifecycleEvent::ConnectionChanged(phase));
            assert!(
                peer.handle(LifecycleEvent::OpenSession {
                    id: 1,
                    attach: false
                })
                .is_empty()
            );
            assert!(peer.session(1).is_none());
        }
    }

    #[test]
    fn connection_loss_drives_child_sessions_according_to_their_phase() {
        let mut peer = PeerLifecycle::default();
        peer.handle(LifecycleEvent::ConnectionChanged(
            PeerConnectionState::Connected { connection_id: 10 },
        ));
        // Session 1: unconfirmed Initializing
        peer.handle(LifecycleEvent::OpenSession {
            id: 1,
            attach: false,
        });
        // Session 2: confirmed Active
        peer.handle(LifecycleEvent::OpenSession {
            id: 2,
            attach: false,
        });
        peer.handle(LifecycleEvent::HostConfirmed {
            id: 2,
            generation: 0,
        });
        peer.handle(LifecycleEvent::SessionConnected {
            id: 2,
            generation: 0,
        });
        // Session 3: confirmed Stopping
        peer.handle(LifecycleEvent::OpenSession {
            id: 3,
            attach: true,
        });
        peer.handle(LifecycleEvent::SessionConnected {
            id: 3,
            generation: 0,
        });
        peer.handle(LifecycleEvent::UserStop { id: 3 });

        let effects = peer.handle(LifecycleEvent::ConnectionChanged(
            PeerConnectionState::Disconnected,
        ));
        assert!(effects.contains(&LifecycleEffect::AbortJob { id: 1 }));
        assert!(effects.contains(&LifecycleEffect::StopInput { id: 1 }));
        assert!(effects.contains(&LifecycleEffect::AbortJob { id: 2 }));
        assert!(effects.contains(&LifecycleEffect::AbortJob { id: 3 }));

        assert_eq!(
            peer.session_state(1),
            Some(&SessionState::Ended("Device offline".into()))
        );
        assert_eq!(peer.session_state(2), Some(&SessionState::Reconnecting));
        assert_eq!(peer.session_state(3), Some(&SessionState::Stopping));
        // No session may remain bound to a dead connection.
        assert!(peer.sessions().values().all(|s| s.bound_connection.is_none()));
        assert!(peer.sessions().values().all(|s| !s.in_flight));

        // Stale event from generation 0 is ignored.
        assert!(
            peer.handle(LifecycleEvent::SessionConnected {
                id: 2,
                generation: 0
            })
            .is_empty()
        );
        assert_eq!(peer.session_state(2), Some(&SessionState::Reconnecting));

        // Stopping session can always be dismissed locally while offline.
        let stop_effects = peer.handle(LifecycleEvent::UserStop { id: 3 });
        assert!(stop_effects.contains(&LifecycleEffect::DismissSession {
            id: 3,
            remove_hosted: false,
            toast_gone: false,
        }));
        assert!(peer.session(3).is_none());
    }

    #[test]
    fn connection_replacement_reattaches_confirmed_and_ends_initializing() {
        let mut peer = PeerLifecycle::default();
        peer.handle(LifecycleEvent::ConnectionChanged(
            PeerConnectionState::Connected { connection_id: 1 },
        ));
        peer.handle(LifecycleEvent::OpenSession {
            id: 1,
            attach: false,
        });
        peer.handle(LifecycleEvent::OpenSession {
            id: 2,
            attach: true,
        });
        peer.handle(LifecycleEvent::SessionConnected {
            id: 2,
            generation: 0,
        });

        let effects = peer.handle(LifecycleEvent::ConnectionChanged(
            PeerConnectionState::Connected { connection_id: 2 },
        ));
        assert!(effects.contains(&LifecycleEffect::AbortJob { id: 1 }));
        assert!(effects.contains(&LifecycleEffect::AbortJob { id: 2 }));
        assert!(effects.contains(&LifecycleEffect::DispatchPending));
        assert_eq!(
            peer.session_state(1),
            Some(&SessionState::Ended("Connection lost while starting".into()))
        );
        assert_eq!(peer.session_state(2), Some(&SessionState::Reconnecting));

        let dispatched = peer.handle(LifecycleEvent::DispatchPending {
            active_session: Some(2),
            visible: true,
        });
        assert_eq!(
            dispatched,
            vec![LifecycleEffect::SpawnAttach {
                id: 2,
                generation: 1,
                connection_id: 2,
            }]
        );
        assert_eq!(peer.session(2).unwrap().bound_connection, Some(2));
    }

    #[test]
    fn foreground_session_takes_priority_during_dispatch() {
        let mut peer = PeerLifecycle::default();
        peer.handle(LifecycleEvent::ConnectionChanged(
            PeerConnectionState::Connected { connection_id: 1 },
        ));
        for id in [1, 2] {
            peer.handle(LifecycleEvent::OpenSession { id, attach: true });
            peer.handle(LifecycleEvent::SessionConnected { id, generation: 0 });
        }
        peer.handle(LifecycleEvent::ConnectionChanged(
            PeerConnectionState::Disconnected,
        ));
        peer.handle(LifecycleEvent::ConnectionChanged(
            PeerConnectionState::Connected { connection_id: 2 },
        ));

        let effects = peer.handle(LifecycleEvent::DispatchPending {
            active_session: Some(2),
            visible: true,
        });
        assert_eq!(
            effects,
            vec![LifecycleEffect::SpawnAttach {
                id: 2,
                generation: 1,
                connection_id: 2,
            }]
        );
        // Once foreground session 2 becomes Active, background session 1 is dispatched.
        let ready = peer.handle(LifecycleEvent::SessionConnected {
            id: 2,
            generation: 1,
        });
        assert!(ready.contains(&LifecycleEffect::DispatchPending));
        let bg = peer.handle(LifecycleEvent::DispatchPending {
            active_session: Some(2),
            visible: true,
        });
        assert_eq!(
            bg,
            vec![LifecycleEffect::SpawnAttach {
                id: 1,
                generation: 1,
                connection_id: 2,
            }]
        );
    }

    #[test]
    fn user_stop_cancels_initializing_and_aborts_stuck_attaching() {
        let mut peer = PeerLifecycle::default();
        peer.handle(LifecycleEvent::ConnectionChanged(
            PeerConnectionState::Connected { connection_id: 5 },
        ));
        peer.handle(LifecycleEvent::OpenSession {
            id: 1,
            attach: false,
        });
        let cancel = peer.handle(LifecycleEvent::UserStop { id: 1 });
        assert!(cancel.contains(&LifecycleEffect::AbortJob { id: 1 }));
        assert!(cancel.contains(&LifecycleEffect::DismissSession {
            id: 1,
            remove_hosted: false,
            toast_gone: false,
        }));
        assert!(peer.session(1).is_none());

        peer.handle(LifecycleEvent::OpenSession {
            id: 2,
            attach: true,
        });
        let stop = peer.handle(LifecycleEvent::UserStop { id: 2 });
        assert!(stop.contains(&LifecycleEffect::AbortJob { id: 2 }));
        assert!(stop.contains(&LifecycleEffect::DispatchPending));
        assert_eq!(peer.session_state(2), Some(&SessionState::Stopping));
        let end = peer.handle(LifecycleEvent::DispatchPending {
            active_session: Some(2),
            visible: true,
        });
        assert_eq!(
            end,
            vec![LifecycleEffect::SpawnEnd {
                id: 2,
                generation: 1,
                connection_id: 5,
            }]
        );
    }
}
