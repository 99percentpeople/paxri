//! Interactive local console for a gateway-hosted PTY.
use anyhow::{Context, Result, bail, ensure};
use std::{io::IsTerminal, path::Path};
use tokio::io::{AsyncReadExt, AsyncWrite, AsyncWriteExt};
use paxri_control::terminal::{Frame, Size, exec::Invocation};
mod capture;
#[cfg(unix)]
mod unix;
#[cfg(unix)]
use unix as platform;
#[cfg(windows)]
mod windows;
#[cfg(windows)]
use windows as platform;

pub(super) fn terminal_columns() -> Option<usize> {
    terminal_size().map(|size| usize::from(size.cols))
}

pub(super) fn terminal_size() -> Option<Size> {
    platform::size().ok()
}

pub async fn pipe(ipc: &Path, peer: String, invocation: Invocation) -> Result<u32> {
    let stream = paxri_control::open_pipe(ipc, peer, invocation).await?;
    let stdout: Box<dyn AsyncWrite + Unpin> = if std::io::stdout().is_terminal() {
        Box::new(tokio::io::stdout())
    } else {
        Box::new(crate::stdio::output()?)
    };
    paxri_control::terminal::pipe::client(
        stream,
        crate::stdio::input()?,
        stdout,
        crate::stdio::error()?,
    )
    .await
}

pub async fn run(ipc: &Path, peer: String, invocation: Option<Invocation>) -> Result<u32> {
    let console_output = std::io::stdout().is_terminal();
    let attached = std::io::stdin().is_terminal() && console_output;
    ensure!(
        attached || invocation.is_some(),
        "Open a terminal window to start an interactive shell"
    );
    let size = if console_output {
        platform::size()?
    } else {
        Size { rows: 24, cols: 80 }
    };
    let socket = paxri_control::open_pty(ipc, peer, size, invocation).await?;
    #[cfg(unix)]
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    #[cfg(unix)]
    let mut hangup = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::hangup())?;
    let _mode = attached.then(platform::RawMode::enter).transpose()?;
    let (mut reader, mut writer) = tokio::io::split(socket);
    let (replies, mut terminal_replies) = tokio::sync::mpsc::channel(4);
    let send = async {
        if !attached {
            let mut input = crate::stdio::input()?;
            let mut bytes = [0; 8192];
            let mut open = true;
            loop {
                let data = tokio::select! {
                    biased;
                    reply = terminal_replies.recv() => reply.context("terminal reply channel closed")?,
                    read = input.read(&mut bytes), if open => {
                        let n = read?;
                        if n == 0 {
                            // The PTY can still issue queries and send output after local EOF.
                            open = false;
                            continue;
                        }
                        bytes[..n].to_vec()
                    }
                };
                Frame::Data(data).write(&mut writer).await?;
            }
        }
        let mut changes = platform::Resize::new()?;
        let mut input = platform::Input::new();
        // The window may have changed while connecting.
        let mut current = platform::size()?;
        if current != size {
            Frame::Resize(current).write(&mut writer).await?;
        }
        loop {
            let frame = tokio::select! {
                data = input.next() => {
                    let Some(bytes) = data? else { return Ok(0); };
                    Frame::Data(bytes)
                }
                result = changes.next() => {
                    result?;
                    let next = platform::size()?;
                    if next == current { continue; }
                    current = next;
                    Frame::Resize(next)
                }
            };
            frame.write(&mut writer).await?;
        }
    };
    let receive = async {
        let mut capture = (!attached).then(|| capture::RedirectedTerminal::new(size));
        let mut output: Box<dyn AsyncWrite + Unpin> = if console_output {
            Box::new(tokio::io::stdout())
        } else {
            Box::new(crate::stdio::output()?)
        };
        #[cfg(windows)]
        let mut pending = Vec::new();
        loop {
            match Frame::read(&mut reader)
                .await
                .context("remote terminal disconnected")?
            {
                Frame::Data(bytes) => {
                    if let Some(term) = &mut capture {
                        let reply = term.replies(&bytes);
                        if !reply.is_empty() {
                            replies.send(reply).await?;
                        }
                    }
                    #[cfg(unix)]
                    output.write_all(&bytes).await?;
                    #[cfg(windows)]
                    if console_output {
                        windows::write_console(&mut output, &mut pending, &bytes).await?;
                    } else {
                        output.write_all(&bytes).await?;
                    }
                    output.flush().await?;
                }
                Frame::Exit(code) => return Ok(code),
                Frame::Resize(_) => bail!("unexpected resize from terminal host"),
            }
        }
    };
    let session = async {
        tokio::select! { biased; result = receive => result, result = send => result }
    };
    #[cfg(unix)]
    return tokio::select! { result = session => result, _ = terminate.recv() => Ok(143), _ = hangup.recv() => Ok(129) };
    #[cfg(windows)]
    session.await
}
