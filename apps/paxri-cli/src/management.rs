use crate::output;
use anyhow::{Context, Result};
use clap::{Args, Subcommand};
use paxri_control::{
    Command, Direction, Forward, PeerForward, listen_address, request, tcp_target,
};
use std::{
    io::{self, IsTerminal, Write},
    net::SocketAddr,
    path::Path,
};

#[derive(Subcommand)]
pub enum ForwardAction {
    /// Show forwarding rules, their endpoints and current listener state
    List {
        #[arg(long)]
        json: bool,
    },
    Add(Rule),
    Edit(EditRule),
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
pub struct EditRule {
    #[command(flatten)]
    rule: Rule,
    /// Rename the rule while updating its settings
    #[arg(long)]
    rename: Option<String>,
}

#[derive(Args)]
pub struct Rule {
    name: String,
    /// Paired device name, full ID or unique ID prefix
    #[arg(long)]
    peer: Option<String>,
    /// Listening IP and port, or a loopback port
    #[arg(long, value_parser = listen_address)]
    listen: SocketAddr,
    /// Listen on the peer and forward connections to a TCP target on this device
    #[arg(long)]
    remote: bool,
    /// Destination host:port, or a loopback port
    #[arg(long, value_parser = tcp_target)]
    target: String,
}

impl ForwardAction {
    pub async fn run(self, ipc: &Path, owner: Option<String>) -> Result<()> {
        let edit = matches!(self, Self::Edit(_));
        let enabled = matches!(self, Self::Resume { .. });
        if let Self::List { json } = self {
            anyhow::ensure!(owner.is_none(), "List shows rules from all online owners");
            let status = request(ipc, Command::ListForwards).await?;
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "local": status.forwards, "peers": status.peer_forwards
                    }))?
                );
            } else {
                print!("{}", output::forwards(&status));
            }
            return Ok(());
        }
        if let Some(owner) = owner {
            let action = match self {
                Self::Edit(EditRule { rule: args, rename }) => {
                    anyhow::ensure!(
                        args.peer.is_none(),
                        "A peer-owned rule already belongs to this pairing. Omit --peer."
                    );
                    let new_name = rename.filter(|next| next != &args.name);
                    PeerForward::Edit {
                        name: args.name,
                        new_name,
                        target: args.target,
                        listen: args.listen,
                        direction: if args.remote {
                            Direction::Remote
                        } else {
                            Direction::Local
                        },
                    }
                }
                Self::Remove { name } => PeerForward::Remove { name },
                Self::Pause { name } | Self::Resume { name } => {
                    PeerForward::SetEnabled { name, enabled }
                }
                Self::Add(_) => anyhow::bail!("Create a rule with --peer and omit --owner"),
                Self::List { .. } => unreachable!(),
            };
            request(
                ipc,
                Command::ManagePeerForward {
                    owner: owner.clone(),
                    action,
                },
            )
            .await?;
            println!("Updated forwarding rules on {owner}.");
            return Ok(());
        }
        let (name, command) = match self {
            Self::List { .. } => unreachable!(),
            Self::Add(args) => {
                let rule = Forward {
                    listen: args.listen,
                    peer: args.peer.context("Specify the paired device with --peer")?,
                    target: args.target,
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
                        new_name: None,
                        expected_peer: None,
                        edit,
                        rule,
                    },
                )
            }
            Self::Edit(EditRule { rule: args, rename }) => {
                let rule = Forward {
                    listen: args.listen,
                    peer: args.peer.context("Specify the paired device with --peer")?,
                    target: args.target,
                    direction: if args.remote {
                        Direction::Remote
                    } else {
                        Direction::Local
                    },
                };
                let new_name = rename.filter(|next| next != &args.name);
                let display_name = new_name.clone().unwrap_or_else(|| args.name.clone());
                (
                    display_name,
                    Command::SaveForward {
                        name: args.name,
                        new_name,
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
pub enum PairAction {
    /// Print this device's current pairing code
    Code,
    /// Display the current invitation as a QR code for a phone to scan
    Qr {
        /// Write an SVG image to stdout (e.g. --svg > pairing.svg)
        #[arg(long)]
        svg: bool,
    },
    /// Replace this device's code and dismiss requests made with the old code
    Refresh,
    /// Request pairing with another device
    Join { code: String },
    /// Show pending requests and the last pairing result
    Status,
    /// Unpair a device by name, full ID or unique ID prefix
    ///
    /// Close its connections and remove its forwarding rules
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
        let qr_svg = match self {
            Self::Qr { svg } => Some(svg),
            _ => None,
        };
        let show_code = qr_svg.is_some() || matches!(self, Self::Code | Self::Refresh);
        let command = match self {
            Self::Code | Self::Qr { .. } | Self::Status => Command::Status,
            Self::Refresh => Command::RefreshPairingCode,
            Self::Join { code } => Command::JoinPairing { code },
            Self::Cancel => Command::CancelPairing,
            Self::Remove { peer } => Command::Unpair {
                peer,
                expected_id: None,
            },
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
            println!(
                "{}",
                pairing.message.as_deref().unwrap_or("Device unpaired.")
            );
            return Ok(());
        }
        if show_code {
            let code = pairing.code.context(
                pairing
                    .message
                    .unwrap_or_else(|| "Gateway did not return a pairing code".into()),
            )?;
            if let Some(svg) = qr_svg {
                return print_pairing_qr(&code, svg);
            }
            println!("{code}");
            return Ok(());
        }
        print!("{}", output::pairing(&pairing));
        Ok(())
    }
}

fn print_pairing_qr(code: &str, svg: bool) -> Result<()> {
    let matrix = qrcode::QrCode::new(code)
        .context("Invitation is too large for a QR code. Use paxri pair code.")?;
    let stdout = io::stdout();
    if svg {
        let image = matrix
            .render::<qrcode::render::svg::Color>()
            .min_dimensions(384, 384)
            .build();
        writeln!(stdout.lock(), "{image}")?;
        return Ok(());
    }
    let rows = matrix.render::<qrcode::render::unicode::Dense1x2>().build();
    if !stdout.is_terminal() {
        writeln!(stdout.lock(), "{rows}")?;
        return Ok(());
    }
    let width = matrix.width() + 8; // Four quiet modules on each side.
    let height = rows.lines().count() + 1; // Leave room for the shell prompt.
    if let Some(size) = crate::shell::terminal_size()
        && size.cols != 0
        && size.rows != 0
    {
        anyhow::ensure!(
            usize::from(size.cols) >= width && usize::from(size.rows) >= height,
            "QR code needs {width} columns and {height} rows. Enlarge the terminal or run paxri pair qr --svg > pairing.svg."
        );
    }
    let mut out = anstream::AutoStream::always(stdout.lock());
    for row in rows.lines() {
        writeln!(out, "\x1b[30;107m{row}\x1b[0m")?;
    }
    Ok(())
}
