use async_channel::{Receiver, Sender, TrySendError};
use paxri_control::terminal::{
    self as wire,
    pty::{self, ExitStatus, PtySize},
};
use std::path::Path;
use tokio::{runtime::Handle, task::JoinHandle};

pub use pty::Output;
use wire::managed::{Key, Request};

pub struct Session {
    io: pty::Session,
    size: Option<PtySize>,
    pub input: Input,
    task: JoinHandle<()>,
    end: Option<tokio::sync::oneshot::Sender<()>>,
}

// The UI cannot block on a full channel. A queued write waits for its predecessor
// so a mouse release, key or terminal reply cannot overtake an earlier write.
pub struct Input {
    sender: Sender<Vec<u8>>,
    runtime: Handle,
    pending: Option<Receiver<()>>,
}

impl Input {
    pub fn send(&mut self, bytes: Vec<u8>) {
        if bytes.is_empty() {
            return;
        }
        let previous = self.pending.take().filter(|done| !done.is_closed());
        let bytes = if previous.is_none() {
            match self.sender.try_send(bytes) {
                Ok(()) | Err(TrySendError::Closed(_)) => return,
                Err(TrySendError::Full(bytes)) => bytes,
            }
        } else {
            bytes
        };
        let sender = self.sender.clone();
        let (done, pending) = async_channel::bounded(1);
        self.pending = Some(pending);
        self.runtime.spawn(async move {
            if let Some(previous) = previous {
                let _ = previous.recv().await;
            }
            let _ = sender.send(bytes).await;
            drop(done);
        });
    }

    pub fn pending(&self) -> Option<Receiver<()>> {
        self.pending
            .as_ref()
            .filter(|done| !done.is_closed())
            .cloned()
    }
}

impl Session {
    pub fn start(
        peer: &str,
        key: Key,
        attach: bool,
        ipc: Option<&Path>,
        runtime: &Handle,
    ) -> (Self, Receiver<Output>) {
        let (io, keys, sizes) = pty::Session::channels();
        let (output, events) = async_channel::bounded(32);
        let address = ipc
            .map(Path::to_owned)
            .unwrap_or_else(paxri_control::default_address);
        let peer = peer.to_owned();
        let (end, stop) = tokio::sync::oneshot::channel();
        let task = runtime.spawn(async move {
            let result = async {
                // The first layout supplies the content area after tabs and padding.
                let size = sizes.recv().await?;
                let request = if attach {
                    Request::Attach(key, size.into())
                } else {
                    Request::Create(key, size.into())
                };
                let (mut stream, _) =
                    paxri_control::managed_terminal(&address, peer, request).await?;
                let history = wire::managed::read_history(&mut stream).await?;
                if !history.is_empty() {
                    output.send(Output::Replay(history)).await?;
                }
                wire::managed::client(stream, keys, sizes, &output, stop).await
            }
            .await;
            let status = result
                .map(ExitStatus::with_exit_code)
                .map_err(|e| std::io::Error::other(format!("{e:#}")));
            let _ = output.send(Output::Exited(status)).await;
        });
        let input = Input {
            sender: io.input.clone(),
            runtime: runtime.clone(),
            pending: None,
        };
        (
            Self {
                io,
                size: None,
                input,
                task,
                end: Some(end),
            },
            events,
        )
    }

    pub fn end(&mut self) {
        if let Some(end) = self.end.take() {
            let _ = end.send(());
        }
    }

    pub fn resize(&mut self, size: PtySize) {
        if self.size != Some(size) {
            self.size = Some(size);
            self.io.resize(size);
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.task.abort();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn first_layout_sizes_the_shell_and_enter_is_forwarded_once() -> anyhow::Result<()> {
        use paxri_control::{Command, ControlListener, receive, respond};
        use std::time::Duration;
        use tokio::time::timeout;

        let dir = tempfile::Builder::new()
            .prefix("p2p-gui-terminal-")
            .tempdir()?;
        #[cfg(unix)]
        let ipc = dir.path().join("ipc/gateway.sock");
        #[cfg(windows)]
        let ipc = std::path::PathBuf::from(dir.path().file_name().unwrap());
        let gateway = ControlListener::bind(&ipc).await?;
        let (mut session, _output) =
            Session::start("pi", Key::new(1)?, false, Some(&ipc), &Handle::current());
        let quiet = Duration::from_millis(50);
        assert!(timeout(quiet, gateway.accept()).await.is_err());

        timeout(Duration::from_secs(5), async {
            let initial = PtySize {
                rows: 33,
                cols: 108,
                ..PtySize::default()
            };
            session.resize(initial);
            let mut stream = gateway.accept().await?;
            let command = receive(&mut stream).await?;
            assert!(matches!(command, Command::ManagedTerminal { peer, action: Request::Create(_, size) }
                if peer == "pi" && size == initial.into()));
            respond(&mut stream, Ok(Vec::<Key>::new())).await?;
            tokio::io::AsyncWriteExt::write_u32(&mut stream, 0).await?;

            session.resize(initial);
            assert!(
                timeout(quiet, wire::Frame::read(&mut stream))
                    .await
                    .is_err()
            );
            session.input.send(vec![b'\r']);
            let input = wire::Frame::read(&mut stream).await?;
            assert!(matches!(input, wire::Frame::Data(bytes) if bytes == b"\r"));
            let resized = PtySize {
                cols: 80,
                ..initial
            };
            session.resize(resized);
            let resize = wire::Frame::read(&mut stream).await?;
            assert!(matches!(resize, wire::Frame::Resize(size) if size == resized.into()));
            Ok(())
        })
        .await?
    }

    #[tokio::test]
    async fn backpressure_preserves_input_and_query_reply_order() {
        let (sender, receiver) = async_channel::bounded(1);
        let mut input = Input {
            sender,
            runtime: Handle::current(),
            pending: None,
        };
        let expected = [
            b"key".as_slice(),
            b"\x1b[<0;4;8M",
            b"\x1b[5;8R",
            b"\x1b[<0;4;8m",
        ];
        for bytes in expected {
            input.send(bytes.to_vec());
        }
        let drained = input.pending().unwrap();
        for bytes in expected {
            assert_eq!(receiver.recv().await.unwrap(), bytes);
        }
        assert!(drained.recv().await.is_err());
        assert!(input.pending().is_none());
    }
}
