//! A window owns an attachment. The gateway owns the PTY and the pairing lease.
mod replay;

use super::*;
use anyhow::Context;
use paxri_terminal::managed::{self as protocol, Key, Request};
use std::{
    collections::BTreeMap,
    sync::{Mutex, Weak},
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncWrite},
    sync::broadcast,
};

const DISCONNECT_GRACE: Duration = Duration::from_secs(300);
type OwnerKey = (String, Key);

#[derive(Clone)]
enum Event {
    Data(Arc<Vec<u8>>),
    Exit(u32),
}

struct History {
    bytes: replay::Replay,
    exit: Option<u32>,
    events: broadcast::Sender<Event>,
}

impl History {
    fn publish(&mut self, event: Event) {
        match &event {
            Event::Data(bytes) => {
                self.bytes.append(bytes);
            }
            Event::Exit(code) => self.exit = Some(*code),
        }
        let _ = self.events.send(event);
    }
}

struct Entry {
    history: Mutex<History>,
    pty: Weak<Session>,
    attachment: watch::Sender<()>,
    stop: watch::Sender<bool>,
    finished: watch::Receiver<bool>,
}

async fn disconnected_for(
    mut changes: watch::Receiver<BTreeMap<String, i64>>,
    online: impl Fn() -> bool,
    grace: Duration,
) {
    let mut deadline = None;
    loop {
        changes.borrow_and_update();
        if online() {
            deadline = None;
        } else {
            deadline.get_or_insert_with(|| tokio::time::Instant::now() + grace);
        }
        let expired = async {
            match deadline {
                Some(until) => tokio::time::sleep_until(until).await,
                None => std::future::pending().await,
            }
        };
        tokio::select! {
            result = changes.changed() => if result.is_err() { return; },
            _ = expired => if !online() { return; },
        }
    }
}

pub(crate) struct Registry {
    entries: Mutex<BTreeMap<OwnerKey, Arc<Entry>>>,
    creation: tokio::sync::Mutex<()>,
    transport: Arc<paxri_native::gateway::NativeTransport>,
    shutdown: watch::Sender<()>,
}

impl Registry {
    pub(crate) fn new(transport: Arc<paxri_native::gateway::NativeTransport>) -> Self {
        Self {
            entries: Default::default(),
            creation: Default::default(),
            transport,
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
            history: Mutex::new(History {
                bytes: Default::default(),
                exit: None,
                events: broadcast::channel(128).0,
            }),
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
                        Output::Bytes(bytes) | Output::Replay(bytes) => active
                            .history
                            .lock()
                            .unwrap()
                            .publish(Event::Data(Arc::new(bytes))),
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
                _ = disconnected_for(transport.subscribe_peer_changes(), || transport.endpoint_online(&owner.0), DISCONNECT_GRACE) => 1,
                code = collect => code,
            };
            drop(pty);
            if let Some(registry) = registry.upgrade() {
                registry.entries.lock().unwrap().remove(&owner);
            }
            active.history.lock().unwrap().publish(Event::Exit(code));
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
    async fn attach(
        &self,
        stream: impl AsyncRead + AsyncWrite + Unpin,
        replay: bool,
    ) -> Result<()> {
        let (mut reader, mut writer) = tokio::io::split(stream);
        let (acknowledge, mut probes) = tokio::sync::mpsc::channel(1);
        let (history, exit, mut events, mut replaced) = {
            let h = self.history.lock().unwrap();
            self.attachment.send_replace(());
            (
                h.bytes.snapshot(),
                h.exit,
                h.events.subscribe(),
                self.attachment.subscribe(),
            )
        };
        let send = async {
            if replay {
                writer.write_u32(history.len() as u32).await?;
                writer.write_all(&history).await?;
            } else {
                writer.write_u32(0).await?;
                Frame::Data(history).write(&mut writer).await?;
            }
            if let Some(code) = exit {
                return Frame::Exit(code).write(&mut writer).await;
            }
            loop {
                tokio::select! {
                    Some(()) = probes.recv() => protocol::write_heartbeat(&mut writer).await?,
                    event = events.recv() => match event? {
                        Event::Data(bytes) => Frame::Data((*bytes).clone()).write(&mut writer).await?,
                        Event::Exit(code) => return Frame::Exit(code).write(&mut writer).await,
                    },
                }
            }
        };
        let input = async {
            loop {
                let Some(frame) = protocol::read_frame(&mut reader).await? else {
                    anyhow::ensure!(
                        self.pty.upgrade().is_some_and(|s| !s.input.is_closed()),
                        "Terminal input closed"
                    );
                    acknowledge.send(()).await?;
                    continue;
                };
                match frame {
                    Frame::Data(bytes) => {
                        let sender = self
                            .pty
                            .upgrade()
                            .context("Terminal input closed")?
                            .input
                            .clone();
                        sender.send(bytes).await.context("Terminal input closed")?;
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

    #[tokio::test(start_paused = true)]
    async fn expiry_requires_five_continuous_minutes_offline() {
        let online = Arc::new(AtomicBool::new(true));
        let observed = online.clone();
        let (changes, receiver) = watch::channel(BTreeMap::new());
        let lease = tokio::spawn(async move {
            disconnected_for(
                receiver,
                || observed.load(Ordering::SeqCst),
                DISCONNECT_GRACE,
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

    #[test]
    fn history_keeps_only_the_most_recent_bytes() {
        let mut history = History {
            bytes: Default::default(),
            exit: None,
            events: broadcast::channel(1).0,
        };
        history.publish(Event::Data(Arc::new(vec![1; protocol::HISTORY_LIMIT + 10])));
        history.publish(Event::Data(Arc::new(vec![2; 3])));
        let bytes = history.bytes.snapshot();
        assert!(bytes.len() <= protocol::HISTORY_LIMIT);
        assert_eq!(bytes.first(), Some(&1));
        assert_eq!(
            bytes.iter().rev().take(4).copied().collect::<Vec<_>>(),
            [2, 2, 2, 1]
        );
    }
}
