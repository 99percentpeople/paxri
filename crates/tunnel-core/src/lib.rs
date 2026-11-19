//! TCP forwarding over authenticated reliable streams. Transport types stay in the adapter.
use std::{future::Future, io::ErrorKind, time::Duration};

use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    net::TcpStream,
    time::timeout,
};
use tunnel_config::{Config, Destination, DevicePlatform};
pub mod reverse;

pub const PROTOCOL: &[u8] = b"tcp-forward/1";
const HANDSHAKE: Duration = Duration::from_secs(10);
const TARGET_CONNECT: Duration = Duration::from_secs(6);
const MAX_FRAME: usize = 4096;

pub trait Transport: Send + Sync {
    type Stream: AsyncRead + AsyncWrite + Unpin + Send;
    /// Open a stream to the configured, authenticated peer. The adapter owns session reuse.
    fn open(&self, peer: &str) -> impl Future<Output = Result<Self::Stream>> + Send;
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "SCREAMING_SNAKE_CASE", deny_unknown_fields)]
enum Request {
    Open { service: String },
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

pub async fn open_service<T: Transport>(
    transport: &T,
    peer: &str,
    service: &str,
) -> Result<T::Stream> {
    open_destination(transport, peer, &Destination::Service(service.to_owned())).await
}

pub async fn peer_platform(transport: &impl Transport, peer: &str) -> Result<DevicePlatform> {
    let mut stream = transport.open(peer).await?;
    write_frame(&mut stream, &Request::DeviceInfo).await?;
    match read_frame(&mut stream).await? {
        Answer::DeviceInfo { platform } => Ok(platform),
        Answer::Error { code } => bail!("remote device info: {code}"),
        Answer::Ok => bail!("missing remote device info"),
    }
}

async fn open_destination<T: Transport>(
    transport: &T,
    peer: &str,
    destination: &Destination,
) -> Result<T::Stream> {
    let mut stream = transport.open(peer).await?;
    let request = match destination {
        Destination::Service(service) => Request::Open {
            service: service.clone(),
        },
        Destination::Target(target) => Request::OpenTcp {
            target: target.clone(),
        },
    };
    write_frame(&mut stream, &request).await?;
    match read_frame(&mut stream).await? {
        Answer::Error { code } => bail!("remote service {}: {code}", destination.address()),
        Answer::Ok => Ok(stream),
        Answer::DeviceInfo { .. } => bail!("unexpected device info in service response"),
    }
}

pub async fn forward(
    transport: &impl Transport,
    peer: &str,
    destination: &Destination,
    mut local: TcpStream,
) -> Result<()> {
    let mut stream = open_destination(transport, peer, destination).await?;
    tokio::io::copy_bidirectional(&mut local, &mut stream).await?;
    Ok(())
}

async fn accept_service(
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
    mut stream: impl AsyncRead + AsyncWrite + Unpin,
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
    // Authorize the completed request against the latest grants. A peer may
    // already have opened a QUIC stream when its access is revoked.
    let config = current_config();
    let allowed = |entry: &tunnel_config::Service| {
        entry.allow_peers.iter().any(|name| {
            config
                .peers
                .get(name)
                .is_some_and(|p| p.endpoint_id.eq_ignore_ascii_case(authenticated_id))
        })
    };
    let authorized = match &request {
        Request::DeviceInfo => {
            if config
                .peers
                .values()
                .any(|p| p.endpoint_id.eq_ignore_ascii_case(authenticated_id))
            {
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
            None
        }
        Request::Open { service } => config
            .services
            .get(service)
            .filter(|s| allowed(s))
            .map(|s| &s.target),
        Request::OpenTcp { target } => {
            // Dial only an exact locally granted endpoint, including a published TCP service.
            let grant = config.peers.values().any(|p| {
                p.endpoint_id.eq_ignore_ascii_case(authenticated_id)
                    && p.ports.connect.contains(target)
            });
            let published = config
                .services
                .values()
                .any(|s| allowed(s) && &s.target == target);
            (grant || published).then_some(target)
        }
    };
    let code = match authorized {
        None => "DENIED",
        Some(address) => match timeout(TARGET_CONNECT, TcpStream::connect(address)).await {
            Ok(Ok(tcp)) => return accept_service(&mut stream, tcp).await,
            Ok(Err(e)) if e.kind() == ErrorKind::ConnectionRefused => "TARGET_REFUSED",
            Ok(Err(e)) if e.kind() == ErrorKind::TimedOut => "TARGET_TIMEOUT",
            Ok(Err(_)) => "TARGET_UNREACHABLE",
            Err(_) => "TARGET_TIMEOUT",
        },
    };
    write_frame(&mut stream, &Answer::Error { code: code.into() }).await?;
    stream.shutdown().await?;
    Ok(())
}
