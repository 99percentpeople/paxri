//! A window owns an attachment. The gateway owns the PTY and the pairing lease.
mod replay;

use super::*;
use paxri_terminal::managed::{self as protocol, Key, Request};
use std::{
    collections::BTreeMap,
    sync::{Mutex, Weak},
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncWrite},
    sync::mpsc,
};

const OUTPUT_QUEUE_SIZE: usize = 128;
type OwnerKey = (String, Key);

#[derive(Default)]
struct History {
    bytes: replay::Replay,
    exit: Option<u32>,
    viewer: Option<mpsc::Sender<Vec<u8>>>,
}

struct Entry {
    history: Mutex<History>,
    pty: Weak<Session>,
    attachment: watch::Sender<()>,
    stop: watch::Sender<bool>,
    finished: watch::Receiver<bool>,
}

async fn disconnected_for<T>(
    mut changes: watch::Receiver<BTreeMap<String, i64>>,
    mut policy: watch::Receiver<T>,
    online: impl Fn() -> bool,
    grace: impl Fn(&T) -> Option<Duration>,
) {
    let mut offline_since = None;
    loop {
        changes.borrow_and_update();
        let timeout = grace(&policy.borrow_and_update());
        if online() {
            offline_since = None;
        } else {
            offline_since.get_or_insert_with(tokio::time::Instant::now);
        }
        let deadline = offline_since.and_then(|since| timeout.map(|limit| since + limit));
        let expired = async {
            match deadline {
                Some(until) => tokio::time::sleep_until(until).await,
                None => std::future::pending().await,
            }
        };
        tokio::select! {
            result = changes.changed() => if result.is_err() { return; },
            result = policy.changed() => if result.is_err() { return; },
            _ = expired => if !online() { return; },
        }
    }
}

pub(crate) struct Registry {
    entries: Mutex<BTreeMap<OwnerKey, Arc<Entry>>>,
    creation: tokio::sync::Mutex<()>,
    transport: Arc<paxri_native::gateway::NativeTransport>,
    session_keep_alive: watch::Receiver<paxri_config::startup::SessionKeepAlive>,
    shutdown: watch::Sender<()>,
}

impl Registry {
    pub(crate) fn new(
        transport: Arc<paxri_native::gateway::NativeTransport>,
        session_keep_alive: watch::Receiver<paxri_config::startup::SessionKeepAlive>,
    ) -> Self {
        Self {
            entries: Default::default(),
            creation: Default::default(),
            transport,
            session_keep_alive,
            shutdown: watch::channel(()).0,
        }
    }

    async fn create(
        self: &Arc<Self>,
        host: &Terminals,
        owner: OwnerKey,
        size: Size,
    ) -> Result<Arc<Entry>> {
        size.validate()?;
        anyhow::ensure!(
            crate::account::is_standard_user()?,
            "Hosting a terminal requires a non-elevated gateway"
        );
        let slot = host
            .slots
            .clone()
            .try_acquire_owned()
            .map_err(|_| anyhow::anyhow!("All terminal sessions are in use"))?;
        // Serialize create admission so simultaneous retries cannot execute shell startup twice.
        let _creating = self.creation.lock().await;
        anyhow::ensure!(
            !self.entries.lock().unwrap().contains_key(&owner),
            "Session already exists"
        );
        let mut config = host.config.clone();
        let keep_alive = self.session_keep_alive.clone();
        let cmd = command_for(None, &host.tools, &owner.0).await?;
        let cfg = config.clone();
        let peer = owner.0.clone();
        let (pty, output) = tokio::task::spawn_blocking(move || {
            let cfg = cfg.borrow();
            anyhow::ensure!(is_paired(&cfg, &peer), "Device is no longer paired");
            Session::spawn_managed(cmd, size.into())
        })
        .await??;
        let pty = Arc::new(pty);
        let (stop, mut stopping) = watch::channel(false);
        let (finished, done) = watch::channel(false);
        let entry = Arc::new(Entry {
            history: Mutex::default(),
            pty: Arc::downgrade(&pty),
            attachment: watch::channel(()).0,
            stop,
            finished: done,
        });
        self.entries
            .lock()
            .unwrap()
            .insert(owner.clone(), entry.clone());
        let registry = Arc::downgrade(self);
        let transport = self.transport.clone();
        let mut shutdown = self.shutdown.subscribe();
        let active = entry.clone();
        tokio::spawn(async move {
            let collect = async {
                while let Ok(event) = output.recv().await {
                    tokio::task::consume_budget().await;
                    match event {
                        Output::Bytes(bytes) | Output::Replay(bytes) => active.publish(bytes).await,
                        Output::Exited(status) => {
                            return status.map(|s| s.exit_code()).unwrap_or(1);
                        }
                        Output::Error(error) => eprintln!("terminal I/O: {error}"),
                    }
                }
                1
            };
            let code = tokio::select! {
                biased;
                _ = shutdown.changed() => 1,
                _ = stopping.changed() => 0,
                _ = config.wait_for(|cfg| !is_paired(cfg, &owner.0)) => 1,
                _ = disconnected_for(
                    transport.subscribe_peer_changes(),
                    keep_alive,
                    || transport.endpoint_online(&owner.0),
                    |policy| policy.duration(),
                ) => 1,
                code = collect => code,
            };
            drop(pty);
            if let Some(registry) = registry.upgrade() {
                registry.entries.lock().unwrap().remove(&owner);
            }
            active.finish(code);
            drop(slot);
            finished.send_replace(true);
        });
        Ok(entry)
    }

    pub(crate) async fn serve(
        self: &Arc<Self>,
        host: &Terminals,
        peer: String,
        mut stream: impl AsyncRead + AsyncWrite + Unpin,
    ) -> Result<()> {
        let request: Request = timeout(STREAM_TIMEOUT, protocol::read(&mut stream)).await??;
        let find = |key| {
            self.entries
                .lock()
                .unwrap()
                .get(&(peer.clone(), key))
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("Session has ended"))
        };
        let mut attachment = None;
        let result: Result<Vec<Key>> = async {
            anyhow::ensure!(
                is_paired(&host.config.borrow(), &peer),
                "Device is no longer paired"
            );
            match request {
                Request::List => {
                    return Ok(self
                        .entries
                        .lock()
                        .unwrap()
                        .keys()
                        .filter(|(owner, _)| owner == &peer)
                        .map(|(_, key)| *key)
                        .collect());
                }
                Request::End(key) => {
                    let entry = find(key)?;
                    entry.stop.send_replace(true);
                    let mut done = entry.finished.clone();
                    done.wait_for(|&finished| finished).await?;
                }
                Request::Create(key, size) => {
                    attachment = Some(self.create(host, (peer.clone(), key), size).await?)
                }
                Request::Attach(key, size) => {
                    let entry = find(key)?;
                    if let Some(pty) = entry.pty.upgrade() {
                        pty.resize(size.validate()?.into());
                    }
                    attachment = Some(entry);
                }
            }
            Ok(Vec::new())
        }
        .await;
        protocol::write(&mut stream, &result.map_err(|e| e.to_string())).await?;
        if let Some(entry) = attachment {
            entry
                .attach(stream, matches!(request, Request::Attach(..)))
                .await?;
        }
        Ok(())
    }
}

impl Entry {
    async fn publish(&self, bytes: Vec<u8>) {
        let viewer = {
            let mut h = self.history.lock().unwrap();
            // A new attachment must see each chunk either in its snapshot or in its live queue.
            h.bytes.append(&bytes);
            h.viewer.take_if(|queue| queue.is_closed());
            h.viewer.clone()
        };
        if let Some(viewer) = viewer {
            // Only an attached viewer applies backpressure. Detach or replacement drops its receiver.
            let _ = viewer.send(bytes).await;
        }
    }

    fn finish(&self, code: u32) {
        let mut h = self.history.lock().unwrap();
        h.exit = Some(code);
        h.viewer = None;
    }

    async fn attach(
        &self,
        stream: impl AsyncRead + AsyncWrite + Unpin,
        replay: bool,
    ) -> Result<()> {
        let (mut reader, mut writer) = tokio::io::split(stream);
        let (acknowledge, mut probes) = mpsc::channel(1);
        let (history, mut output, mut replaced) = {
            let mut h = self.history.lock().unwrap();
            self.attachment.send_replace(());
            let (viewer, output) = mpsc::channel(OUTPUT_QUEUE_SIZE);
            if h.exit.is_none() {
                h.viewer = Some(viewer);
            }
            (h.bytes.snapshot(), output, self.attachment.subscribe())
        };
        let send = async {
            if replay {
                writer.write_u32(history.len() as u32).await?;
                writer.write_all(&history).await?;
            } else {
                writer.write_u32(0).await?;
                Frame::Data(history).write(&mut writer).await?;
            }
            loop {
                tokio::select! {
                    Some(()) = probes.recv() => protocol::write_heartbeat(&mut writer).await?,
                    bytes = output.recv() => match bytes {
                        Some(bytes) => Frame::Data(bytes).write(&mut writer).await?,
                        None => {
                            let exit = self.history.lock().unwrap().exit;
                            if let Some(code) = exit {
                                Frame::Exit(code).write(&mut writer).await?;
                            }
                            return Ok(());
                        }
                    },
                }
            }
        };
        let input = async {
            loop {
                let Some(frame) = protocol::read_frame(&mut reader).await? else {
                    if self.pty.upgrade().is_some_and(|s| !s.input.is_closed()) {
                        acknowledge.send(()).await?;
                    }
                    continue;
                };
                match frame {
                    Frame::Data(bytes) => {
                        if let Some(sender) = self.pty.upgrade().map(|s| s.input.clone()) {
                            // The PTY can close input while its final output is still draining.
                            let _ = sender.send(bytes).await;
                        }
                    }
                    Frame::Resize(size) => {
                        if let Some(pty) = self.pty.upgrade() {
                            pty.resize(size.into());
                        }
                    }
                    Frame::Exit(0) => {
                        self.stop.send_replace(true);
                        return std::future::pending().await;
                    }
                    Frame::Exit(_) => bail!("Invalid termination request"),
                }
            }
        };
        tokio::select! { biased; _ = replaced.changed() => Ok(()), res = send => res, res = input => res }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    use tokio::io::{AsyncReadExt, DuplexStream, duplex};

    fn entry() -> Arc<Entry> {
        Arc::new(Entry {
            history: Mutex::default(),
            pty: Weak::new(),
            attachment: watch::channel(()).0,
            stop: watch::channel(false).0,
            finished: watch::channel(false).1,
        })
    }

    fn viewer(entry: &Arc<Entry>) -> DuplexStream {
        let (stream, host) = duplex(16);
        let entry = entry.clone();
        tokio::spawn(async move { entry.attach(host, true).await.unwrap() });
        stream
    }

    #[tokio::test(start_paused = true)]
    async fn full_output_queue_preserves_bytes_and_can_hand_over_to_a_new_viewer() {
        for replace in [false, true] {
            timeout(Duration::from_secs(5), exercise_full_queue(replace))
                .await
                .expect("output stalled");
        }
    }

    async fn exercise_full_queue(replace: bool) {
        let entry = entry();
        let mut stream = viewer(&entry);
        assert!(
            protocol::read_history(&mut stream)
                .await
                .unwrap()
                .is_empty()
        );
        let first = vec![b'x'; 64];
        entry.publish(first.clone()).await;
        assert_eq!(stream.read_u8().await.unwrap(), 1);
        // The first data frame cannot fit in the stream, so every following chunk queues up.
        let mut expected = first.clone();
        for i in 0..OUTPUT_QUEUE_SIZE {
            entry.publish(vec![i as u8]).await;
            expected.push(i as u8);
        }
        let blocked = entry.publish(b"pending".to_vec());
        tokio::pin!(blocked);
        tokio::select! {
            biased;
            _ = &mut blocked => panic!("a full output queue must wait for its viewer"),
            _ = std::future::ready(()) => {}
        }
        expected.extend_from_slice(b"pending");
        let mut next;
        let (reader, mut received) = if replace {
            // Keep the old stream open and stalled throughout the handover.
            next = viewer(&entry);
            let replay = protocol::read_history(&mut next).await.unwrap();
            (&mut next, replay)
        } else {
            assert_eq!(usize::from(stream.read_u16().await.unwrap()), first.len());
            let mut received = vec![0; first.len()];
            stream.read_exact(&mut received).await.unwrap();
            (&mut stream, received)
        };
        let ((), ()) = tokio::join!(
            async {
                blocked.await;
                entry.publish(b"tail".to_vec()).await;
                entry.finish(23);
            },
            async {
                loop {
                    match Frame::read(reader).await.unwrap() {
                        Frame::Data(bytes) => received.extend(bytes),
                        Frame::Exit(23) => break,
                        _ => panic!("unexpected output frame"),
                    }
                }
            },
        );
        expected.extend_from_slice(b"tail");
        assert_eq!(received, expected);
    }

    #[tokio::test(start_paused = true)]
    async fn expiry_requires_five_continuous_minutes_offline() {
        let online = Arc::new(AtomicBool::new(true));
        let observed = online.clone();
        let (changes, receiver) = watch::channel(BTreeMap::new());
        let (_policy_tx, policy_rx) = watch::channel(Some(Duration::from_secs(300)));
        let lease = tokio::spawn(async move {
            disconnected_for(
                receiver,
                policy_rx,
                || observed.load(Ordering::SeqCst),
                |grace| *grace,
            )
            .await;
        });
        let set_online = |value| {
            online.store(value, Ordering::SeqCst);
            changes.send_modify(|_| {});
        };
        let advance = |seconds| async move {
            tokio::task::yield_now().await;
            tokio::time::advance(Duration::from_secs(seconds)).await;
            tokio::task::yield_now().await;
        };
        advance(600).await;
        assert!(!lease.is_finished(), "detached while still online");
        set_online(false);
        advance(299).await;
        assert!(!lease.is_finished());
        set_online(true);
        advance(600).await;
        assert!(
            !lease.is_finished(),
            "reconnect cancels the previous deadline"
        );
        set_online(false);
        advance(200).await;
        changes.send_modify(|_| {}); // Another peer changed state.
        advance(99).await;
        assert!(!lease.is_finished());
        advance(1).await;
        assert!(lease.is_finished());
        lease.await.unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn forever_keep_alive_never_expires_offline_and_live_updates_apply() {
        let online = Arc::new(AtomicBool::new(false));
        let observed = online.clone();
        let (_changes, receiver) = watch::channel(BTreeMap::new());
        let (policy_tx, policy_rx) = watch::channel(Some(Duration::from_secs(300)));
        let lease = tokio::spawn(async move {
            disconnected_for(
                receiver,
                policy_rx,
                || observed.load(Ordering::SeqCst),
                |grace| *grace,
            )
            .await;
        });
        let advance = |seconds| async move {
            tokio::task::yield_now().await;
            tokio::time::advance(Duration::from_secs(seconds)).await;
            tokio::task::yield_now().await;
        };
        advance(299).await;
        assert!(!lease.is_finished());
        // Switching to Forever while already offline cancels expiry.
        policy_tx.send_replace(None);
        advance(48 * 3600).await;
        assert!(!lease.is_finished(), "forever policy must never expire");
        // Lowering the policy back while still offline expires promptly.
        policy_tx.send_replace(Some(Duration::from_secs(300)));
        advance(1).await;
        assert!(lease.is_finished());
        lease.await.unwrap();
    }

    #[tokio::test]
    async fn history_keeps_only_the_most_recent_bytes() {
        let entry = entry();
        entry.publish(vec![1; protocol::HISTORY_LIMIT + 10]).await;
        entry.publish(vec![2; 3]).await;
        let bytes = entry.history.lock().unwrap().bytes.snapshot();
        assert!(bytes.len() <= protocol::HISTORY_LIMIT);
        assert_eq!(bytes.first(), Some(&1));
        assert_eq!(
            bytes.iter().rev().take(4).copied().collect::<Vec<_>>(),
            [2, 2, 2, 1]
        );
    }
}
