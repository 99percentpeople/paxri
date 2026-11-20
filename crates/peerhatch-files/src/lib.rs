//! Native file copy over a paired, reliable duplex stream.
mod paths;
mod receive;
mod send;
mod wire;

use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use std::{path::Path, time::Duration};
use tokio::io::{AsyncRead, AsyncWrite};
use wire::Message;

const MAX_DEPTH: usize = 128;

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub struct Options {
    pub recursive: bool,
    pub preserve: bool,
    pub no_clobber: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    Copying,
    Copied,
    Skipped,
}

pub struct Progress<'a> {
    pub path: &'a Path,
    pub bytes: u64,
    pub size: u64,
    pub stage: Stage,
}

impl Progress<'_> {
    pub fn label(&self) -> String {
        let path = self.path.display();
        match self.stage {
            Stage::Skipped => format!("Skipped {path}"),
            Stage::Copied => format!("Copied {path} ({} bytes)", self.size),
            Stage::Copying => {
                let percent = self
                    .bytes
                    .saturating_mul(100)
                    .checked_div(self.size)
                    .unwrap_or(100);
                format!("{path}  {percent}%  ({} / {} bytes)", self.bytes, self.size)
            }
        }
    }
}

pub type Report<'a> = dyn FnMut(Progress<'_>) + Send + 'a;
trait Duplex: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> Duplex for T {}

pub enum Copy<'a> {
    Upload {
        source: &'a Path,
        destination: &'a str,
    },
    Download {
        source: &'a str,
        destination: &'a Path,
    },
}

async fn notify_error(stream: &mut impl Duplex, err: &anyhow::Error) {
    let message = format!("{err:#}").chars().take(2048).collect();
    let _ = tokio::time::timeout(
        Duration::from_secs(1),
        wire::write(stream, &Message::Error { message }),
    )
    .await;
}

pub async fn copy(
    mut stream: impl AsyncRead + AsyncWrite + Unpin + Send,
    copy: Copy<'_>,
    options: Options,
    require_directory: bool,
    report: &mut Report<'_>,
) -> Result<()> {
    let request = match copy {
        Copy::Upload { destination, .. } => Message::Put {
            path: destination.into(),
            options,
            require_directory,
        },
        Copy::Download { source, .. } => Message::Get {
            path: source.into(),
            options,
        },
    };
    wire::write(&mut stream, &request).await?;
    let ready = tokio::time::timeout(Duration::from_secs(30), wire::read(&mut stream))
        .await
        .context("Timed out starting file transfer")?
        .context("Open file transfer (both gateways must support file copy)")?;
    ensure!(
        matches!(ready, Message::Ready),
        "Invalid file transfer handshake"
    );
    let result = match copy {
        Copy::Upload { source, .. } => send::tree(&mut stream, source, options, report).await,
        Copy::Download {
            source,
            destination,
        } => {
            let expected = source
                .trim_end_matches(['/', '\\'])
                .rsplit(['/', '\\'])
                .next()
                .filter(|n| !matches!(*n, "" | "." | ".." | "~") && !n.ends_with(':'));
            receive::tree(
                &mut stream,
                destination,
                require_directory,
                expected,
                options,
                report,
            )
            .await
        }
    };
    if let Err(e) = &result {
        notify_error(&mut stream, e).await;
    }
    result
}

/// The gateway checks the current pairing and its account before allowing filesystem access.
pub async fn serve(
    mut stream: impl AsyncRead + AsyncWrite + Unpin + Send,
    host_allowed: bool,
) -> Result<()> {
    let result = async {
        let req = tokio::time::timeout(Duration::from_secs(20), wire::read(&mut stream)).await??;
        ensure!(
            host_allowed,
            "The remote gateway must run as a non-elevated user to host file transfers"
        );
        wire::write(&mut stream, &Message::Ready).await?;
        match req {
            Message::Put {
                path,
                options,
                require_directory,
            } => {
                let require_directory = require_directory || paths::trailing_separator(&path);
                receive::tree(
                    &mut stream,
                    &paths::host_path(&path)?,
                    require_directory,
                    None,
                    options,
                    &mut |_| {},
                )
                .await
            }
            Message::Get { path, options } => {
                send::tree(&mut stream, &paths::host_path(&path)?, options, &mut |_| {}).await
            }
            _ => bail!("Expected a file transfer request"),
        }
    }
    .await;
    if let Err(e) = &result {
        notify_error(&mut stream, e).await;
    }
    result
}

#[cfg(test)]
mod tests;
