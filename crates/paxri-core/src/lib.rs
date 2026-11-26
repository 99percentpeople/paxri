//! TCP forwarding over authenticated reliable streams. Transport types stay in the adapter.
use std::{
    io::ErrorKind,
    net::{Ipv4Addr, Ipv6Addr, SocketAddr},
    time::Duration,
};

use anyhow::{Context, Result, bail, ensure};
use paxri_config::{Config, DevicePlatform, tcp_target};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    net::TcpStream,
    time::timeout,
};
pub mod connection;
pub mod reverse;

pub mod session;
pub use session::{PeerSession, StreamKind, Transport};
const HANDSHAKE: Duration = Duration::from_secs(10);
const TARGET_CONNECT: Duration = Duration::from_secs(6);
const MAX_FRAME: usize = 4096;

#[derive(Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "SCREAMING_SNAKE_CASE", deny_unknown_fields)]
enum Request {
    OpenTcp { target: String },
    DeviceInfo,
}
#[derive(Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "SCREAMING_SNAKE_CASE")]
enum Answer {
    Ok,
    Error { code: String },
    DeviceInfo { platform: DevicePlatform },
}

async fn read_frame<T: DeserializeOwned>(stream: &mut (impl AsyncRead + Unpin)) -> Result<T> {
    timeout(HANDSHAKE, async {
        let length = stream.read_u32().await? as usize;
        ensure!(
            (1..=MAX_FRAME).contains(&length),
            "invalid handshake frame length"
        );
        let mut frame = vec![0; length];
        stream.read_exact(&mut frame).await?;
        Ok(serde_json::from_slice(&frame)?)
    })
    .await
    .context("handshake read timeout")?
}

async fn write_frame(stream: &mut (impl AsyncWrite + Unpin), frame: &impl Serialize) -> Result<()> {
    let bytes = serde_json::to_vec(frame)?;
    ensure!(bytes.len() <= MAX_FRAME, "handshake message too large");
    timeout(HANDSHAKE, async {
        stream.write_u32(bytes.len() as u32).await?;
        stream.write_all(&bytes).await?;
        stream.flush().await
    })
    .await??;
    Ok(())
}

pub async fn peer_platform(transport: &impl Transport, peer: &str) -> Result<DevicePlatform> {
    let mut stream = transport.open(peer, StreamKind::Services).await?;
    write_frame(&mut stream, &Request::DeviceInfo).await?;
    match read_frame(&mut stream).await? {
        Answer::DeviceInfo { platform } => Ok(platform),
        Answer::Error { code } => bail!("remote device info: {code}"),
        _ => bail!("missing remote device info"),
    }
}

pub async fn open_tcp<T: Transport>(
    transport: &T,
    peer: &str,
    target: &str,
) -> Result<<T::Session as PeerSession>::Stream> {
    let target = tcp_target(target).map_err(anyhow::Error::msg)?;
    let mut stream = transport.open(peer, StreamKind::Services).await?;
    let request = Request::OpenTcp {
        target: target.clone(),
    };
    write_frame(&mut stream, &request).await?;
    match read_frame(&mut stream).await? {
        Answer::Error { code } => bail!("remote TCP target {target}: {code}"),
        Answer::Ok => Ok(stream),
        _ => bail!("unexpected TCP response"),
    }
}

pub async fn forward(
    transport: &impl Transport,
    peer: &str,
    target: &str,
    mut local: TcpStream,
) -> Result<()> {
    let mut stream = open_tcp(transport, peer, target).await?;
    tokio::io::copy_bidirectional(&mut local, &mut stream).await?;
    Ok(())
}

/// Connect to a validated TCP target. When an IPv4 loopback target refuses a connection,
/// retry IPv6 loopback on the same port so bare-port rules reach IPv6-only local services.
pub async fn connect_tcp(target: &str) -> std::io::Result<TcpStream> {
    match timeout(TARGET_CONNECT, TcpStream::connect(target)).await {
        Ok(Ok(tcp)) => Ok(tcp),
        Ok(Err(err)) if err.kind() == ErrorKind::ConnectionRefused => {
            if let Ok(SocketAddr::V4(addr)) = target.parse::<SocketAddr>()
                && *addr.ip() == Ipv4Addr::LOCALHOST
            {
                let v6 = SocketAddr::from((Ipv6Addr::LOCALHOST, addr.port()));
                if let Ok(Ok(tcp)) = timeout(TARGET_CONNECT, TcpStream::connect(v6)).await {
                    return Ok(tcp);
                }
            }
            Err(err)
        }
        Ok(Err(err)) => Err(err),
        Err(_) => Err(std::io::Error::new(
            ErrorKind::TimedOut,
            "target connect timed out",
        )),
    }
}

async fn accept_tcp(
    stream: &mut (impl AsyncRead + AsyncWrite + Unpin),
    mut target: impl AsyncRead + AsyncWrite + Unpin,
) -> Result<()> {
    write_frame(stream, &Answer::Ok).await?;
    tokio::io::copy_bidirectional(stream, &mut target).await?;
    Ok(())
}

pub async fn serve(
    current_config: impl FnOnce() -> std::sync::Arc<Config>,
    authenticated_id: &str,
    stream: impl AsyncRead + AsyncWrite + Unpin,
) -> Result<()> {
    serve_services(current_config, authenticated_id, stream, true).await
}

/// A client-only endpoint still answers authenticated device probes from older gateways.
pub async fn serve_device_info(
    current_config: impl FnOnce() -> std::sync::Arc<Config>,
    authenticated_id: &str,
    stream: impl AsyncRead + AsyncWrite + Unpin,
) -> Result<()> {
    serve_services(current_config, authenticated_id, stream, false).await
}

async fn serve_services(
    current_config: impl FnOnce() -> std::sync::Arc<Config>,
    authenticated_id: &str,
    mut stream: impl AsyncRead + AsyncWrite + Unpin,
    allow_tcp: bool,
) -> Result<()> {
    let request = match read_frame(&mut stream).await {
        Ok(request) => request,
        Err(e) => {
            write_frame(
                &mut stream,
                &Answer::Error {
                    code: "INVALID_REQUEST".into(),
                },
            )
            .await?;
            return Err(e);
        }
    };
    // Check the current pairing after reading the request. A peer may
    // already have opened a QUIC stream when its access is revoked.
    let config = current_config();
    if !config.is_paired(authenticated_id) {
        write_frame(
            &mut stream,
            &Answer::Error {
                code: "DENIED".into(),
            },
        )
        .await?;
        stream.shutdown().await?;
        return Ok(());
    }
    let target = match &request {
        Request::DeviceInfo => {
            write_frame(
                &mut stream,
                &Answer::DeviceInfo {
                    platform: DevicePlatform::current(),
                },
            )
            .await?;
            stream.shutdown().await?;
            return Ok(());
        }
        Request::OpenTcp { .. } if !allow_tcp => {
            write_frame(&mut stream, &Answer::Error { code: "UNSUPPORTED".into() }).await?;
            stream.shutdown().await?;
            return Ok(());
        }
        Request::OpenTcp { target } => tcp_target(target),
    };
    let code = match target {
        Err(_) => "INVALID_REQUEST",
        Ok(address) => match connect_tcp(&address).await {
            Ok(tcp) => return accept_tcp(&mut stream, tcp).await,
            Err(e) if e.kind() == ErrorKind::ConnectionRefused => "TARGET_REFUSED",
            Err(e) if e.kind() == ErrorKind::TimedOut => "TARGET_TIMEOUT",
            Err(_) => "TARGET_UNREACHABLE",
        },
    };
    write_frame(&mut stream, &Answer::Error { code: code.into() }).await?;
    stream.shutdown().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::TcpListener;

    #[tokio::test]
    async fn connect_tcp_falls_back_to_ipv6_loopback_for_ipv4_loopback_target() {
        let Ok(listener) = TcpListener::bind((Ipv6Addr::LOCALHOST, 0)).await else {
            return;
        };
        let port = listener.local_addr().unwrap().port();
        // Ensure IPv4 loopback on the same port is actually closed so the fallback path runs.
        if TcpStream::connect((Ipv4Addr::LOCALHOST, port)).await.is_ok() {
            return;
        }
        let target = format!("127.0.0.1:{port}");
        let (client, accepted) = tokio::join!(connect_tcp(&target), listener.accept());
        assert!(client.is_ok(), "IPv6 loopback fallback failed: {client:?}");
        assert!(accepted.is_ok());
    }

    #[tokio::test]
    async fn connect_tcp_reports_refused_when_loopback_port_is_closed() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        let err = connect_tcp(&format!("127.0.0.1:{port}")).await.unwrap_err();
        assert_eq!(err.kind(), ErrorKind::ConnectionRefused);
    }
}
