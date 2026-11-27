use std::collections::BTreeMap;
use std::net::SocketAddr;

use serde::{Deserialize, Serialize};
use url::{Host, Url};

pub mod file;
pub mod gui;
pub mod keybar;
pub mod limits;
pub mod shortcuts;
pub mod peer_id;
pub mod startup;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub version: u32,
    pub device_name: Option<String>,
    pub pairing_code: Option<String>,
    #[serde(default)]
    pub pending_unpairs: BTreeMap<String, PendingUnpair>,
    #[serde(default)]
    pub gui: gui::GuiSettings,
    #[serde(default)]
    pub gateway: startup::GatewaySettings,
    #[serde(default)]
    pub native: Native,
    #[serde(default, rename = "port_access")]
    _retired_port_access: Option<serde::de::IgnoredAny>,
    #[serde(default)]
    pub peers: BTreeMap<String, Peer>,
    #[serde(default, rename = "services")]
    _retired_services: Option<serde::de::IgnoredAny>,
    #[serde(default)]
    pub forwards: BTreeMap<String, Forward>,
}

impl Config {
    pub fn is_paired(&self, endpoint_id: &str) -> bool {
        self.peers
            .values()
            .any(|peer| peer.endpoint_id.eq_ignore_ascii_case(endpoint_id))
    }

    pub fn resolve_peer(&self, selector: &str) -> anyhow::Result<(&String, &Peer)> {
        peer_id::resolve(&self.peers, selector, |peer| &peer.endpoint_id)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Native {
    pub upnp: bool,
    pub relay_url: Option<Url>,
    /// Find a LAN router for native IPv4 UDP when the system default route uses a VPN.
    #[serde(default)]
    pub lan_port_mapping: bool,
}

impl Default for Native {
    fn default() -> Self {
        Self {
            upnp: true,
            relay_url: None,
            lan_port_mapping: false,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NativeChange {
    Upnp(bool),
    LanPortMapping(bool),
    /// None restores automatic public relay selection.
    RelayUrl(Option<String>),
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PendingUnpair {
    pub name: String,
    pub ticket: String,
    pub request_id: String,
    pub pairing_id: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Peer {
    pub endpoint_id: String,
    pub pairing_id: Option<String>,
    pub ticket: Option<String>,
    pub platform: Option<DevicePlatform>,
    /// Last name advertised by the paired endpoint, including while it is offline.
    pub device_name: Option<String>,
    /// Unix milliseconds of the last pairing or connection state transition observed locally.
    pub last_status_change_ms: Option<i64>,
    // Accept profiles written by versions with the OpenSSH client.
    #[serde(rename = "ssh", default)]
    _legacy_ssh: Option<serde::de::IgnoredAny>,
    // Stored by earlier gateways. The paired identity now grants terminal access.
    #[serde(rename = "allow_terminal", default)]
    _legacy_terminal: Option<serde::de::IgnoredAny>,
    // Read development profiles without promoting grants made to individual peers.
    #[serde(default, rename = "ports")]
    _legacy_ports: Option<serde::de::IgnoredAny>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum DevicePlatform {
    Android,
    Windows,
    Linux,
    MacOs,
    Web,
    #[default]
    #[serde(other)]
    Unknown,
}

/// Services this endpoint hosts for an authenticated peer.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default)]
pub struct DeviceCapabilities {
    pub shell: bool,
    pub managed_terminals: bool,
    /// The managed terminal stream can acknowledge a probe without writing to the PTY.
    pub terminal_heartbeat: bool,
    pub exec: bool,
    /// None for older peers that did not advertise rule management.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub forward_rules: Option<bool>,
    /// Accepts creation of gateway-owned rules scoped to the authenticated pairing.
    pub create_forward_rules: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct DeviceInfo {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_name: Option<String>,
    pub platform: DevicePlatform,
    pub capabilities: DeviceCapabilities,
}

impl DevicePlatform {
    /// Gateways predating capability exchange hosted terminals on these systems.
    pub fn supports_shell(self, capabilities: Option<DeviceCapabilities>) -> bool {
        capabilities.map_or(matches!(self, Self::Windows | Self::Linux | Self::MacOs), |c| c.shell)
    }

    pub fn supports_forward_rules(self, capabilities: Option<DeviceCapabilities>) -> bool {
        capabilities.and_then(|c| c.forward_rules)
            .unwrap_or(!matches!(self, Self::Android | Self::Web))
    }

    pub fn current() -> Self {
        match std::env::consts::OS {
            "android" => Self::Android,
            "windows" => Self::Windows,
            "linux" => Self::Linux,
            "macos" => Self::MacOs,
            "unknown" | "emscripten" if cfg!(target_family = "wasm") => Self::Web,
            _ => Self::Unknown,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Android => "Android",
            Self::Windows => "Windows",
            Self::Linux => "Linux",
            Self::MacOs => "macOS",
            Self::Web => "Web",
            Self::Unknown => "Unknown",
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Android => "android",
            Self::Windows => "windows",
            Self::Linux => "linux",
            Self::MacOs => "macos",
            Self::Web => "web",
            Self::Unknown => "unknown",
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

impl Direction {
    pub fn opposite(self) -> Self {
        match self {
            Self::Local => Self::Remote,
            Self::Remote => Self::Local,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Forward {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub listen: SocketAddr,
    pub peer: String,
    #[serde(default)]
    pub direction: Direction,
    pub target: String,
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
    config.gui.validate()?;
    config.gateway.limits.validate()?;
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
    for (name, peer) in &config.peers {
        if peer.endpoint_id.len() != 64 || !peer.endpoint_id.bytes().all(|c| c.is_ascii_hexdigit())
        {
            return Err(format!("peers.{name}.endpoint_id requires 64 hex digits"));
        }
    }
    #[derive(PartialEq, Eq, PartialOrd, Ord)]
    enum ListenHost {
        Local,
        Remote(String),
    }
    let mut seen_ports = std::collections::BTreeSet::new();
    for (id, forward) in &mut config.forwards {
        if let Some(name) = &forward.name
            && (name.trim().is_empty() || name.len() > 128 || name.chars().any(char::is_control))
        {
            return Err(format!(
                "forwards.{id}.name requires a non-empty, single-line name"
            ));
        }
        let Some(peer) = config.peers.get(&forward.peer) else {
            return Err(format!(
                "forwards.{id} refers to unknown peer {}",
                forward.peer
            ));
        };
        forward.target = tcp_target(&forward.target).map_err(|e| format!("forwards.{id}: {e}"))?;
        let port = forward.listen.port();
        if port != 0 {
            let host = match forward.direction {
                Direction::Local => ListenHost::Local,
                Direction::Remote => ListenHost::Remote(peer.endpoint_id.to_ascii_lowercase()),
            };
            if !seen_ports.insert((host, port)) {
                return Err(match forward.direction {
                    Direction::Local => format!(
                        "forwards.{id}: listen port {port} is already used on this device"
                    ),
                    Direction::Remote => format!(
                        "forwards.{id}: listen port {port} is already forwarded on {}",
                        forward.peer
                    ),
                });
            }
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
