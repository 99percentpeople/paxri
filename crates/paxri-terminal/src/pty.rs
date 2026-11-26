use anyhow::{Context, Result};
use async_channel::{Receiver, Sender};
use portable_pty::native_pty_system;
pub use portable_pty::{CommandBuilder, ExitStatus, PtySize};
use std::{
    io::{Read, Write},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
};

#[cfg(unix)]
mod unix;

pub enum Output {
    /// Previously displayed bytes. Clients must discard generated terminal replies.
    Replay(Vec<u8>),
    Bytes(Vec<u8>),
    Error(String),
    Exited(std::io::Result<ExitStatus>),
}

pub struct Session {
    pub input: Sender<Vec<u8>>,
    sizes: Sender<PtySize>,
}

impl Session {
    pub fn channels() -> (Self, Receiver<Vec<u8>>, Receiver<PtySize>) {
        let (input, keys) = async_channel::bounded(128);
        let (sizes, changes) = async_channel::bounded(1);
        (Self { input, sizes }, keys, changes)
    }

    pub fn spawn(cmd: CommandBuilder, size: PtySize) -> Result<(Self, Receiver<Output>)> {
        Self::spawn_pty(cmd, size, false)
    }

    pub fn spawn_managed(cmd: CommandBuilder, size: PtySize) -> Result<(Self, Receiver<Output>)> {
        Self::spawn_pty(cmd, size, true)
    }

    fn spawn_pty(
        mut cmd: CommandBuilder,
        size: PtySize,
        managed: bool,
    ) -> Result<(Self, Receiver<Output>)> {
        // A remote PTY has its own display, independent of the gateway service's console.
        cmd.env("TERM", "xterm-256color");
        cmd.env("COLORTERM", "truecolor");
        cmd.env_remove("NO_COLOR");
        let pair = native_pty_system()
            .openpty(size)
            .context("open a terminal")?;
        #[cfg(unix)]
        let (mut reader, notify_exit) = if managed {
            let (reader, notify) = unix::Reader::new(&*pair.master)?;
            (Box::new(reader) as Box<dyn Read + Send>, Some(notify))
        } else {
            (pair.master.try_clone_reader()?, None)
        };
        #[cfg(not(unix))]
        let mut reader = {
            let _ = managed;
            pair.master.try_clone_reader()?
        };
        // portable-pty's Unix writer sends "\n\x04" on Drop even in raw mode, which injects
        // a newline into foreground TUIs (such as tmux sessions) when a PTY disconnects.
        #[cfg(unix)]
        let mut writer = unix::clone_master(&*pair.master)?;
        #[cfg(not(unix))]
        let mut writer = pair.master.take_writer()?;
        let mut child = pair
            .slave
            .spawn_command(cmd)
            .context("start terminal process")?;
        drop(pair.slave);
        let (output, events) = async_channel::bounded(32);
        let ended = Arc::new(AtomicBool::new(false));
        let (session, keys, changes) = Self::channels();
        let child_sizes = changes.clone();
        let child_input = keys.clone();
        let mut killer = child.clone_killer();
        let finished = ended.clone();
        let control_output = output.clone();
        // Resizing or closing a Windows pseudoconsole can wait for its clients and
        // output pipe. Keep the UI free to drain output while that happens.
        thread::spawn(move || {
            let master = pair.master;
            while let Ok(size) = changes.recv_blocking() {
                if let Err(e) = master.resize(size) {
                    let _ = control_output.try_send(Output::Error(format!("Resize failed: {e}")));
                }
            }
            if !finished.load(Ordering::Acquire) {
                #[cfg(unix)]
                if let Some(group) = master
                    .process_group_leader()
                    .and_then(rustix::process::Pid::from_raw)
                {
                    // A foreground job can be in a different process group from its shell.
                    let _ =
                        rustix::process::kill_process_group(group, rustix::process::Signal::HUP);
                }
                let _ = killer.kill();
            }
        });
        let (exit, status) = std::sync::mpsc::channel();
        thread::spawn(move || {
            let result = child.wait();
            ended.store(true, Ordering::Release);
            // Closing the master lets ConPTY flush its remaining output and signal EOF.
            child_sizes.close();
            child_input.close();
            let _ = exit.send(result);
            #[cfg(unix)]
            drop(notify_exit);
        });
        let errors = output.clone();
        thread::spawn(move || {
            let mut buf = [0; 8192];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        if output
                            .send_blocking(Output::Bytes(buf[..n].to_vec()))
                            .is_err()
                        {
                            break;
                        }
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                    #[cfg(windows)]
                    Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => break,
                    // A Unix PTY can report EIO when the last slave closes.
                    #[cfg(unix)]
                    Err(e) if e.raw_os_error() == Some(5) => break,
                    Err(e) => {
                        let _ = output.send_blocking(Output::Error(e.to_string()));
                        break;
                    }
                }
            }
            if let Ok(result) = status.recv() {
                let _ = output.send_blocking(Output::Exited(result));
            }
        });
        thread::spawn(move || {
            while let Ok(data) = keys.recv_blocking() {
                if let Err(e) = writer.write_all(&data).and_then(|_| writer.flush()) {
                    keys.close();
                    let _ = errors.send_blocking(Output::Error(e.to_string()));
                    break;
                }
            }
        });
        Ok((session, events))
    }

    pub fn resize(&self, size: PtySize) {
        let _ = self.sizes.force_send(size);
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.input.close();
        self.sizes.close();
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[test]
    fn dropping_session_does_not_inject_input_into_pty() {
        let dir = std::env::temp_dir().join(format!(
            "paxri-pty-drop-{}-{}",
            std::process::id(),
            getrandom::u64().unwrap()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let captured = dir.join("captured");
        let done = dir.join("done");

        let mut cmd = CommandBuilder::new("sh");
        cmd.args([
            "-c",
            "stty raw -echo; (trap '' HUP; cat <&0 > \"$1\"; printf ok > \"$2\") & printf READY; exec sleep 60",
            "_",
        ]);
        cmd.arg(&captured);
        cmd.arg(&done);

        let (session, output) = Session::spawn_managed(
            cmd,
            PtySize {
                rows: 24,
                cols: 80,
                ..PtySize::default()
            },
        )
        .unwrap();

        let deadline = Instant::now() + Duration::from_secs(5);
        let mut seen = Vec::new();
        while !seen.windows(5).any(|w| w == b"READY") {
            assert!(Instant::now() < deadline, "timed out waiting for PTY child");
            if let Ok(Output::Bytes(bytes)) = output.recv_blocking() {
                seen.extend(bytes);
            }
        }

        drop(session);
        // Drain output until the reader releases its duplicated master fd.
        while output.recv_blocking().is_ok() {}

        while !done.exists() {
            assert!(Instant::now() < deadline, "timed out waiting for slave EOF");
            thread::sleep(Duration::from_millis(10));
        }

        let bytes = std::fs::read(&captured).unwrap_or_default();
        let _ = std::fs::remove_dir_all(&dir);
        assert!(
            bytes.is_empty(),
            "unexpected bytes injected into PTY on disconnect: {bytes:?}"
        );
    }
}
