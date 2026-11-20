//! One PTY per authenticated terminal stream.
#[cfg(windows)]
mod windows;

use crate::{STREAM_TIMEOUT, streams::is_paired};
use anyhow::{Context, Result, bail};
use std::sync::Arc;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    sync::{Semaphore, watch},
    time::timeout,
};
use peerhatch_config::Config;
use peerhatch_core::{PeerSession, StreamKind, Transport};
use peerhatch_terminal::{
    self as wire, Frame, Size,
    exec::Invocation,
    pty::{CommandBuilder, Output, Session},
};

pub async fn open_pty<T: Transport>(
    transport: &T,
    peer: &str,
    size: Size,
    invocation: Option<Invocation>,
) -> Result<<T::Session as PeerSession>::Stream> {
    size.validate()?;
    let kind = if invocation.is_some() {
        StreamKind::Command
    } else {
        StreamKind::Terminal
    };
    timeout(STREAM_TIMEOUT, async {
        let mut stream = transport.open(peer, kind).await?;
        size.write(&mut stream).await?;
        if let Some(command) = &invocation {
            command.write(&mut stream).await?;
        }
        match stream.read_u8().await? {
            wire::READY => Ok(stream),
            wire::DENIED => bail!(
                "The remote gateway denied the terminal request. Check that the devices are paired."
            ),
            wire::BUSY => bail!("All native terminal sessions are in use"),
            wire::PRIVILEGED => {
                bail!("The remote gateway must run as a non-elevated user to host a terminal")
            }
            wire::UNAVAILABLE if invocation.is_some() => {
                bail!("The remote gateway could not start the requested program")
            }
            wire::UNAVAILABLE => bail!("The remote gateway could not start its default shell"),
            _ => bail!("Invalid terminal handshake"),
        }
    })
    .await
    .context("opening native terminal timed out")?
}

async fn command_for(invocation: Option<Invocation>) -> Result<CommandBuilder> {
    let mut builder = match invocation {
        Some(Invocation::Program { program, args }) => {
            let mut builder = CommandBuilder::new(program);
            builder.args(args);
            builder
        }
        default_or_script => {
            #[cfg(windows)]
            let mut shell = CommandBuilder::new(windows::default_shell().await?);
            #[cfg(unix)]
            let mut shell = CommandBuilder::new_default_prog();
            if let Some(Invocation::Shell { script }) = default_or_script {
                #[cfg(unix)]
                {
                    shell = CommandBuilder::new(shell.get_shell());
                    shell.arg("-c");
                }
                #[cfg(windows)]
                shell.args(["-NoLogo", "-NoProfile", "-Command"]);
                shell.arg(script);
            }
            shell
        }
    };
    builder.env("TERM", "xterm-256color");
    #[cfg(windows)]
    if let Some(home) = std::env::var_os("USERPROFILE") {
        builder.cwd(home);
    }
    Ok(builder)
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

    pub async fn serve(
        &self,
        id: String,
        kind: StreamKind,
        stream: impl tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
    ) -> Result<()> {
        let paired = |cfg: &Config| is_paired(cfg, &id);
        let mut config = self.config.clone();
        let (mut recv, mut send) = tokio::io::split(stream);
        let size = timeout(STREAM_TIMEOUT, Size::read(&mut recv)).await??;
        let prepared = if !paired(&config.borrow_and_update()) {
            Err(wire::DENIED)
        } else if !crate::account::is_standard_user()? {
            Err(wire::PRIVILEGED)
        } else if let Ok(slot) = self.slots.try_acquire() {
            let invocation = if kind == StreamKind::Command {
                Some(timeout(STREAM_TIMEOUT, Invocation::read(&mut recv)).await??)
            } else {
                None
            };
            let command = command_for(invocation).await;
            let latest = config.clone();
            let peer_id = id.clone();
            tokio::task::spawn_blocking(move || {
                let config = latest.borrow();
                if !is_paired(&config, &peer_id) {
                    return Err(wire::DENIED);
                }
                command
                    .and_then(|cmd| Session::spawn(cmd, size.into()))
                    .map_err(|e| {
                        eprintln!("start native terminal: {e:#}");
                        wire::UNAVAILABLE
                    })
            })
            .await?
            .map(|(session, events)| (session, events, slot))
        } else {
            Err(wire::BUSY)
        };
        match prepared {
            Ok((session, output, _slot)) if paired(&config.borrow_and_update()) => {
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
                    _ = config.wait_for(|cfg| !paired(cfg)) => Ok(()),
                    result = stream_output => result,
                    result = input => result,
                };
                drop(session);
                result?;
            }
            Ok(_) => send.write_u8(wire::DENIED).await?,
            Err(code) => send.write_u8(code).await?,
        }
        send.shutdown().await?;
        Ok(())
    }
}
