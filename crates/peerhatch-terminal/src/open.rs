//! Open a terminal or command stream on an authenticated peer.
use crate::{self as wire, Size, exec::Invocation};
use anyhow::{Context, Result, bail};
use peerhatch_core::{PeerSession, StreamKind};
use std::time::Duration;
use tokio::{io::AsyncReadExt, time::timeout};

pub async fn open<S: PeerSession>(
    session: S,
    size: Option<Size>,
    invocation: Option<Invocation>,
) -> Result<S::Stream> {
    let kind = match (size, &invocation) {
        (Some(_), Some(_)) => StreamKind::Command,
        (Some(_), None) => StreamKind::Terminal,
        (None, Some(_)) => StreamKind::Pipe,
        (None, None) => bail!("missing piped command"),
    };
    timeout(Duration::from_secs(20), async {
        let mut stream = session.open(kind).await?;
        if let Some(size) = size {
            size.write(&mut stream).await?;
        }
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
    .with_context(|| {
        if kind == StreamKind::Pipe {
            "Opening piped command (the remote gateway must support --pipe)"
        } else {
            "Opening native terminal"
        }
    })
}
