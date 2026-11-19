use std::collections::BTreeMap;
use std::net::SocketAddr;

use serde::{Deserialize, Serialize};
use url::{Host, Url};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub version: u32,
    pub device_name: Option<String>,
    pub pairing_code: Option<String>,
    #[serde(default)]
    pub native: Native,
    #[serde(default)]
    pub peers: BTreeMap<String, Peer>,
    #[serde(default, deserialize_with = "tcp_services")]
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
    pub platform: Option<DevicePlatform>,
    // Accept profiles written by versions with the OpenSSH client.
    #[serde(rename = "ssh", default)]
    _legacy_ssh: Option<serde::de::IgnoredAny>,
    #[serde(default)]
    pub allow_terminal: bool,
    #[serde(default)]
    pub ports: PortAccess,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum DevicePlatform {
    Windows,
    Linux,
    MacOs,
    Web,
    #[default]
    #[serde(other)]
    Unknown,
}

impl DevicePlatform {
    pub fn current() -> Self {
        match std::env::consts::OS {
            "windows" => Self::Windows,
            "linux" => Self::Linux,
            "macos" => Self::MacOs,
            "unknown" | "emscripten" if cfg!(target_family = "wasm") => Self::Web,
            _ => Self::Unknown,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Windows => "Windows",
            Self::Linux => "Linux",
            Self::MacOs => "macOS",
            Self::Web => "Web",
            Self::Unknown => "Unknown platform",
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Windows => "windows",
            Self::Linux => "linux",
            Self::MacOs => "macos",
            Self::Web => "web",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct PortAccess {
    pub connect: Vec<String>,
    pub listen: Vec<SocketAddr>,
}

#[derive(Debug)]
pub struct Service {
    pub target: String,
    pub allow_peers: Vec<String>,
}

// Read older files in place. A retired per-connection sshd handler publishes no service.
fn tcp_services<'de, D: serde::Deserializer<'de>>(
    data: D,
) -> Result<BTreeMap<String, Service>, D::Error> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Entry {
        target: Target,
        allow_peers: Vec<String>,
    }
    #[derive(Deserialize)]
    #[serde(untagged, deny_unknown_fields)]
    enum Target {
        Tcp(String),
        Retired {
            #[serde(rename = "sshd_config")]
            _path: std::path::PathBuf,
        },
    }
    let entries = BTreeMap::<String, Entry>::deserialize(data)?;
    Ok(entries
        .into_iter()
        .filter_map(|(name, entry)| {
            let Target::Tcp(target) = entry.target else {
                return None;
            };
            Some((
                name,
                Service {
                    target,
                    allow_peers: entry.allow_peers,
                },
            ))
        })
        .collect())
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
        for address in &mut peer.ports.connect {
            *address = tcp_target(address)?;
        }
    }
    for (name, service) in &config.services {
        if !valid_target(&service.target) {
            return Err(format!(
                "services.{name}.target requires a host and TCP port"
            ));
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
