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
use peerhatch_config::{Config, DeviceInfo};
use peerhatch_core::{
    PeerSession, StreamKind,
    connection::{ConnectionTraffic, PathKind, PeerConnection, unix_time_ms},
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

#[derive(Debug, Default)]
struct Slot {
    selected: watch::Sender<Option<NativeSession>>,
    dial: AsyncMutex<()>,
    admission: AsyncMutex<()>,
}
impl Slot {
    fn live(&self) -> Option<NativeSession> {
        self.selected
            .borrow()
            .as_ref()
            .filter(|s| s.conn.close_reason().is_none())
            .cloned()
    }

    fn clear(&self, session: &NativeSession, changes: &PeerChanges) {
        self.selected.send_if_modified(|selected| {
            if selected
                .as_ref()
                .is_some_and(|s| s.conn.stable_id() == session.conn.stable_id())
            {
                *selected = None;
                changes.record(session.conn.remote_id());
                true
            } else {
                false
            }
        });
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

    pub(super) fn forget(&self, id: EndpointId) {
        let slot = self.slots.lock().expect("peer sessions").remove(&id);
        if let Some(session) = slot.and_then(|slot| slot.selected.send_replace(None)) {
            session.conn.close(1u32.into(), b"device unpaired");
        }
        self.peer_info.send_if_modified(|info| info.remove(&id.to_string()).is_some());
        self.changes.0.send_modify(|times| {
            times.remove(&id.to_string());
        });
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
        let mut selected = slot.selected.subscribe();
        let ready = async {
            let current = selected
                .wait_for(|s| s.as_ref().is_some_and(|s| s.conn.close_reason().is_none()))
                .await?;
            Ok(current.as_ref().expect("selected session").clone())
        };
        let dial = async {
            let _dial = slot.dial.lock().await;
            if let Some(current) = slot.live() {
                return Ok(current);
            }
            let conn = super::addresses::dial(ep, addr, PROTOCOL).await.context(
                "shared peer protocol handshake failed; both gateways must support p2p-tunnel/session/1"
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
                let selected = slot.selected.borrow();
                if selected.as_ref().is_some_and(|s| s.conn.stable_id() == conn.stable_id()) {
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
                    .is_some_and(|current| Arc::ptr_eq(current, slot)),
            "peer no longer paired"
        );
        let session = NativeSession {
            conn,
            token,
            connected_at_ms: unix_time_ms(),
            activity: Default::default(),
        };
        let old = slot.selected.send_replace(Some(session.clone()));
        if old.as_ref().is_none_or(|s| s.conn.close_reason().is_some()) {
            self.changes.record(session.conn.remote_id());
        }
        if let Some(old) = old {
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
            slot.clear(&session, &changes);
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
            let selected = slot.selected.borrow().clone();
            if let Some(session) = selected {
                session.conn.close(0u32.into(), b"gateway stopped");
                slot.clear(&session, &self.changes);
            }
        }
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

    #[tokio::test]
    async fn simultaneous_dials_select_one_connection_for_both_directions() -> Result<()> {
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
        ) {
            let config = peerhatch_config::parse(&format!(
                "version=1\n[peers.peer]\nendpoint_id=\"{}\"",
                remote.id()
            ))
            .unwrap();
            let (_, config) = watch::channel(Arc::new(config));
            let (tx, rx) = mpsc::channel(4);
            (
                Arc::new(Connections::new(
                    local.clone(),
                    config,
                    tx,
                    mpsc::channel(8).0,
                    DeviceInfo { platform: peerhatch_config::DevicePlatform::Linux, capabilities: Default::default(), device_name: None },
                )),
                rx,
            )
        }
        let a = endpoint().await?;
        let b = endpoint().await?;
        let (pa, mut at_a) = pool(&a, &b);
        let (pb, mut at_b) = pool(&b, &a);
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
                platform: peerhatch_config::DevicePlatform::Android,
                capabilities: Default::default(),
            };
            pb.local_info.send_replace(android.clone());
            remote_info.wait_for(|all| all.get(&id) == Some(&android)).await?;
            let upgraded = DeviceInfo {
                capabilities: peerhatch_config::DeviceCapabilities { shell: true, exec: true, forward_rules: Some(true), create_forward_rules: true },
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
}
