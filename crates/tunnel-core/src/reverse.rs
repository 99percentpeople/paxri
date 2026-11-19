//! One control stream per remote listener, plus a peer-opened stream per accepted socket.
use super::{read_frame, write_frame};
use anyhow::{Result, bail, ensure};
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use tokio::io::{AsyncRead, AsyncWrite};

pub const PROTOCOL: &[u8] = b"tcp-listen/1";
pub const INCOMING: u8 = 1;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Listen {
    listen: SocketAddr,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Reply {
    Bound(SocketAddr),
    Error(String),
}

pub async fn request(
    send: &mut (impl AsyncWrite + Unpin),
    recv: &mut (impl AsyncRead + Unpin),
    listen: SocketAddr,
) -> Result<SocketAddr> {
    write_frame(send, &Listen { listen }).await?;
    match read_frame(recv).await? {
        Reply::Bound(addr) => {
            ensure!(
                addr.ip() == listen.ip()
                    && addr.port() != 0
                    && (listen.port() == 0 || listen.port() == addr.port()),
                "Peer returned an unexpected listen address"
            );
            Ok(addr)
        }
        Reply::Error(error) => bail!("{error}"),
    }
}

pub async fn receive(recv: &mut (impl AsyncRead + Unpin)) -> Result<SocketAddr> {
    Ok(read_frame::<Listen>(recv).await?.listen)
}

pub async fn respond(
    send: &mut (impl AsyncWrite + Unpin),
    result: Result<SocketAddr, String>,
) -> Result<()> {
    let reply = match result {
        Ok(addr) => Reply::Bound(addr),
        Err(reason) => Reply::Error(reason),
    };
    write_frame(send, &reply).await
}
