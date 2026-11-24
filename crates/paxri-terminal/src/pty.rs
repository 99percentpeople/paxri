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
        cmd: CommandBuilder,
        size: PtySize,
        managed: bool,
    ) -> Result<(Self, Receiver<Output>)> {
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
