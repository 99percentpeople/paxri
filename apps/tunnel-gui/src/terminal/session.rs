use anyhow::{Context, Result};
use async_channel::{Receiver, Sender};
use portable_pty::{CommandBuilder, ExitStatus, PtySize, native_pty_system};
use std::{
    io::{Read, Write},
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
};

pub enum Output {
    Bytes(Vec<u8>),
    Error(String),
    Exited(std::io::Result<ExitStatus>),
}

pub struct Session {
    pub input: Sender<Vec<u8>>,
    sizes: Sender<PtySize>,
}

impl Session {
    pub fn start(
        peer: &str,
        ipc: Option<&Path>,
        size: PtySize,
    ) -> Result<(Self, Receiver<Output>)> {
        let cli = std::env::current_exe()?
            .with_file_name(format!("tunnel{}", std::env::consts::EXE_SUFFIX));
        let mut cmd = CommandBuilder::new(&cli);
        // portable-pty merges the Windows registry environment over the
        // process environment. Use the same SSH search path as the GUI caller.
        #[cfg(windows)]
        if let Some(path) = std::env::var_os("PATH") {
            cmd.env("PATH", path);
        }
        if let Some(path) = ipc {
            cmd.arg("--ipc");
            cmd.arg(path);
        }
        cmd.args(["ssh", "--", peer]);
        cmd.env("TERM", "xterm-256color");
        let pair = native_pty_system()
            .openpty(size)
            .context("open a terminal")?;
        let mut reader = pair.master.try_clone_reader()?;
        let mut writer = pair.master.take_writer()?;
        let mut child = pair
            .slave
            .spawn_command(cmd)
            .with_context(|| format!("start {}", cli.display()))?;
        drop(pair.slave);
        let (output, events) = async_channel::bounded(32);
        let ended = Arc::new(AtomicBool::new(false));
        let (sizes, changes) = async_channel::bounded(1);
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
                let _ = killer.kill();
            }
        });
        let exit_output = output.clone();
        thread::spawn(move || {
            let status = child.wait();
            ended.store(true, Ordering::Release);
            let _ = exit_output.send_blocking(Output::Exited(status));
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
                    // A Unix PTY can report EIO when the last slave closes.
                    #[cfg(unix)]
                    Err(e) if e.raw_os_error() == Some(5) => break,
                    Err(e) => {
                        let _ = output.send_blocking(Output::Error(e.to_string()));
                        break;
                    }
                }
            }
        });
        let (input, keys) = async_channel::bounded::<Vec<u8>>(128);
        thread::spawn(move || {
            while let Ok(data) = keys.recv_blocking() {
                if let Err(e) = writer.write_all(&data).and_then(|_| writer.flush()) {
                    let _ = errors.send_blocking(Output::Error(e.to_string()));
                    break;
                }
            }
        });
        Ok((Self { input, sizes }, events))
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
