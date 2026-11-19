//! One authenticated connection and PTY per native terminal session.
#[cfg(windows)]
mod windows;

use super::{Connection, NativeTransport, RecvStream, SendStream};
use crate::CONNECT_TIMEOUT;
use anyhow::{Context, Result, bail};
use iroh::protocol::{AcceptError, ProtocolHandler};
use std::sync::Arc;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    sync::{Semaphore, watch},
    time::timeout,
};
use tunnel_config::Config;
use tunnel_terminal::{
    self as wire, Frame, Size,
    pty::{CommandBuilder, Output, Session},
};

impl NativeTransport {
    pub async fn open_terminal(
        &self,
        peer: &str,
        size: Size,
    ) -> Result<(Connection, SendStream, RecvStream)> {
        size.validate()?;
        let dest = self.peer(peer)?;
        timeout(CONNECT_TIMEOUT, async {
            let conn = self.endpoint.connect(dest.address.clone(), wire::PROTOCOL).await
                .context("connect to native terminal (both gateways must support native terminals)")?;
            let (mut send, mut recv) = conn.open_bi().await?;
            size.write(&mut send).await?;
            match recv.read_u8().await? {
                wire::READY => Ok((conn, send, recv)),
                wire::DENIED => bail!("Terminal access denied. On the remote device, allow this peer under Device settings → Native terminal."),
                wire::BUSY => bail!("All native terminal sessions are in use"),
                wire::PRIVILEGED => bail!("The remote gateway must run as a non-elevated user to host a terminal"),
                wire::UNAVAILABLE => bail!("The remote gateway could not start its default shell"),
                _ => bail!("Invalid terminal handshake"),
            }
        }).await.context("opening native terminal timed out")?
    }
}

#[derive(Debug)]
pub(super) struct Terminals {
    config: watch::Receiver<Arc<Config>>,
    slots: Semaphore,
}

impl Terminals {
    pub(super) fn new(config: watch::Receiver<Arc<Config>>) -> Self {
        Self {
            config,
            slots: Semaphore::new(16),
        }
    }

    async fn serve(&self, conn: Connection) -> Result<()> {
        let id = conn.remote_id().to_string();
        let permitted = |cfg: &Config| {
            cfg.peers
                .values()
                .any(|p| p.allow_terminal && p.endpoint_id.eq_ignore_ascii_case(&id))
        };
        let mut config = self.config.clone();
        if !config
            .borrow()
            .peers
            .values()
            .any(|p| p.endpoint_id.eq_ignore_ascii_case(&id))
        {
            conn.close(1u32.into(), b"peer not paired");
            return Ok(());
        }
        let (mut send, mut recv) = timeout(CONNECT_TIMEOUT, conn.accept_bi()).await??;
        let size = timeout(CONNECT_TIMEOUT, Size::read(&mut recv)).await??;
        let prepared = if !permitted(&config.borrow_and_update()) {
            Err(wire::DENIED)
        } else if !can_host_shell()? {
            Err(wire::PRIVILEGED)
        } else if let Ok(slot) = self.slots.try_acquire() {
            #[cfg(windows)]
            let powershell = windows::default_shell().await;
            tokio::task::spawn_blocking(move || {
                #[cfg(windows)]
                let mut shell = CommandBuilder::new(powershell?);
                #[cfg(not(windows))]
                let mut shell = CommandBuilder::new_default_prog();
                shell.env("TERM", "xterm-256color");
                #[cfg(windows)]
                if let Some(home) = std::env::var_os("USERPROFILE") {
                    shell.cwd(home);
                }
                Session::spawn(shell, size.into())
            })
            .await?
            .map(|(session, events)| (session, events, slot))
            .map_err(|e| {
                eprintln!("start native terminal: {e:#}");
                wire::UNAVAILABLE
            })
        } else {
            Err(wire::BUSY)
        };
        match prepared {
            Ok((session, output, _slot)) if permitted(&config.borrow_and_update()) => {
                send.write_u8(wire::READY).await?;
                let input = async {
                    loop {
                        match Frame::read(&mut recv).await? {
                            Frame::Data(bytes) => {
                                if session.input.send(bytes).await.is_err() {
                                    // The shell is exiting. The output task still owns its trailing bytes and status.
                                    return std::future::pending().await;
                                }
                            }
                            Frame::Resize(size) => session.resize(size.into()),
                            Frame::Exit(_) => {
                                return Err(anyhow::anyhow!(
                                    "unexpected exit from terminal client"
                                ));
                            }
                        }
                    }
                };
                let stream_output = async {
                    loop {
                        match output.recv().await? {
                            Output::Bytes(bytes) => Frame::Data(bytes).write(&mut send).await?,
                            Output::Exited(result) => {
                                return Frame::Exit(result?.exit_code()).write(&mut send).await;
                            }
                            Output::Error(error) => bail!("terminal I/O: {error}"),
                        }
                    }
                };
                let result = tokio::select! {
                    biased;
                    _ = config.wait_for(|cfg| !permitted(cfg)) => Ok(()),
                    result = stream_output => result,
                    result = input => result,
                };
                drop(session);
                result?;
            }
            Ok(_) => send.write_u8(wire::DENIED).await?,
            Err(code) => send.write_u8(code).await?,
        }
        send.finish()?;
        let _ = timeout(CONNECT_TIMEOUT, send.stopped()).await;
        Ok(())
    }
}

impl ProtocolHandler for Terminals {
    async fn accept(&self, conn: Connection) -> Result<(), AcceptError> {
        self.serve(conn)
            .await
            .map_err(|e| AcceptError::from_boxed(e.into()))
    }
}

#[cfg(unix)]
fn can_host_shell() -> Result<bool> {
    Ok(!rustix::process::geteuid().is_root())
}

#[cfg(windows)]
fn can_host_shell() -> Result<bool> {
    use std::{
        mem,
        os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle},
    };
    use windows_sys::Win32::{
        Security::{GetTokenInformation, TOKEN_ELEVATION, TOKEN_QUERY, TokenElevation},
        System::Threading::{GetCurrentProcess, OpenProcessToken},
    };
    let mut raw = std::ptr::null_mut();
    // Only a standard user token may be used for a remote interactive shell.
    unsafe {
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut raw) == 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        let token = OwnedHandle::from_raw_handle(raw);
        let mut elevation: TOKEN_ELEVATION = mem::zeroed();
        let mut length = 0;
        if GetTokenInformation(
            token.as_raw_handle(),
            TokenElevation,
            (&raw mut elevation).cast(),
            mem::size_of_val(&elevation) as u32,
            &mut length,
        ) == 0
        {
            return Err(std::io::Error::last_os_error().into());
        }
        Ok(elevation.TokenIsElevated == 0)
    }
}
