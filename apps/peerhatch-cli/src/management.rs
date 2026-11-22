use crate::output;
use anyhow::{Context, Result};
use clap::{Args, Subcommand};
use std::{net::SocketAddr, path::Path};
use peerhatch_control::{
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
    /// Paired device name, full ID or unique ID prefix
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
                print!("{}", output::forwards(&status.forwards));
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
    Show,
    /// Replace both gateway-wide allowlists for paired devices.
    Set {
        #[arg(long, value_delimiter = ',', value_parser = tcp_target)]
        connect: Vec<String>,
        #[arg(long, value_delimiter = ',', value_parser = listen_address)]
        listen: Vec<SocketAddr>,
    },
}

impl PortAction {
    pub async fn run(self, ipc: &Path) -> Result<()> {
        let command = match self {
            Self::Set { connect, listen } => Command::SetGatewayPortAccess {
                expected: None,
                ports: PortAccess { connect, listen },
            },
            Self::Show => Command::Status,
        };
        let status = request(ipc, command).await?;
        let ports = status
            .gateway_port_access
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Update the gateway to manage global port access."))?;
        print!("{}", output::port_access(ports));
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
    /// Unpair a device by name, full ID or unique ID prefix
    ///
    /// Close its connections and remove its forwarding rules and service grants
    /// on both gateways. An offline gateway receives the notice on a later retry.
    Remove { peer: String },
    /// Withdraw this device's outgoing request
    Cancel,
    /// Approve the pending request from this device ID
    Accept { device_id: String },
    /// Decline the pending request from this device ID
    Decline { device_id: String },
}

impl PairAction {
    pub async fn run(self, ipc: &Path) -> Result<()> {
        let unpair = matches!(self, Self::Remove { .. });
        let show_code = matches!(self, Self::Code | Self::Refresh);
        let command = match self {
            Self::Code | Self::Status => Command::Status,
            Self::Refresh => Command::RefreshPairingCode,
            Self::Join { code } => Command::JoinPairing { code },
            Self::Cancel => Command::CancelPairing,
            Self::Remove { peer } => Command::Unpair { peer, expected_id: None },
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
        if unpair {
            println!("{}", pairing.message.as_deref().unwrap_or("Device unpaired."));
            return Ok(());
        }
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
        print!("{}", output::pairing(&pairing));
        Ok(())
    }
}
