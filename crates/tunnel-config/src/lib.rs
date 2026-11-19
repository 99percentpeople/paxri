use std::collections::BTreeMap;
use std::net::SocketAddr;

use serde::{Deserialize, Serialize};
use url::{Host, Url};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub version: u32,
    pub device_name: Option<String>,
    #[serde(default)]
    pub native: Native,
    #[serde(default)]
    pub peers: BTreeMap<String, Peer>,
    #[serde(default)]
    pub services: BTreeMap<String, Service>,
    #[serde(default)]
    pub forwards: BTreeMap<String, Forward>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Native {
    pub relay_url: Option<Url>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Peer {
    pub endpoint_id: String,
    pub ticket: Option<String>,
    pub ssh: Option<SshProfile>,
    #[serde(default)]
    pub ports: PortAccess,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct SshProfile {
    pub user: Option<String>,
    pub service: String,
    pub identity_file: Option<String>,
}

impl Default for SshProfile {
    fn default() -> Self {
        Self {
            user: None,
            service: "ssh".into(),
            identity_file: None,
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct PortAccess {
    pub connect: Vec<String>,
    pub listen: Vec<SocketAddr>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Service {
    pub target: ServiceTarget,
    pub allow_peers: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged, deny_unknown_fields)]
pub enum ServiceTarget {
    Tcp(String),
    OpenSsh { sshd_config: std::path::PathBuf },
}

impl std::fmt::Display for ServiceTarget {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Tcp(address) => f.write_str(address),
            Self::OpenSsh { sshd_config } => write!(f, "OpenSSH ({})", sshd_config.display()),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    #[default]
    Local,
    Remote,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Destination {
    Service(String),
    Target(String),
}

impl Destination {
    pub fn address(&self) -> &str {
        match self {
            Self::Service(s) | Self::Target(s) => s,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Forward {
    pub listen: SocketAddr,
    pub peer: String,
    #[serde(default)]
    pub direction: Direction,
    #[serde(flatten)]
    pub destination: Destination,
}

/// A bare port in a port rule means IPv4 loopback. Explicit IPs keep their scope.
pub fn listen_address(input: &str) -> Result<SocketAddr, String> {
    if let Ok(port) = input.parse::<u16>() {
        return Ok(([127, 0, 0, 1], port).into());
    }
    input
        .parse()
        .map_err(|_| format!("Invalid listen address {input:?}. Enter an IP and TCP port."))
}

pub fn tcp_target(input: &str) -> Result<String, String> {
    if let Ok(port @ 1..) = input.parse::<u16>() {
        return Ok(format!("127.0.0.1:{port}"));
    }
    if valid_target(input) {
        return Ok(input
            .parse::<SocketAddr>()
            .map_or_else(|_| input.to_owned(), |a| a.to_string()));
    }
    Err(format!(
        "Invalid target {input:?}. Enter a host and TCP port."
    ))
}

pub fn parse(input: &str) -> Result<Config, String> {
    let mut config: Config = toml::from_str(input).map_err(|e| format!("{e}"))?;
    if config.version != 1 {
        return Err(format!(
            "unsupported configuration version {}",
            config.version
        ));
    }
    if let Some(name) = &config.device_name
        && (name.trim().is_empty() || name.chars().any(char::is_control))
    {
        return Err("device_name requires a non-empty, single-line name".into());
    }
    if let Some(relay) = &config.native.relay_url
        && (!matches!(relay.scheme(), "https" | "http") || relay.host().is_none())
    {
        return Err("native.relay_url requires an HTTP(S) URL with a host".into());
    }
    for (name, peer) in &mut config.peers {
        if peer.endpoint_id.len() != 64 || !peer.endpoint_id.bytes().all(|c| c.is_ascii_hexdigit())
        {
            return Err(format!("peers.{name}.endpoint_id requires 64 hex digits"));
        }
        if let Some(profile) = &peer.ssh {
            for text in std::iter::once(&profile.service)
                .chain(&profile.user)
                .chain(&profile.identity_file)
            {
                if text.trim().is_empty() || text.chars().any(char::is_control) {
                    return Err(format!(
                        "peers.{name}.ssh requires non-empty single-line values"
                    ));
                }
            }
        }
        for address in &mut peer.ports.connect {
            *address = tcp_target(address)?;
        }
    }
    for (name, service) in &config.services {
        match &service.target {
            ServiceTarget::Tcp(address) if !valid_target(address) => {
                return Err(format!(
                    "services.{name}.target requires a host and TCP port"
                ));
            }
            ServiceTarget::OpenSsh { .. } if !cfg!(target_os = "linux") => {
                return Err(format!(
                    "services.{name}: the sshd_config backend requires Linux"
                ));
            }
            ServiceTarget::OpenSsh { sshd_config } if !sshd_config.is_absolute() => {
                return Err(format!(
                    "services.{name}.target.sshd_config requires an absolute path"
                ));
            }
            _ => {}
        }
        for peer in &service.allow_peers {
            if !config.peers.contains_key(peer) {
                return Err(format!(
                    "services.{name}.allow_peers refers to unknown peer {peer}"
                ));
            }
        }
    }
    for (name, forward) in &mut config.forwards {
        if !config.peers.contains_key(&forward.peer) {
            return Err(format!(
                "forwards.{name} refers to unknown peer {}",
                forward.peer
            ));
        }
        match &mut forward.destination {
            Destination::Service(service) => {
                if service.trim().is_empty() || forward.direction == Direction::Remote {
                    return Err(format!(
                        "forwards.{name}: service requires a local listener and a remote service name"
                    ));
                }
            }
            Destination::Target(target) => *target = tcp_target(target)?,
        }
    }
    Ok(config)
}

fn valid_target(target: &str) -> bool {
    let Some((host, port)) = target.rsplit_once(':') else {
        return false;
    };
    Host::parse(host).is_ok() && port.parse::<u16>().is_ok_and(|port| port > 0)
}
