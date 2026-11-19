use anyhow::{Context, Result, ensure};
use clap::{Args, Subcommand};
use std::{net::SocketAddr, path::Path};
use tunnel_control::{
    Command, Destination, Direction, Forward, PortAccess, listen_address, request, tcp_target,
};

#[derive(Subcommand)]
pub enum ForwardAction {
    /// Show forwarding rules, their endpoints and current listener state
    List,
    Add(Rule),
    Edit(Rule),
    Remove {
        name: String,
    },
    Pause {
        name: String,
    },
    Resume {
        name: String,
    },
}

#[derive(Args)]
pub struct Rule {
    name: String,
    #[arg(long)]
    peer: String,
    /// Listening IP and port, or a loopback port
    #[arg(long, value_parser = listen_address)]
    listen: SocketAddr,
    /// Listen on the peer and forward connections to a TCP target on this device
    #[arg(long, conflicts_with = "service")]
    remote: bool,
    /// Destination host:port, or a loopback port
    #[arg(long, required_unless_present = "service", conflicts_with = "service", value_parser = tcp_target)]
    target: Option<String>,
    /// Published service on the peer, for a local listener
    #[arg(long)]
    service: Option<String>,
}

impl ForwardAction {
    pub async fn run(self, ipc: &Path) -> Result<()> {
        let edit = matches!(self, Self::Edit(_));
        let enabled = matches!(self, Self::Resume { .. });
        let (name, command) = match self {
            Self::List => {
                let status = request(ipc, Command::Status).await?;
                if status.forwards.is_empty() {
                    println!("No forwarding rules.");
                }
                for (name, f) in status.forwards {
                    println!("{name}: {} ({})", f.route(), f.state());
                }
                return Ok(());
            }
            Self::Add(args) | Self::Edit(args) => {
                let destination = match (args.target, args.service) {
                    (Some(s), _) => Destination::Target(s),
                    (_, Some(s)) => Destination::Service(s),
                    _ => anyhow::bail!("Enter a target or service"),
                };
                let rule = Forward {
                    listen: args.listen,
                    peer: args.peer,
                    destination,
                    direction: if args.remote {
                        Direction::Remote
                    } else {
                        Direction::Local
                    },
                };
                (
                    args.name.clone(),
                    Command::SaveForward {
                        name: args.name,
                        expected_peer: None,
                        edit,
                        rule,
                    },
                )
            }
            Self::Remove { name } => (
                name.clone(),
                Command::RemoveForward {
                    name,
                    expected_peer: None,
                },
            ),
            Self::Resume { name } | Self::Pause { name } => (
                name.clone(),
                Command::SetForward {
                    name,
                    enabled,
                    expected_peer: None,
                },
            ),
        };
        let status = request(ipc, command).await?;
        if let Some(f) = status.forwards.get(&name) {
            println!("{name}: {} ({})", f.route(), f.state());
        } else {
            println!("Removed {name}");
        }
        Ok(())
    }
}

#[derive(Subcommand)]
pub enum PortAction {
    Show {
        peer: String,
    },
    /// Replace both explicit allowlists. Repeat an option for several addresses.
    Set {
        peer: String,
        #[arg(long, value_delimiter = ',', value_parser = tcp_target)]
        connect: Vec<String>,
        #[arg(long, value_delimiter = ',', value_parser = listen_address)]
        listen: Vec<SocketAddr>,
    },
}

impl PortAction {
    pub async fn run(self, ipc: &Path) -> Result<()> {
        let (peer, command) = match self {
            Self::Set {
                peer,
                connect,
                listen,
            } => (
                peer.clone(),
                Command::SetPortAccess {
                    peer,
                    ports: PortAccess { connect, listen },
                },
            ),
            Self::Show { peer } => (peer, Command::Status),
        };
        let mut status = request(ipc, command).await?;
        ensure!(status.peers.contains_key(&peer), "Unknown peer {peer}");
        let ports = status.port_access.remove(&peer).unwrap_or_default();
        println!(
            "Allow connect: {}\nAllow listen: {}",
            ports.connect.join(", "),
            ports
                .listen
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        );
        Ok(())
    }
}

#[derive(Subcommand)]
pub enum PairAction {
    /// Print this device's current pairing code
    Code,
    /// Replace this device's code and dismiss requests made with the old code
    Refresh,
    /// Request pairing with another device
    Join { code: String },
    /// Show pending requests and the last pairing result
    Status,
    /// Withdraw this device's outgoing request
    Cancel,
    /// Approve the pending request from this device ID
    Accept { device_id: String },
    /// Decline the pending request from this device ID
    Decline { device_id: String },
}

impl PairAction {
    pub async fn run(self, ipc: &Path) -> Result<()> {
        let show_code = matches!(self, Self::Code | Self::Refresh);
        let command = match self {
            Self::Code | Self::Status => Command::Status,
            Self::Refresh => Command::RefreshPairingCode,
            Self::Join { code } => Command::JoinPairing { code },
            Self::Cancel => Command::CancelPairing,
            Self::Accept { device_id } => Command::AnswerPairing {
                device_id,
                request_id: None,
                accept: true,
            },
            Self::Decline { device_id } => Command::AnswerPairing {
                device_id,
                request_id: None,
                accept: false,
            },
        };
        let pairing = request(ipc, command).await?.pairing;
        if show_code {
            println!(
                "{}",
                pairing.code.context(
                    pairing
                        .message
                        .unwrap_or_else(|| "Gateway did not return a pairing code".into())
                )?
            );
            return Ok(());
        }
        if pairing.incoming.is_empty() && pairing.outgoing.is_none() && pairing.message.is_none() {
            println!("No pairing in progress.");
        }
        for peer in pairing.incoming {
            println!(
                "Approval requested by {} ({} · {})",
                peer.name,
                peer.platform.label(),
                peer.device_id
            );
        }
        if let Some(id) = pairing.outgoing {
            println!("Waiting for approval from {id}");
        }
        if let Some(message) = pairing.message {
            println!("{message}");
        }
        Ok(())
    }
}

#[derive(Subcommand)]
pub enum TerminalAccess {
    Show { peer: String },
    Allow { peer: String },
    Deny { peer: String },
}

impl TerminalAccess {
    pub async fn run(self, ipc: &Path) -> Result<()> {
        let (peer, allowed) = match self {
            Self::Show { peer } => (peer, None),
            Self::Allow { peer } => (peer, Some(true)),
            Self::Deny { peer } => (peer, Some(false)),
        };
        let cmd = allowed.map_or(Command::Status, |allowed| Command::SetTerminalAccess {
            peer: peer.clone(),
            allowed,
        });
        let status = request(ipc, cmd).await?;
        ensure!(status.peers.contains_key(&peer), "Unknown peer {peer}");
        let access = if status.terminal_access.contains(&peer) {
            "allowed"
        } else {
            "denied"
        };
        println!("Native terminal access for {peer} on this device: {access}");
        Ok(())
    }
}
