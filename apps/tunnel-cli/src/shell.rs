//! Interactive local console for a gateway-hosted PTY.
use anyhow::{Context, Result, bail, ensure};
use std::{io::IsTerminal, path::Path};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tunnel_control::terminal::Frame;
#[cfg(unix)]
mod unix;
#[cfg(unix)]
use unix as platform;
#[cfg(windows)]
mod windows;
#[cfg(windows)]
use windows as platform;

pub async fn run(ipc: &Path, peer: String) -> Result<u32> {
    ensure!(
        std::io::stdin().is_terminal() && std::io::stdout().is_terminal(),
        "Open a terminal window to start an interactive shell"
    );
    let mut changes = platform::Resize::new()?;
    let size = platform::size()?;
    let socket = tunnel_control::open_terminal(ipc, peer, size).await?;
    #[cfg(unix)]
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    #[cfg(unix)]
    let mut hangup = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::hangup())?;
    let _mode = platform::RawMode::enter()?;
    let (mut reader, mut writer) = tokio::io::split(socket);
    let send = async {
        let mut input = tokio::io::stdin();
        let mut bytes = [0; 8192];
        // The window may have changed while connecting.
        let mut current = platform::size()?;
        if current != size {
            Frame::Resize(current).write(&mut writer).await?;
        }
        loop {
            let frame = tokio::select! {
                n = input.read(&mut bytes) => {
                    let n = n?;
                    if n == 0 { return Ok(0); }
                    Frame::Data(bytes[..n].to_vec())
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
        let mut output = tokio::io::stdout();
        #[cfg(windows)]
        let mut pending = Vec::new();
        loop {
            match Frame::read(&mut reader)
                .await
                .context("remote terminal disconnected")?
            {
                Frame::Data(bytes) => {
                    #[cfg(unix)]
                    output.write_all(&bytes).await?;
                    #[cfg(windows)]
                    windows::write_console(&mut output, &mut pending, &bytes).await?;
                    output.flush().await?;
                }
                Frame::Exit(code) => return Ok(code),
                Frame::Resize(_) => bail!("unexpected resize from terminal host"),
            }
        }
    };
    let session = async {
        tokio::select! { result = receive => result, result = send => result }
    };
    #[cfg(unix)]
    return tokio::select! { result = session => result, _ = terminate.recv() => Ok(143), _ = hangup.recv() => Ok(129) };
    #[cfg(windows)]
    session.await
}
