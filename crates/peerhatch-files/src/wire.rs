//! Metadata frames and binary chunks share one reliable, ordered byte stream.
use super::Options;
use anyhow::{Result, bail, ensure};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

pub const CHUNK: usize = 64 * 1024;
const DATA: u32 = 1 << 31;
const MAX_HEADER: usize = 64 * 1024;

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Message {
    Put {
        path: String,
        options: Options,
        require_directory: bool,
    },
    Get {
        path: String,
        options: Options,
    },
    Directory {
        name: String,
        attrs: Option<Attributes>,
    },
    File {
        name: String,
        size: u64,
        attrs: Option<Attributes>,
    },
    EndDirectory,
    EndFile,
    Ready,
    Skipped,
    Done,
    Error {
        message: String,
    },
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Attributes {
    pub accessed: (i64, u32),
    pub modified: (i64, u32),
    pub mode: Option<u32>,
}

pub async fn write(w: &mut (impl AsyncWrite + Unpin), msg: &Message) -> Result<()> {
    let bytes = serde_json::to_vec(msg)?;
    ensure!(bytes.len() <= MAX_HEADER, "File transfer message too large");
    w.write_u32(bytes.len() as u32).await?;
    w.write_all(&bytes).await?;
    w.flush().await?;
    Ok(())
}

async fn header(r: &mut (impl AsyncRead + Unpin), length: u32) -> Result<Message> {
    ensure!(
        (1..=MAX_HEADER).contains(&(length as usize)),
        "Invalid file transfer header"
    );
    let mut bytes = vec![0; length as usize];
    r.read_exact(&mut bytes).await?;
    match serde_json::from_slice(&bytes)? {
        Message::Error { message } => bail!("Peer: {message}"),
        msg => Ok(msg),
    }
}

pub async fn read(r: &mut (impl AsyncRead + Unpin)) -> Result<Message> {
    let length = r.read_u32().await?;
    header(r, length).await
}

pub async fn write_data(w: &mut (impl AsyncWrite + Unpin), bytes: &[u8]) -> Result<()> {
    w.write_u32(DATA | bytes.len() as u32).await?;
    w.write_all(bytes).await?;
    Ok(())
}

/// Zero is the explicit end-of-file record. Transport EOF is always an error here.
pub async fn read_data(r: &mut (impl AsyncRead + Unpin), buffer: &mut [u8]) -> Result<usize> {
    let length = r.read_u32().await?;
    if length & DATA == 0 {
        ensure!(
            matches!(header(r, length).await?, Message::EndFile),
            "Expected end of file"
        );
        return Ok(0);
    }
    let length = (length ^ DATA) as usize;
    ensure!(
        (1..=buffer.len()).contains(&length),
        "Invalid file chunk size"
    );
    r.read_exact(&mut buffer[..length]).await?;
    Ok(length)
}
