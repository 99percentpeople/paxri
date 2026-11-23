//! Host paired terminal sessions and piped commands under the gateway account.
pub(super) mod managed;
mod pipe;
mod tools;
pub(crate) use tools::SessionTools;
#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub(crate) use windows::launch_if_requested;

use crate::{STREAM_TIMEOUT, streams::is_paired};
use anyhow::{Result, bail};
use paxri_config::Config;
use paxri_core::StreamKind;
use paxri_terminal::{
    self as wire, Frame, Size,
    exec::Invocation,
    pty::{CommandBuilder, Output, Session},
};
use std::sync::Arc;
use tokio::{
    io::AsyncWriteExt,
    sync::{Semaphore, watch},
    time::timeout,
};

pub use paxri_terminal::open;

/// Create a gateway-managed interactive shell for the local CLI.
pub(super) async fn open_shell<S: paxri_core::PeerSession>(
    session: S,
    size: Size,
) -> Result<S::Stream> {
    use wire::managed::{Key, Request, read_history, request};
    let (_, keys) = request(session.clone(), Request::List).await?;
    let number = keys.iter().map(|key| key.number).max().unwrap_or(0).saturating_add(1);
    let (mut stream, _) = request(session, Request::Create(Key::new(number)?, size)).await?;
    read_history(&mut stream).await?;
    Ok(stream)
}

pub(super) fn capabilities() -> paxri_config::DeviceCapabilities {
    let exec = crate::account::is_standard_user().unwrap_or(false);
    #[cfg(unix)]
    let shell = exec && std::path::Path::new(&CommandBuilder::new_default_prog().get_shell()).is_file();
    #[cfg(windows)]
    let shell = exec && windows::has_shell();
    paxri_config::DeviceCapabilities { shell, exec, managed_terminals: shell, forward_rules: Some(true), create_forward_rules: true }
}

pub(super) async fn watch_capabilities(transport: Arc<paxri_native::gateway::NativeTransport>) {
    loop {
        tokio::time::sleep(std::time::Duration::from_secs(10)).await;
        if let Ok(available) = tokio::task::spawn_blocking(capabilities).await {
            transport.set_capabilities(available);
        }
    }
}

async fn command_for(
    invocation: Option<Invocation>,
    tools: &SessionTools,
    caller_id: &str,
) -> Result<CommandBuilder> {
    let mut builder = match invocation {
        Some(Invocation::Program { program, args }) => {
            let mut builder = CommandBuilder::new(program);
            builder.args(args);
            builder
        }
        #[cfg(windows)]
        Some(Invocation::Shell { script }) => windows::shell_command(Some(&script)).await?,
        #[cfg(windows)]
        None => windows::shell_command(None).await?,
        #[cfg(unix)]
        default_or_script => {
            let mut shell = CommandBuilder::new_default_prog();
            if let Some(Invocation::Shell { script }) = default_or_script {
                shell = CommandBuilder::new(shell.get_shell());
                shell.arg("-c");
                shell.arg(script);
            }
            shell
        }
    };
    tools.apply(&mut builder, caller_id);
    #[cfg(windows)]
    if let Some(home) = std::env::var_os("USERPROFILE") {
        builder.cwd(home);
    }
    Ok(builder)
}

#[derive(Debug)]
pub(super) struct Terminals {
    config: watch::Receiver<Arc<Config>>,
    tools: SessionTools,
    slots: Arc<Semaphore>,
}

impl Terminals {
    pub(super) fn new(config: watch::Receiver<Arc<Config>>, tools: SessionTools) -> Self {
        Self {
            config,
            tools,
            slots: Arc::new(Semaphore::new(wire::managed::SESSION_LIMIT)),
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
        let size = if kind == StreamKind::Pipe {
            None
        } else {
            Some(timeout(STREAM_TIMEOUT, Size::read(&mut recv)).await??)
        };
        let admission = if !paired(&config.borrow_and_update()) {
            Err(wire::DENIED)
        } else if !crate::account::is_standard_user()? {
            Err(wire::PRIVILEGED)
        } else {
            self.slots.try_acquire().map_err(|_| wire::BUSY)
        };
        let _slot = match admission {
            Ok(slot) => slot,
            Err(code) => {
                send.write_u8(code).await?;
                send.shutdown().await?;
                return Ok(());
            }
        };
        let invocation = timeout(STREAM_TIMEOUT, Invocation::read(&mut recv)).await??;
        let command = command_for(Some(invocation), &self.tools, &id).await;
        let Some(size) = size else {
            return pipe::serve(command, recv, send, config, &id).await;
        };
        let latest = config.clone();
        let peer_id = id.clone();
        let prepared = tokio::task::spawn_blocking(move || {
            let config = latest.borrow();
            if !is_paired(&config, &peer_id) {
                return Err(wire::DENIED);
            }
            command
                .and_then(|mut cmd| {
                    cmd.env("TERM", "xterm-256color");
                    Session::spawn(cmd, size.into())
                })
                .map_err(|e| {
                    eprintln!("start native terminal: {e:#}");
                    wire::UNAVAILABLE
                })
        })
        .await?;
        match prepared {
            Ok((session, output)) if paired(&config.borrow_and_update()) => {
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
                            Output::Bytes(bytes) | Output::Replay(bytes) => Frame::Data(bytes).write(&mut send).await?,
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
