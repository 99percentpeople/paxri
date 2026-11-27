//! One authenticated application connection per identity. The smaller endpoint key serializes
//! admission, including when both peers dial simultaneously. Application streams start only
//! after selection; a redundant dial can never take a running terminal down with it.
use crate::{CONNECT_TIMEOUT, EndpointTicket};
use anyhow::{Context, Result};
use iroh::{
    Endpoint, EndpointAddr, EndpointId, TransportAddr,
    endpoint::{Connection, RecvStream, SendStream},
    protocol::{AcceptError, ProtocolHandler},
};
use paxri_config::{Config, DeviceInfo};
use paxri_core::{
    PeerSession, StreamKind,
    connection::{
        ConnectionEvent, ConnectionPhase, ConnectionTraffic, PathKind, PeerConnection, unix_time_ms,
    },
    session::{IncomingStream, PROTOCOL},
};
use std::{
    collections::BTreeMap,
    pin::Pin,
    sync::{Arc, Mutex},
    task::{Context as TaskContext, Poll},
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, Join, ReadBuf},
    sync::{Mutex as AsyncMutex, Semaphore, mpsc, watch},
    task::JoinSet,
    time::timeout,
};

#[derive(Clone, Debug)]
pub struct NativeSession {
    conn: Connection,
    token: [u8; 16],
    connected_at_ms: i64,
    activity: Arc<Mutex<BTreeMap<StreamKind, usize>>>,
}

/// The usage counter lives as long as the stream, including through a TCP half-close.
pub struct NativeStream {
    io: Join<RecvStream, SendStream>,
    session: NativeSession,
    kind: StreamKind,
}
impl Drop for NativeStream {
    fn drop(&mut self) {
        let mut streams = self.session.activity.lock().expect("session activity");
        if let Some(n) = streams.get_mut(&self.kind) {
            *n -= 1;
            if *n == 0 {
                streams.remove(&self.kind);
            }
        }
    }
}
impl AsyncRead for NativeStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut TaskContext<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.io).poll_read(cx, buf)
    }
}
impl AsyncWrite for NativeStream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut TaskContext<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        Pin::new(&mut self.io).poll_write(cx, buf)
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.io).poll_flush(cx)
    }
    fn poll_shutdown(
        mut self: Pin<&mut Self>,
        cx: &mut TaskContext<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.io).poll_shutdown(cx)
    }
}

impl NativeSession {
    pub fn id(&self) -> usize {
        self.conn.stable_id()
    }

    pub fn same_connection(&self, other: &Self) -> bool {
        self.token == other.token && self.conn.remote_id() == other.conn.remote_id()
    }

    pub async fn closed(&self) {
        self.conn.closed().await;
    }

    /// Retire this specific connection after a failed application-level round trip.
    pub fn close_unresponsive(&self) {
        self.conn.close(0u32.into(), b"peer is not responding");
    }
    pub(super) fn add_paths(&self, address: &mut EndpointAddr) {
        address.addrs.extend(
            self.conn
                .paths()
                .iter()
                .map(|path| path.remote_addr().clone()),
        );
    }

    fn stream(&self, kind: StreamKind, rx: RecvStream, tx: SendStream) -> NativeStream {
        if kind == StreamKind::Files {
            // Give queued terminal input and redraws precedence over bulk file bytes.
            let _ = tx.set_priority(-1);
        }
        // Keep activity reports readable by clients that predate pipe execution.
        let kind = match kind {
            StreamKind::Pipe => StreamKind::Command,
            StreamKind::ManagedTerminal => StreamKind::Terminal,
            other => other,
        };
        *self
            .activity
            .lock()
            .expect("session activity")
            .entry(kind)
            .or_default() += 1;
        NativeStream {
            io: tokio::io::join(rx, tx),
            session: self.clone(),
            kind,
        }
    }
    pub fn status(&self) -> PeerConnection {
        let paths = self.conn.paths();
        let (path, address, rtt_ms) = match paths.iter().find(|p| p.is_selected()) {
            Some(p) => {
                let (kind, address) = match p.remote_addr() {
                    TransportAddr::Ip(a) => (PathKind::Direct, a.to_string()),
                    TransportAddr::Relay(u) => (PathKind::Relay, u.to_string()),
                    other => (PathKind::Other, other.to_string()),
                };
                (kind, Some(address), Some(p.rtt().as_millis() as u64))
            }
            None => (PathKind::Selecting, None, None),
        };
        let stats = self.conn.stats();
        PeerConnection {
            protocol: "Shared connection".into(),
            connection_id: Some(self.conn.stable_id() as u64),
            path,
            address,
            rtt_ms,
            traffic: Some(ConnectionTraffic {
                connected_at_ms: self.connected_at_ms,
                sent_bytes: stats.udp_tx.bytes,
                received_bytes: stats.udp_rx.bytes,
            }),
            streams: self.activity.lock().expect("session activity").clone(),
        }
    }
}
impl PeerSession for NativeSession {
    type Stream = NativeStream;
    fn peer_id(&self) -> String {
        self.conn.remote_id().to_string()
    }
    async fn open(&self, kind: StreamKind) -> Result<NativeStream> {
        timeout(CONNECT_TIMEOUT, async {
            let (mut tx, rx) = self.conn.open_bi().await?;
            kind.write(&mut tx).await?;
            Ok(self.stream(kind, rx, tx))
        })
        .await
        .context("opening peer stream timed out")?
    }
}

#[derive(Clone, Debug, Default)]
enum SlotState {
    #[default]
    Disconnected,
    Initializing,
    Connected(NativeSession),
    Reconnecting,
    Stopping,
}

impl SlotState {
    fn phase(&self) -> ConnectionPhase {
        match self {
            Self::Disconnected => ConnectionPhase::Disconnected,
            Self::Initializing => ConnectionPhase::Initializing,
            Self::Connected(session) if session.conn.close_reason().is_none() => {
                ConnectionPhase::Connected
            }
            Self::Connected(_) => ConnectionPhase::Disconnected,
            Self::Reconnecting => ConnectionPhase::Reconnecting,
            Self::Stopping => ConnectionPhase::Stopping,
        }
    }

    fn live(&self) -> Option<NativeSession> {
        match self {
            Self::Connected(session) if session.conn.close_reason().is_none() => {
                Some(session.clone())
            }
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub(super) struct PhaseEvents(watch::Sender<u64>);

impl PhaseEvents {
    fn notify(&self) {
        self.0.send_modify(|seq| *seq = seq.wrapping_add(1));
    }

    pub(super) fn subscribe(&self) -> watch::Receiver<u64> {
        self.0.subscribe()
    }
}

#[derive(Debug, Default)]
struct Slot {
    state: watch::Sender<SlotState>,
    dial: AsyncMutex<()>,
    admission: AsyncMutex<()>,
}

struct PhaseGuard {
    slot: Arc<Slot>,
    expected: ConnectionPhase,
    phases: PhaseEvents,
}

impl Drop for PhaseGuard {
    fn drop(&mut self) {
        let mut changed = false;
        self.slot.state.send_if_modified(|state| {
            if state.phase() == self.expected
                && state.phase().apply(ConnectionEvent::Closed)
                    == Some(ConnectionPhase::Disconnected)
            {
                *state = SlotState::Disconnected;
                changed = true;
                true
            } else {
                false
            }
        });
        if changed {
            self.phases.notify();
        }
    }
}

impl Slot {
    fn phase(&self) -> ConnectionPhase {
        self.state.borrow().phase()
    }

    fn live(&self) -> Option<NativeSession> {
        self.state.borrow().live()
    }

    fn begin_event(
        self: &Arc<Self>,
        event: ConnectionEvent,
        phases: &PhaseEvents,
    ) -> Result<PhaseGuard> {
        let current = self.phase();
        let next = current
            .apply(event)
            .with_context(|| format!("invalid connection transition {current:?} + {event:?}"))?;
        let slot_state = match next {
            ConnectionPhase::Initializing => SlotState::Initializing,
            ConnectionPhase::Reconnecting => SlotState::Reconnecting,
            ConnectionPhase::Stopping => SlotState::Stopping,
            ConnectionPhase::Disconnected => SlotState::Disconnected,
            ConnectionPhase::Connected => {
                anyhow::bail!("Connected requires a NativeSession")
            }
        };
        self.state.send_replace(slot_state);
        phases.notify();
        Ok(PhaseGuard {
            slot: self.clone(),
            expected: next,
            phases: phases.clone(),
        })
    }

    fn clear(&self, session: &NativeSession, changes: &PeerChanges, phases: &PhaseEvents) {
        let mut changed = false;
        self.state.send_if_modified(|state| {
            if matches!(
                state,
                SlotState::Connected(s) if s.conn.stable_id() == session.conn.stable_id()
            ) {
                *state = SlotState::Disconnected;
                changes.record(session.conn.remote_id());
                changed = true;
                true
            } else {
                false
            }
        });
        if changed {
            phases.notify();
        }
    }

    fn retire(&self, session: &NativeSession, changes: &PeerChanges, phases: &PhaseEvents) -> bool {
        let mut retired = None;
        self.state.send_if_modified(|state| {
            let phase = state.phase();
            if let SlotState::Connected(current) = state
                && current.conn.stable_id() == session.conn.stable_id()
                && phase.apply(ConnectionEvent::BeginStop) == Some(ConnectionPhase::Stopping)
            {
                retired = Some(current.clone());
                *state = SlotState::Stopping;
                true
            } else {
                false
            }
        });
        let Some(current) = retired else {
            return false;
        };
        phases.notify();
        current.close_unresponsive();
        self.state.send_if_modified(|state| {
            if matches!(state, SlotState::Stopping)
                && state.phase().apply(ConnectionEvent::Closed)
                    == Some(ConnectionPhase::Disconnected)
            {
                *state = SlotState::Disconnected;
                true
            } else {
                false
            }
        });
        changes.record(current.conn.remote_id());
        phases.notify();
        true
    }
}

#[derive(Clone, Debug, Default)]
pub(super) struct PeerChanges(watch::Sender<BTreeMap<String, i64>>);

impl PeerChanges {
    fn record(&self, id: EndpointId) {
        self.0.send_modify(|times| {
            times.insert(id.to_string(), unix_time_ms());
        });
    }

    pub(super) fn subscribe(&self) -> watch::Receiver<BTreeMap<String, i64>> {
        self.0.subscribe()
    }
}

#[derive(Debug)]
pub(super) struct Connections {
    endpoint: Endpoint,
    slots: Mutex<BTreeMap<EndpointId, Arc<Slot>>>,
    config: watch::Receiver<Arc<Config>>,
    incoming: mpsc::Sender<IncomingStream<NativeSession>>,
    peer_addresses: mpsc::Sender<EndpointTicket>,
    pub(super) changes: PeerChanges,
    pub(super) phase_events: PhaseEvents,
    pub(super) local_info: watch::Sender<DeviceInfo>,
    pub(super) peer_info: watch::Sender<super::PeerDeviceInfo>,
    headers: Arc<Semaphore>,
}
impl Connections {
    pub(super) fn new(
        endpoint: Endpoint,
        config: watch::Receiver<Arc<Config>>,
        incoming: mpsc::Sender<IncomingStream<NativeSession>>,
        peer_addresses: mpsc::Sender<EndpointTicket>,
        local_info: DeviceInfo,
    ) -> Self {
        Self {
            endpoint,
            peer_addresses,
            slots: Default::default(),
            changes: PeerChanges::default(),
            phase_events: PhaseEvents::default(),
            local_info: watch::channel(local_info).0,
            peer_info: Default::default(),
            config,
            incoming,
            headers: Arc::new(Semaphore::new(256)),
        }
    }
    fn paired_slot(&self, id: EndpointId) -> Result<Arc<Slot>> {
        let grants = self.config.borrow();
        anyhow::ensure!(grants.is_paired(&id.to_string()), "peer not paired");
        Ok(self
            .slots
            .lock()
            .expect("peer sessions")
            .entry(id)
            .or_default()
            .clone())
    }
    pub(super) fn live(&self, id: EndpointId) -> Option<NativeSession> {
        self.slots.lock().expect("peer sessions").get(&id)?.live()
    }

    pub(super) fn phase(&self, id: EndpointId) -> ConnectionPhase {
        self.slots
            .lock()
            .expect("peer sessions")
            .get(&id)
            .map_or(ConnectionPhase::Disconnected, |slot| slot.phase())
    }

    pub(super) fn state_snapshot(&self, id: EndpointId) -> (ConnectionPhase, Option<u64>) {
        let Some(slot) = self.slots.lock().expect("peer sessions").get(&id).cloned() else {
            return (ConnectionPhase::Disconnected, None);
        };
        let state = slot.state.borrow();
        let phase = state.phase();
        let conn_id = state.live().map(|s| s.id() as u64);
        (phase, conn_id)
    }

    pub(super) fn retire(&self, id: EndpointId, session: &NativeSession) -> bool {
        let slot = self.slots.lock().expect("peer sessions").get(&id).cloned();
        slot.is_some_and(|slot| slot.retire(session, &self.changes, &self.phase_events))
    }

    pub(super) fn forget(&self, id: EndpointId) {
        let slot = self.slots.lock().expect("peer sessions").remove(&id);
        if let Some(SlotState::Connected(session)) =
            slot.map(|slot| slot.state.send_replace(SlotState::Stopping))
        {
            session.conn.close(1u32.into(), b"device unpaired");
        }
        self.peer_info.send_if_modified(|info| info.remove(&id.to_string()).is_some());
        self.changes.0.send_modify(|times| {
            times.remove(&id.to_string());
        });
        self.phase_events.notify();
    }

    pub(super) fn snapshot(&self) -> BTreeMap<EndpointId, PeerConnection> {
        self.slots
            .lock()
            .expect("peer sessions")
            .iter()
            .filter_map(|(id, slot)| Some((*id, slot.live()?.status())))
            .collect()
    }
    pub(super) async fn get(
        self: &Arc<Self>,
        ep: &Endpoint,
        addr: EndpointAddr,
    ) -> Result<NativeSession> {
        let slot = self.paired_slot(addr.id)?;
        if let Some(s) = slot.live() {
            return Ok(s);
        }
        let mut selected = slot.state.subscribe();
        let ready = async {
            let current = selected.wait_for(|s| s.live().is_some()).await?;
            Ok(current.live().expect("selected session"))
        };
        let dial = async {
            let _dial = slot.dial.lock().await;
            if let Some(current) = slot.live() {
                return Ok(current);
            }
            let _phase = slot.begin_event(ConnectionEvent::BeginInitialize, &self.phase_events)?;
            let conn = super::addresses::dial(ep, addr, PROTOCOL).await.context(
                "shared peer protocol handshake failed; both gateways must support paxri/session/1"
            )?;
            let pool = self.clone();
            // A cancelled request must not cancel admission on the connection its peer may select.
            tokio::spawn(async move { pool.admit(conn).await }).await??;
            slot.live().context("peer session closed during admission")
        };
        timeout(CONNECT_TIMEOUT, async {
            tokio::select! {
                biased;
                result = ready => result,
                result = dial => result,
            }
        })
        .await
        .context("connecting to paired device timed out")?
    }

    /// Refresh local network discovery, retire any existing session for this peer, and establish
    /// a fresh authenticated connection so path selection and holepunching renegotiate cleanly.
    pub(super) async fn reconnect(
        self: &Arc<Self>,
        ep: &Endpoint,
        addr: EndpointAddr,
    ) -> Result<NativeSession> {
        let slot = self.paired_slot(addr.id)?;
        timeout(CONNECT_TIMEOUT, async {
            let _dial = slot.dial.lock().await;
            ep.network_change().await;
            if let Some(previous) = slot.live() {
                previous.conn.close(0u32.into(), b"reconnecting");
                slot.clear(&previous, &self.changes, &self.phase_events);
            }
            let _phase = slot.begin_event(ConnectionEvent::BeginReconnect, &self.phase_events)?;
            let conn = super::addresses::dial(ep, addr, PROTOCOL).await.context(
                "shared peer protocol handshake failed; both gateways must support paxri/session/1",
            )?;
            let pool = self.clone();
            // A cancelled request must not cancel admission on the connection its peer may select.
            tokio::spawn(async move { pool.admit(conn).await }).await??;
            slot.live().context("peer session closed during admission")
        })
        .await
        .context("reconnecting to paired device timed out")?
    }

    async fn admit(&self, conn: Connection) -> Result<()> {
        let remote = conn.remote_id();
        let slot = match self.paired_slot(remote) {
            Ok(slot) => slot,
            Err(error) => {
                conn.close(1u32.into(), b"peer not paired");
                return Err(error);
            }
        };
        // Exactly one end serializes admission. Taking locks at both ends could deadlock
        // when each end simultaneously dials and accepts a different candidate.
        let selection = async {
            if self.endpoint.id() < remote {
                let _guard = slot.admission.lock().await;
                let previous = slot.live();
                let mut token = [0; 16];
                getrandom::fill(&mut token)
                    .map_err(|e| anyhow::anyhow!("generate session ID: {e}"))?;
                let (mut tx, mut rx) = conn.open_bi().await?;
                tx.write_all(&[u8::from(previous.is_some())]).await?;
                if let Some(existing) = &previous {
                    tx.write_all(&existing.token).await?;
                }
                tx.write_all(&token).await?;
                match rx.read_u8().await? {
                    0 => {
                        self.install(&slot, conn.clone(), token)?;
                        self.exchange_info(&slot, &conn, tx, rx);
                    }
                    1 if previous.is_some() => conn.close(0u32.into(), b"session already selected"),
                    _ => anyhow::bail!("invalid peer session acknowledgement"),
                }
            } else {
                let (mut tx, mut rx) = conn.accept_bi().await?;
                let reuse = match rx.read_u8().await? {
                    0 => false,
                    1 => {
                        let mut old = [0; 16];
                        rx.read_exact(&mut old).await?;
                        slot.live().is_some_and(|s| s.token == old)
                    }
                    _ => anyhow::bail!("invalid peer session selection"),
                };
                let mut token = [0; 16];
                rx.read_exact(&mut token).await?;
                if !reuse {
                    // Publish before acknowledging: a simultaneous dial at the selector can
                    // only propose this token after we have made it usable in both directions.
                    self.install(&slot, conn.clone(), token)?;
                }
                tx.write_all(&[u8::from(reuse)]).await?;
                if !reuse {
                    self.exchange_info(&slot, &conn, tx, rx);
                } else {
                    tx.finish()?;
                    // Keep the candidate alive until the selector consumes the acknowledgement.
                    conn.closed().await;
                }
            }
            Ok::<_, anyhow::Error>(())
        };
        let result = timeout(CONNECT_TIMEOUT, selection)
            .await
            .context("peer session admission timed out")
            .and_then(|result| result);
        if result.is_err() {
            conn.close(2u32.into(), b"session admission failed");
        }
        result
    }

    fn exchange_info(&self, slot: &Arc<Slot>, conn: &Connection, tx: SendStream, rx: RecvStream) {
        let slot = slot.clone();
        let conn = conn.clone();
        let local = self.local_info.subscribe();
        let peers = self.peer_info.clone();
        tokio::spawn(async move {
            let receive = super::device_info::receive(rx, |info| {
                // Only the currently selected connection may update this identity.
                if slot
                    .live()
                    .is_some_and(|s| s.conn.stable_id() == conn.stable_id())
                {
                    super::device_info::record(&peers, conn.remote_id(), info);
                }
            });
            tokio::select! {
                _ = conn.closed() => {}
                _ = super::device_info::publish(tx, local) => {}
                _ = receive => {}
            }
        });
    }

    fn install(&self, slot: &Arc<Slot>, conn: Connection, token: [u8; 16]) -> Result<()> {
        // Hold the grant through publishing the session. Revocation either sees and
        // closes this connection or wins before a pending handshake can publish it.
        let grants = self.config.borrow();
        anyhow::ensure!(
            grants.is_paired(&conn.remote_id().to_string())
                && self
                    .slots
                    .lock()
                    .expect("peer sessions")
                    .get(&conn.remote_id())
                    .is_some_and(|current| Arc::ptr_eq(current, slot))
                && slot.phase().apply(ConnectionEvent::Established)
                    == Some(ConnectionPhase::Connected),
            "peer no longer paired"
        );
        let session = NativeSession {
            conn,
            token,
            connected_at_ms: unix_time_ms(),
            activity: Default::default(),
        };
        let old = slot.state.send_replace(SlotState::Connected(session.clone()));
        if old.live().is_none() {
            self.changes.record(session.conn.remote_id());
        }
        self.phase_events.notify();
        if let SlotState::Connected(old) = old {
            old.conn.close(0u32.into(), b"replaced session")
        }
        drop(grants);
        super::addresses::exchange(
            self.endpoint.clone(),
            session.conn.clone(),
            self.peer_addresses.clone(),
        );
        let incoming = self.incoming.clone();
        let headers = self.headers.clone();
        let changes = self.changes.clone();
        let phases = self.phase_events.clone();
        let slot = slot.clone();
        tokio::spawn(async move {
            let mut tasks = JoinSet::new();
            loop {
                tokio::select! {
                    _ = incoming.closed() => {
                        session.conn.close(0u32.into(), b"gateway stopped");
                        break;
                    }
                    _ = tasks.join_next(), if !tasks.is_empty() => {}
                    result = session.conn.accept_bi() => {
                        let Ok((tx, mut rx)) = result else { break };
                        let Ok(permit) = headers.clone().try_acquire_owned() else { continue };
                        let incoming = incoming.clone();
                        let session = session.clone();
                        tasks.spawn(async move {
                            let _permit = permit;
                            if let Ok(Ok(kind)) = timeout(CONNECT_TIMEOUT, StreamKind::read(&mut rx)).await {
                                let stream = session.stream(kind, rx, tx);
                                let _ = incoming.send(IncomingStream { session, kind, stream }).await;
                            }
                        });
                    }
                }
            }
            slot.clear(&session, &changes, &phases);
        });
        Ok(())
    }
}

impl ProtocolHandler for Connections {
    async fn accept(&self, conn: Connection) -> Result<(), AcceptError> {
        self.admit(conn)
            .await
            .map_err(|e| AcceptError::from_boxed(e.into()))
    }
    async fn shutdown(&self) {
        for slot in self.slots.lock().expect("peer sessions").values() {
            let old = slot.state.send_replace(SlotState::Stopping);
            if let SlotState::Connected(session) = old {
                session.conn.close(0u32.into(), b"gateway stopped");
                self.changes.record(session.conn.remote_id());
            }
        }
        self.phase_events.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iroh::{
        RelayMode,
        endpoint::{PortmapperConfig, presets},
        protocol::Router,
    };
    use std::{net::Ipv4Addr, time::Duration};
    use tokio::io::AsyncWriteExt;

    async fn endpoint() -> Result<Endpoint> {
        Ok(Endpoint::builder(presets::Minimal)
            .bind_addr((Ipv4Addr::LOCALHOST, 0))?
            .relay_mode(RelayMode::Disabled)
            .portmapper_config(PortmapperConfig::Disabled)
            .bind()
            .await?)
    }

    fn pool(
        local: &Endpoint,
        remote: &Endpoint,
    ) -> (
        Arc<Connections>,
        mpsc::Receiver<IncomingStream<NativeSession>>,
        watch::Sender<Arc<Config>>,
    ) {
        let config = paxri_config::parse(&format!(
            "version=1\n[peers.peer]\nendpoint_id=\"{}\"",
            remote.id()
        ))
        .unwrap();
        let (grants, config) = watch::channel(Arc::new(config));
        let (tx, rx) = mpsc::channel(4);
        (
            Arc::new(Connections::new(
                local.clone(),
                config,
                tx,
                mpsc::channel(8).0,
                DeviceInfo {
                    platform: paxri_config::DevicePlatform::Linux,
                    capabilities: Default::default(),
                    device_name: None,
                },
            )),
            rx,
            grants,
        )
    }

    #[tokio::test]
    async fn device_recovers_after_failed_dials_without_reopening_terminal_streams() -> Result<()> {
        use super::super::NativeTransport;
        let a = endpoint().await?;
        let b = endpoint().await?;
        let (pa, _streams_a, grants) = pool(&a, &b);
        let (pb, mut streams_b, _grants_b) = pool(&b, &a);
        b.set_alpns(vec![PROTOCOL.to_vec()]);
        let client = Arc::new(NativeTransport {
            endpoint: a.clone(),
            connections: pa.clone(),
            config: grants,
            peers: std::sync::RwLock::new(BTreeMap::from([("peer".into(), b.addr())])),
            presence_refresh: Default::default(),
        });
        let announce = tokio::spawn(client.clone().announce_presence());
        let next = || async {
            b.accept()
                .await
                .context("endpoint closed")?
                .await
                .map_err(anyhow::Error::from)
        };
        let mut selected = pa.paired_slot(b.id())?.state.subscribe();
        timeout(Duration::from_secs(25), async {
            pb.admit(next().await?).await?;
            let first = selected
                .wait_for(|s| s.live().is_some())
                .await?
                .live()
                .unwrap();
            let mut terminal = first.open(StreamKind::Terminal).await?;
            let remote_terminal = streams_b.recv().await.unwrap();
            assert_eq!(remote_terminal.kind, StreamKind::Terminal);
            first.conn.close(0u32.into(), b"link lost");
            selected.wait_for(|s| s.live().is_none()).await?;
            // Only A initiates connections. B stays at the same address, refuses two
            // attempts, then accepts the next retry without notifying A.
            for _ in 0..2 {
                next().await?.close(1u32.into(), b"unavailable");
            }
            pb.admit(next().await?).await?;
            let restored = selected
                .wait_for(|s| s.live().is_some())
                .await?
                .live()
                .unwrap();
            assert_ne!(restored.token, first.token);
            assert!(terminal.read_u8().await.is_err());
            assert!(restored.status().streams.is_empty());
            assert!(streams_b.try_recv().is_err());
            Ok::<_, anyhow::Error>(())
        })
        .await??;
        a.close().await;
        b.close().await;
        announce.await?;
        Ok(())
    }

    #[tokio::test]
    async fn simultaneous_dials_select_one_connection_for_both_directions() -> Result<()> {
        let a = endpoint().await?;
        let b = endpoint().await?;
        let (pa, mut at_a, _grants_a) = pool(&a, &b);
        let (pb, mut at_b, _grants_b) = pool(&b, &a);
        let ra = Router::builder(a.clone())
            .accept(PROTOCOL, pa.clone())
            .spawn();
        let rb = Router::builder(b.clone())
            .accept(PROTOCOL, pb.clone())
            .spawn();
        timeout(Duration::from_secs(8), async {
            // Force two actual QUIC handshakes before either outgoing candidate is admitted.
            let (out_a, out_b) =
                tokio::try_join!(a.connect(b.addr(), PROTOCOL), b.connect(a.addr(), PROTOCOL))?;
            tokio::try_join!(pa.admit(out_a.clone()), pb.admit(out_b.clone()))?;
            assert_ne!(
                out_a.close_reason().is_some(),
                out_b.close_reason().is_some()
            );
            let sa = pa.get(&a, b.addr()).await?;
            let sb = pb.get(&b, a.addr()).await?;
            assert_eq!(sa.token, sb.token);
            let mut terminal = sa.open(StreamKind::Terminal).await?;
            let mut host = at_b.recv().await.unwrap();
            let mut command = sb.open(StreamKind::Command).await?;
            let mut client = at_a.recv().await.unwrap();
            assert_eq!(host.kind, StreamKind::Terminal);
            assert_eq!(client.kind, StreamKind::Command);
            let mut remote_info = pa.peer_info.subscribe();
            let id = b.id().to_string();
            remote_info.wait_for(|all| all.contains_key(&id)).await?;
            let android = DeviceInfo {
                device_name: Some("Z Flip3".to_owned()),
                platform: paxri_config::DevicePlatform::Android,
                capabilities: Default::default(),
            };
            pb.local_info.send_replace(android.clone());
            remote_info.wait_for(|all| all.get(&id) == Some(&android)).await?;
            let upgraded = DeviceInfo {
                capabilities: paxri_config::DeviceCapabilities { shell: true, exec: true, managed_terminals: true, terminal_heartbeat: true, forward_rules: Some(true), create_forward_rules: true },
                ..android
            };
            pb.local_info.send_replace(upgraded.clone());
            remote_info.wait_for(|all| all.get(&id) == Some(&upgraded)).await?;
            assert_eq!(pa.live(b.id()).unwrap().token, sa.token);
            host.stream.write_u8(37).await?;
            assert_eq!(terminal.read_u8().await?, 37);
            drop((terminal, host));
            client.stream.write_u8(23).await?;
            assert_eq!(command.read_u8().await?, 23);
            assert_eq!(
                sa.status().streams,
                BTreeMap::from([(StreamKind::Command, 1)])
            );
            Ok::<_, anyhow::Error>(())
        })
        .await??;
        ra.shutdown().await?;
        rb.shutdown().await?;
        Ok(())
    }

    #[tokio::test]
    async fn reconnect_replaces_existing_session_from_either_side() -> Result<()> {
        let a = endpoint().await?;
        let b = endpoint().await?;
        let (pa, mut at_a, _grants_a) = pool(&a, &b);
        let (pb, mut at_b, _grants_b) = pool(&b, &a);
        let ra = Router::builder(a.clone())
            .accept(PROTOCOL, pa.clone())
            .spawn();
        let rb = Router::builder(b.clone())
            .accept(PROTOCOL, pb.clone())
            .spawn();
        timeout(Duration::from_secs(12), async {
            let initial = pa.get(&a, b.addr()).await?;
            assert_eq!(pb.get(&b, a.addr()).await?.token, initial.token);

            // Reconnect initiated by endpoint A replaces the live session on both sides.
            let second = pa.reconnect(&a, b.addr()).await?;
            assert!(!second.same_connection(&initial));
            assert_eq!(pb.get(&b, a.addr()).await?.token, second.token);
            let mut tx = second.open(StreamKind::Command).await?;
            let mut rx = at_b.recv().await.unwrap();
            tx.write_u8(41).await?;
            assert_eq!(rx.stream.read_u8().await?, 41);
            drop((tx, rx));

            // Reconnect initiated by endpoint B (the opposite ID ordering) also replaces cleanly.
            let third = pb.reconnect(&b, a.addr()).await?;
            assert!(!third.same_connection(&second));
            assert_eq!(pa.get(&a, b.addr()).await?.token, third.token);
            let mut tx = third.open(StreamKind::Command).await?;
            let mut rx = at_a.recv().await.unwrap();
            tx.write_u8(53).await?;
            assert_eq!(rx.stream.read_u8().await?, 53);
            Ok::<_, anyhow::Error>(())
        })
        .await??;
        ra.shutdown().await?;
        rb.shutdown().await?;
        Ok(())
    }

    #[tokio::test]
    async fn retire_and_shutdown_enforce_stopping_and_disconnected_phases() -> Result<()> {
        let a = endpoint().await?;
        let b = endpoint().await?;
        let (pa, _at_a, _grants_a) = pool(&a, &b);
        let (pb, _at_b, _grants_b) = pool(&b, &a);
        let ra = Router::builder(a.clone())
            .accept(PROTOCOL, pa.clone())
            .spawn();
        let rb = Router::builder(b.clone())
            .accept(PROTOCOL, pb.clone())
            .spawn();
        timeout(Duration::from_secs(10), async {
            assert_eq!(pa.phase(b.id()), ConnectionPhase::Disconnected);
            let session = pa.get(&a, b.addr()).await?;
            assert_eq!(pa.phase(b.id()), ConnectionPhase::Connected);
            assert!(pa.retire(b.id(), &session));
            assert_eq!(pa.phase(b.id()), ConnectionPhase::Disconnected);
            assert!(pa.live(b.id()).is_none());
            // Retiring the already-retired session is a no-op.
            assert!(!pa.retire(b.id(), &session));
            // Re-establishing after retire succeeds cleanly.
            let fresh = pa.get(&a, b.addr()).await?;
            assert_eq!(pa.phase(b.id()), ConnectionPhase::Connected);
            assert!(!fresh.same_connection(&session));
            // Shutdown moves the slot into Stopping and prevents late installs.
            pa.shutdown().await;
            assert_eq!(pa.phase(b.id()), ConnectionPhase::Stopping);
            assert!(pa.live(b.id()).is_none());
            Ok::<_, anyhow::Error>(())
        })
        .await??;
        ra.shutdown().await?;
        rb.shutdown().await?;
        Ok(())
    }
}
