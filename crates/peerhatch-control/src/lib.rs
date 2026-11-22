//! Versioned local management protocol shared by the gateway, CLI and desktop.
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::{Context, Result, ensure};
pub use interprocess::local_socket::tokio::Stream as ControlStream;
use interprocess::local_socket::{
    ListenerOptions, Name,
    tokio::{Listener, Stream, prelude::*},
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
pub use peerhatch_config::{
    DevicePlatform, Direction, Forward, listen_address, tcp_target,
};

pub mod forwarding;
pub use forwarding::{PeerForward, ForwardDirectory};
pub use peerhatch_config::gui::{GuiChange, GuiSettings};
pub use peerhatch_files as files;
pub use peerhatch_terminal as terminal;

const VERSION: u16 = 1;
const MAX_FRAME: usize = 1024 * 1024;
const DEADLINE: Duration = Duration::from_secs(5);

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "command", rename_all = "snake_case")]
pub enum Command {
    Status,
    WatchPeerConnections,
    ListForwards,
    ManagePeerForward { owner: String, action: PeerForward },
    CheckPeer {
        peer: String,
    },
    Shutdown,
    RefreshPairingCode,
    CancelPairing,
    Unpair {
        peer: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_id: Option<String>,
    },
    JoinPairing {
        code: String,
    },
    AnswerPairing {
        device_id: String,
        request_id: Option<String>,
        accept: bool,
    },
    SetForward {
        name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_peer: Option<String>,
        enabled: bool,
    },
    SaveForward {
        name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_peer: Option<String>,
        edit: bool,
        #[serde(flatten)]
        rule: Forward,
    },
    RemoveForward {
        name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_peer: Option<String>,
    },
    SetDeviceName {
        name: Option<String>,
    },
    SetNative {
        change: peerhatch_config::NativeChange,
    },
    SetGui {
        change: GuiChange,
    },
    OpenFiles {
        peer: String,
    },
    OpenTerminal {
        peer: String,
        size: terminal::Size,
    },
    Exec {
        peer: String,
        size: terminal::Size,
        invocation: terminal::exec::Invocation,
    },
    ExecPipe {
        peer: String,
        invocation: terminal::exec::Invocation,
    },
    Connect {
        peer: String,
        target: String,
    },
}

#[derive(Serialize, Deserialize)]
struct Request {
    version: u16,
    #[serde(flatten)]
    command: Command,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ForwardStatus {
    pub listen: String,
    pub bound: Option<String>,
    pub peer: String,
    #[serde(default)]
    pub target: String,
    #[serde(default)]
    pub direction: Direction,
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default)]
    pub error: Option<String>,
}

impl ForwardStatus {
    pub fn enabled(&self) -> bool {
        self.enabled.unwrap_or(self.bound.is_some())
    }

    pub fn state(&self) -> &str {
        if !self.enabled() {
            "Paused"
        } else if let Some(error) = &self.error {
            error
        } else if self.bound.is_some() {
            "Listening"
        } else {
            "Connecting…"
        }
    }

    pub fn route(&self) -> String {
        let listen = self.bound.as_ref().unwrap_or(&self.listen);
        let dest = &self.target;
        match self.direction {
            Direction::Local => format!("This device / {listen} → {} / {dest}", self.peer),
            Direction::Remote => format!("{} / {listen} → This device / {dest}", self.peer),
        }
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct PairingStatus {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pending_unpairs: Vec<String>,
    pub code: Option<String>,
    pub incoming: Vec<PairingPeer>,
    pub outgoing: Option<String>,
    pub message: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct PairingPeer {
    pub request_id: String,
    pub name: String,
    pub device_id: String,
    #[serde(default)]
    pub platform: DevicePlatform,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Status {
    #[serde(default)]
    pub peer_presence: bool,
    /// Absent on gateways that predate editable network settings.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native: Option<peerhatch_config::Native>,
    #[serde(default)]
    pub network_restart_required: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub executable: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ipc_path: Option<PathBuf>,
    #[serde(default)]
    pub user_managed: bool,
    #[serde(default)]
    pub startup: peerhatch_config::startup::Startup,
    #[serde(default)]
    pub config_path: Option<PathBuf>,
    #[serde(default)]
    pub gui: GuiSettings,
    #[serde(default)]
    pub pairing: PairingStatus,
    pub device_id: String,
    #[serde(default)]
    pub computer_name: String,
    #[serde(default)]
    pub device_name: Option<String>,
    pub ticket: String,
    pub peers: BTreeMap<String, String>,
    #[serde(default)]
    pub peer_platforms: BTreeMap<String, DevicePlatform>,
    /// Absent when connected to an older gateway. An empty list is a known idle peer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub peer_connections: Option<BTreeMap<String, Vec<PeerConnection>>>,
    /// Last pairing or Connected/Idle transition by peer alias, in Unix milliseconds.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub peer_last_status_change_ms: BTreeMap<String, i64>,
    pub forwards: BTreeMap<String, ForwardStatus>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub peer_forwards: Option<ForwardDirectory>,
}

pub use peerhatch_core::{
    StreamKind,
    connection::{ConnectionTraffic, PathKind, PeerConnection, TrafficRates, format_bytes, unix_time_ms},
};

impl Status {
    pub fn resolve_peer(&self, selector: &str) -> Result<(&str, &str)> {
        let (name, id) = peerhatch_config::peer_id::resolve(&self.peers, selector, String::as_str)?;
        Ok((name, id))
    }

    pub fn short_peer_ids(&self) -> BTreeMap<&str, String> {
        peerhatch_config::peer_id::short_ids(&self.peers)
    }

    pub fn peers_by_last_change(&self) -> Vec<(&String, &String)> {
        let mut peers: Vec<_> = self.peers.iter().collect();
        // Stable sorting keeps aliases in name order for equal or unknown times.
        peers.sort_by_cached_key(|(name, _)| {
            std::cmp::Reverse(self.peer_last_status_change_ms.get(*name).copied())
        });
        peers
    }

    pub fn connections_for(&self, peer: &str) -> Option<&[PeerConnection]> {
        self.peer_connections.as_ref()?.get(peer).map(Vec::as_slice)
    }

    pub fn peer_state(&self, peer: &str) -> &'static str {
        match self.connections_for(peer) {
            None => "Unavailable",
            Some([]) if self.peer_presence => "Offline",
            Some([]) => "Disconnected",
            Some(_) => "Connected",
        }
    }

    pub fn peer_online(&self, peer: &str) -> bool {
        self.connections_for(peer).is_some_and(|paths| !paths.is_empty())
    }

    pub fn peer_platform(&self, peer: &str) -> DevicePlatform {
        self.peer_platforms.get(peer).copied().unwrap_or_default()
    }

    pub fn display_name(&self) -> &str {
        self.device_name.as_deref().unwrap_or_else(|| {
            if self.computer_name.is_empty() {
                &self.device_id
            } else {
                &self.computer_name
            }
        })
    }
}

#[derive(Serialize, Deserialize)]
struct Reply<T> {
    version: u16,
    result: Result<T, String>,
}

pub fn default_address() -> PathBuf {
    if let Some(address) = std::env::var_os("PEERHATCH_IPC") {
        return address.into();
    }
    #[cfg(windows)]
    return PathBuf::from("peerhatch-gateway");
    #[cfg(unix)]
    {
        #[cfg(target_os = "linux")]
        let system = Path::new("/run/peerhatch/gateway.sock");
        #[cfg(target_os = "macos")]
        let system = Path::new("/Library/Application Support/PeerHatch/run/gateway.sock");
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        if system.exists() {
            return system.into();
        }
        let base = std::env::var_os("XDG_RUNTIME_DIR").or_else(|| std::env::var_os("HOME"));
        base.map(PathBuf::from)
            .unwrap_or_default()
            .join(".peerhatch/gateway.sock")
    }
}

fn socket_name(path: &Path) -> Result<Name<'_>> {
    #[cfg(unix)]
    return Ok(path.to_fs_name::<interprocess::local_socket::GenericFilePath>()?);
    #[cfg(windows)]
    {
        ensure!(
            path.components().count() == 1 && !path.as_os_str().is_empty(),
            "use a local pipe name"
        );
        Ok(path
            .as_os_str()
            .to_ns_name::<interprocess::local_socket::GenericNamespaced>()?)
    }
}

pub struct ControlListener {
    listener: Listener,
    // Keep the advisory lock until after the listener has unlinked its socket.
    #[cfg(unix)]
    _lock: std::fs::File,
}

impl ControlListener {
    pub async fn bind(path: &Path) -> Result<Self> {
        let options = ListenerOptions::new().name(socket_name(path)?);
        #[cfg(unix)]
        {
            use std::{
                fs,
                io::ErrorKind,
                os::unix::fs::{DirBuilderExt, FileTypeExt, MetadataExt, PermissionsExt},
            };
            let parent = path
                .parent()
                .filter(|p| !p.as_os_str().is_empty())
                .context("IPC requires a dedicated socket directory")?;
            fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(parent)?;
            let meta = fs::symlink_metadata(parent)?;
            ensure!(
                meta.is_dir()
                    && meta.uid() == rustix::process::geteuid().as_raw()
                    && meta.mode() & 0o027 == 0,
                "IPC directory must be owned by the gateway account with mode 0700 or 0750: {}",
                parent.display()
            );
            let mut lock_name = path.as_os_str().to_owned();
            lock_name.push(".lock");
            let lock = fs::OpenOptions::new()
                .create(true)
                .truncate(false)
                .write(true)
                .open(lock_name)?;
            lock.try_lock()
                .context("another gateway owns this IPC address")?;
            match fs::symlink_metadata(path) {
                Ok(meta) if meta.file_type().is_socket() => {
                    match tokio::time::timeout(
                        Duration::from_secs(1),
                        tokio::net::UnixStream::connect(path),
                    )
                    .await?
                    {
                        Ok(_) => anyhow::bail!("IPC address already has a listener"),
                        Err(e) if e.kind() == ErrorKind::ConnectionRefused => {
                            fs::remove_file(path)?
                        }
                        Err(e) if e.kind() == ErrorKind::NotFound => {}
                        Err(e) => return Err(e.into()),
                    }
                }
                Ok(_) => anyhow::bail!("IPC path is occupied by a non-socket file"),
                Err(e) if e.kind() == ErrorKind::NotFound => {}
                Err(e) => return Err(e.into()),
            }
            let listener = options.create_tokio()?;
            fs::set_permissions(path, fs::Permissions::from_mode(0o660))?;
            Ok(Self {
                listener,
                _lock: lock,
            })
        }
        #[cfg(windows)]
        {
            use interprocess::os::windows::{
                local_socket::ListenerOptionsExt, security_descriptor::SecurityDescriptor,
            };
            let acl = SecurityDescriptor::deserialize(widestring::u16cstr!(
                "D:P(A;;GA;;;SY)(A;;GA;;;BA)(A;;GA;;;OW)"
            ))?;
            Ok(Self {
                listener: options.security_descriptor(acl).create_tokio()?,
            })
        }
    }

    pub async fn accept(&self) -> Result<Stream> {
        Ok(self.listener.accept().await?)
    }
}

async fn read<T: DeserializeOwned>(socket: &mut (impl AsyncRead + Unpin)) -> Result<T> {
    let size = socket.read_u32().await? as usize;
    ensure!(
        (1..=MAX_FRAME).contains(&size),
        "invalid management frame length"
    );
    let mut bytes = vec![0; size];
    socket.read_exact(&mut bytes).await?;
    Ok(serde_json::from_slice(&bytes)?)
}

async fn write<T: Serialize>(socket: &mut (impl AsyncWrite + Unpin), value: &T) -> Result<()> {
    let bytes = serde_json::to_vec(value)?;
    ensure!(bytes.len() <= MAX_FRAME, "management response too large");
    socket.write_u32(bytes.len() as u32).await?;
    socket.write_all(&bytes).await?;
    socket.flush().await?;
    Ok(())
}

pub async fn receive(socket: &mut Stream) -> Result<Command> {
    let req: Request = tokio::time::timeout(DEADLINE, read(socket)).await??;
    ensure!(
        req.version == VERSION,
        "unsupported management protocol {}",
        req.version
    );
    Ok(req.command)
}

pub async fn respond<T: Serialize>(socket: &mut Stream, result: Result<T>) -> Result<()> {
    let reply = Reply {
        version: VERSION,
        result: result.map_err(|e| format!("{e:#}")),
    };
    tokio::time::timeout(DEADLINE, write(socket, &reply)).await?
}

async fn begin<T: DeserializeOwned>(
    path: &Path,
    command: Command,
    deadline: Duration,
) -> Result<(Stream, T)> {
    tokio::time::timeout(deadline, async {
        let mut stream = Stream::connect(socket_name(path)?)
            .await
            .with_context(|| format!("connect to gateway at {}", path.display()))?;
        write(
            &mut stream,
            &Request {
                version: VERSION,
                command,
            },
        )
        .await?;
        let reply: Reply<T> = read(&mut stream).await?;
        ensure!(
            reply.version == VERSION,
            "unsupported management response version"
        );
        Ok((stream, reply.result.map_err(anyhow::Error::msg)?))
    })
    .await
    .context("gateway request timed out")?
}

/// The gateway sends a byte on subscription and each time shared peer connections change.
pub async fn watch_peer_connections(path: &Path) -> Result<Stream> {
    let (stream, ()) = begin(path, Command::WatchPeerConnections, DEADLINE).await?;
    Ok(stream)
}

pub async fn request(path: &Path, command: Command) -> Result<Status> {
    let deadline = if matches!(command, Command::CheckPeer { .. } | Command::Unpair { .. } | Command::ListForwards | Command::ManagePeerForward { .. }) {
        Duration::from_secs(30)
    } else {
        DEADLINE
    };
    let (_stream, mut status) = begin::<Status>(path, command, deadline).await?;
    status.ipc_path = Some(path.to_owned());
    // Older gateways did not report their image. Ask the OS about this pipe's server.
    #[cfg(windows)]
    if status.executable.is_none() {
        status.executable = _stream
            .peer_creds()
            .ok()
            .and_then(|p| process_image(p.pid()?));
    }
    Ok(status)
}

#[cfg(windows)]
fn process_image(pid: u32) -> Option<PathBuf> {
    use std::{
        ffi::OsString,
        os::windows::{
            ffi::OsStringExt,
            io::{AsRawHandle, FromRawHandle, OwnedHandle},
        },
    };
    use windows_sys::Win32::System::Threading::{
        OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
    };
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if handle.is_null() {
        return None;
    }
    let process = unsafe { OwnedHandle::from_raw_handle(handle) };
    let mut path = vec![0u16; 32768];
    let mut len = path.len() as u32;
    let ok = unsafe {
        QueryFullProcessImageNameW(process.as_raw_handle(), 0, path.as_mut_ptr(), &mut len)
    };
    (ok != 0).then(|| OsString::from_wide(&path[..len as usize]).into())
}

/// The gateway acknowledges after closing its listeners and transport.
pub async fn shutdown(path: &Path) -> Result<()> {
    begin::<()>(path, Command::Shutdown, Duration::from_secs(30)).await?;
    Ok(())
}

pub fn is_offline(error: &anyhow::Error) -> bool {
    error.downcast_ref::<std::io::Error>().is_some_and(|e| {
        matches!(
            e.kind(),
            std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused
        )
    })
}

pub async fn connect(path: &Path, peer: String, target: String) -> Result<Stream> {
    // Allow the network dial and remote OPEN handshake to finish.
    let (socket, ()) = begin(
        path,
        Command::Connect { peer, target },
        Duration::from_secs(50),
    )
    .await?;
    Ok(socket)
}

/// Open a binary file stream and run the same transfer for a CLI or desktop caller.
pub async fn copy_files(
    path: &Path,
    peer: &str,
    copy: files::Copy<'_>,
    options: files::Options,
    require_directory: bool,
    report: &mut files::Report<'_>,
) -> Result<()> {
    let (socket, ()) = begin(
        path,
        Command::OpenFiles { peer: peer.into() },
        Duration::from_secs(50),
    )
    .await?;
    files::copy(socket, copy, options, require_directory, report).await
}

/// Open a native interactive terminal. After the reply, IPC carries terminal frames directly.
pub async fn open_terminal(path: &Path, peer: String, size: terminal::Size) -> Result<Stream> {
    open_pty(path, peer, size, None).await
}

pub async fn open_pty(
    path: &Path,
    peer: String,
    size: terminal::Size,
    invocation: Option<terminal::exec::Invocation>,
) -> Result<Stream> {
    let size = size.validate()?;
    let command = match invocation {
        Some(invocation) => Command::Exec {
            peer,
            size,
            invocation,
        },
        None => Command::OpenTerminal { peer, size },
    };
    let (socket, ()) = begin(path, command, Duration::from_secs(50)).await?;
    Ok(socket)
}

pub async fn open_pipe(
    path: &Path,
    peer: String,
    invocation: terminal::exec::Invocation,
) -> Result<Stream> {
    let (socket, ()) = begin(
        path,
        Command::ExecPipe { peer, invocation },
        Duration::from_secs(50),
    )
    .await?;
    Ok(socket)
}

/// Bridge an upgraded IPC connection. A zero-length frame closes one direction,
/// including on Windows named pipes which have no native half-close operation.
pub async fn bridge(
    socket: Stream,
    mut source: impl AsyncRead + Unpin,
    destination: impl AsyncWrite + Unpin,
) -> Result<()> {
    let (mut incoming, mut outgoing) = tokio::io::split(socket);
    let send = async {
        let mut buf = vec![0; STREAM_CHUNK];
        loop {
            let size = source.read(&mut buf).await?;
            outgoing.write_u32(size as u32).await?;
            outgoing.write_all(&buf[..size]).await?;
            outgoing.flush().await?;
            if size == 0 {
                return Ok::<_, anyhow::Error>(());
            }
        }
    };
    tokio::pin!(send);
    {
        let receive = receive_data(&mut incoming, destination);
        tokio::pin!(receive);
        tokio::select! {
            result = &mut send => { result?; return receive.await; },
            result = &mut receive => result?,
        }
    }
    // Detect a disconnected local client even after it has sent its FIN.
    tokio::select! {
        biased;
        result = send => result,
        byte = incoming.read_u8() => anyhow::bail!("IPC closed or sent data after FIN: {byte:?}"),
    }
}

const STREAM_CHUNK: usize = 32 * 1024;

async fn receive_data(
    incoming: &mut (impl AsyncRead + Unpin),
    mut destination: impl AsyncWrite + Unpin,
) -> Result<()> {
    let mut buf = vec![0; STREAM_CHUNK];
    loop {
        let size = incoming.read_u32().await? as usize;
        ensure!(size <= STREAM_CHUNK, "invalid stream frame length");
        if size == 0 {
            destination.shutdown().await?;
            return Ok(());
        }
        incoming.read_exact(&mut buf[..size]).await?;
        destination.write_all(&buf[..size]).await?;
        destination.flush().await?;
    }
}
