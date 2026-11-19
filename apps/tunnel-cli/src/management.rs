use anyhow::{Result, ensure};
use clap::{Args, Subcommand};
use std::{net::SocketAddr, path::Path};
use tunnel_control::{
    Command, Destination, Direction, Forward, PortAccess, SshProfile, listen_address, request,
    tcp_target,
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
                        edit,
                        rule,
                    },
                )
            }
            Self::Remove { name } => (name.clone(), Command::RemoveForward { name }),
            Self::Resume { name } | Self::Pause { name } => {
                (name.clone(), Command::SetForward { name, enabled })
            }
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
pub enum SshAction {
    Show {
        peer: String,
    },
    /// Replace the saved settings. Omitted fields use their normal defaults.
    Set {
        peer: String,
        #[arg(short = 'l', long)]
        user: Option<String>,
        #[arg(long, default_value = "ssh")]
        service: String,
        #[arg(short = 'i', long)]
        identity: Option<String>,
    },
    Reset {
        peer: String,
    },
}

impl SshAction {
    pub async fn run(self, ipc: &Path) -> Result<()> {
        let (peer, command) = match self {
            Self::Set {
                peer,
                user,
                service,
                identity,
            } => (
                peer.clone(),
                Command::SetSshProfile {
                    peer,
                    profile: Some(SshProfile {
                        user,
                        service,
                        identity_file: identity,
                    }),
                },
            ),
            Self::Reset { peer } => (
                peer.clone(),
                Command::SetSshProfile {
                    peer,
                    profile: None,
                },
            ),
            Self::Show { peer } => (peer, Command::Status),
        };
        let mut status = request(ipc, command).await?;
        ensure!(status.peers.contains_key(&peer), "Unknown peer {peer}");
        let profile = status.ssh_profiles.remove(&peer).unwrap_or_default();
        println!(
            "Login: {}\nService: {}\nIdentity: {}",
            profile.user.as_deref().unwrap_or("OpenSSH default"),
            profile.service,
            profile
                .identity_file
                .as_deref()
                .unwrap_or("OpenSSH default")
        );
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
