//! Binary command I/O. A zero-length stdin chunk closes only the child's stdin.
use crate::MAX_DATA;
use anyhow::{Result, bail, ensure};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

pub const STDOUT: u8 = 1;
pub const STDERR: u8 = 2;
pub const EXIT: u8 = 3;

pub async fn write_chunk(out: &mut (impl AsyncWrite + Unpin), bytes: &[u8]) -> Result<()> {
    ensure!(
        bytes.len() <= MAX_DATA,
        "pipe chunk exceeds {MAX_DATA} bytes"
    );
    out.write_u16(bytes.len() as u16).await?;
    out.write_all(bytes).await?;
    out.flush().await?;
    Ok(())
}

pub async fn read_chunk(input: &mut (impl AsyncRead + Unpin), buf: &mut [u8]) -> Result<usize> {
    let size = usize::from(input.read_u16().await?);
    ensure!(
        size <= MAX_DATA && size <= buf.len(),
        "invalid pipe chunk length"
    );
    input.read_exact(&mut buf[..size]).await?;
    Ok(size)
}

pub async fn client(
    stream: impl AsyncRead + AsyncWrite + Unpin,
    mut stdin: impl AsyncRead + Unpin,
    mut stdout: impl AsyncWrite + Unpin,
    mut stderr: impl AsyncWrite + Unpin,
) -> Result<u32> {
    let (mut reader, mut writer) = tokio::io::split(stream);
    let upload = async {
        let mut buf = vec![0; MAX_DATA];
        loop {
            let n = stdin.read(&mut buf).await?;
            if let Err(error) = write_chunk(&mut writer, &buf[..n]).await {
                // A short-lived command can exit while the caller is still uploading.
                // Read its final output and status if the gateway closed its input.
                if error.downcast_ref::<std::io::Error>().is_some_and(|e| {
                    matches!(
                        e.kind(),
                        std::io::ErrorKind::BrokenPipe
                            | std::io::ErrorKind::ConnectionReset
                            | std::io::ErrorKind::ConnectionAborted
                            | std::io::ErrorKind::NotConnected
                    )
                }) {
                    return Ok(());
                }
                return Err(error);
            }
            if n == 0 {
                return Ok::<(), anyhow::Error>(());
            }
        }
    };
    let download = async {
        let mut buf = vec![0; MAX_DATA];
        loop {
            let dest: &mut (dyn AsyncWrite + Unpin) = match reader.read_u8().await? {
                STDOUT => &mut stdout,
                STDERR => &mut stderr,
                EXIT => return Ok(reader.read_u32().await?),
                _ => bail!("invalid pipe output frame"),
            };
            let size = read_chunk(&mut reader, &mut buf).await?;
            ensure!(size > 0, "empty pipe output frame");
            dest.write_all(&buf[..size]).await?;
            dest.flush().await?;
        }
    };
    tokio::pin!(download);
    tokio::select! {
        biased;
        result = &mut download => result,
        result = upload => { result?; download.await }
    }
}
