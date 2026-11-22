use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use clap::{Args, Subcommand};
use peerhatch_config::Config;
use peerhatch_native::{self as native, EndpointTicket};

mod copy;
pub mod gateway;
mod management;
mod output;
mod shell;
mod stdio;
use management::{ForwardAction, PairAction};

#[derive(Args)]
pub struct GatewayOptions {
    /// Gateway socket path on Unix or local pipe name on Windows
    #[arg(long, global = true)]
    pub ipc: Option<PathBuf>,
}

#[derive(Subcommand)]
pub enum Command {
    #[cfg(windows)]
    #[command(hide = true)]
    BootGateway {
        #[arg(long)]
        client: PathBuf,
        #[arg(long)]
        node_file: PathBuf,
    },
    /// Manage the local background gateway and its startup policy
    Gateway {
        /// Node TOML (defaults to the current user's node)
        #[arg(
            id = "gateway_config",
            long = "config",
            global = true,
            value_name = "FILE"
        )]
        config: Option<PathBuf>,
        #[command(subcommand)]
        action: gateway::Action,
    },
    /// Show the running gateway, peers and bound ports
    Status {
        /// Paired device name, full ID or unique ID prefix (at least 4 hex digits)
        peer: Option<String>,
        /// Contact the named device before reporting its connection
        #[arg(long, requires = "peer")]
        check: bool,
        /// Print the full status as JSON, including complete device IDs
        #[arg(long)]
        json: bool,
    },
    /// Pair devices by code and approval for terminal, file and TCP access
    Pair {
        #[command(subcommand)]
        action: PairAction,
    },
    /// Export the running gateway connection ticket
    Ticket,
    /// Copy files between this computer and a paired device
    Cp(copy::CopyArgs),
    /// Open a TCP address on a paired device using stdin/stdout
    Connect {
        /// Paired device name, full ID or unique ID prefix
        peer: String,
        #[arg(value_parser = peerhatch_control::tcp_target)]
        target: String,
    },
    /// Open the paired device's native interactive shell
    Shell {
        /// Paired device name, full ID or unique ID prefix
        peer: String,
    },
    /// Run a command on a paired device and return its exit status
    #[command(
        after_help = "Examples\n  peerhatch exec home top\n  peerhatch exec home -- vim notes.txt\n  peerhatch exec home -c 'uname -a'\n  peerhatch exec --pipe home -- cat image.png > copy.png"
    )]
    Exec {
        /// Use byte-preserving stdin, stdout and stderr pipes without a terminal
        #[arg(long)]
        pipe: bool,
        /// Paired device name, full ID or unique ID prefix
        peer: String,
        /// Run one script in the remote device's default shell
        #[arg(
            short = 'c',
            long = "command",
            value_name = "SCRIPT",
            conflicts_with = "argv"
        )]
        script: Option<String>,
        /// Remote executable followed by its literal arguments
        #[arg(trailing_var_arg = true, num_args = 1.., required_unless_present = "script", value_names = ["PROGRAM", "ARG"])]
        argv: Vec<String>,
    },
    /// Manage local and remote TCP listeners
    #[command(
        after_help = "Examples\n  peerhatch forward add web --peer home --listen 8080 --target 3000\n  peerhatch forward add preview --remote --peer home --listen 9000 --target 5000"
    )]
    Forward {
        /// Manage an existing rule stored on this paired device
        #[arg(long, global = true)]
        owner: Option<String>,
        #[command(subcommand)]
        action: ForwardAction,
    },
    /// Inspect a local configuration file
    Config {
        #[command(subcommand)]
        command: ConfigCommand,
    },
    /// Print the device public key, creating the private key if needed
    Identity { key: PathBuf },
    /// Test an authenticated native connection
    Probe {
        #[command(subcommand)]
        command: ProbeCommand,
    },
}

#[derive(Subcommand)]
pub enum ConfigCommand {
    /// Validate a TOML file
    Check { file: PathBuf },
}

#[derive(Args)]
pub struct Node {
    /// Path to this device's private identity file
    #[arg(long)]
    key: PathBuf,
    /// Local TOML configuration
    #[arg(long)]
    config: PathBuf,
    /// Configured peer name, full ID or unique ID prefix
    #[arg(long)]
    peer: String,
    /// Use direct IP connectivity only
    #[arg(long)]
    no_relay: bool,
    /// Disable direct paths to measure the actual relay fallback
    #[arg(long, conflicts_with = "no_relay")]
    relay_only: bool,
}

#[derive(Subcommand)]
pub enum ProbeCommand {
    /// Print a connection ticket and answer probes until Ctrl-C
    Listen(Node),
    /// Send numbered echoes on one connection and sample the selected path
    Connect {
        #[command(flatten)]
        node: Node,
        /// Peer ticket. Omit when both devices use the same configured native.relay_url
        #[arg(long, required_if_eq("no_relay", "true"))]
        ticket: Option<EndpointTicket>,
        #[arg(long, default_value_t = 20, value_parser = clap::value_parser!(u16).range(1..=300))]
        count: u16,
    },
}

fn load_config(path: &Path) -> Result<Config> {
    let data = fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    peerhatch_config::parse(&data)
        .map_err(anyhow::Error::msg)
        .with_context(|| format!("parse {}", path.display()))
}

impl Command {
    /// Run a command in the calling terminal, forwarding a remote program's exit status.
    pub fn run(self, options: GatewayOptions) -> Result<()> {
        let uses_stdio = matches!(
            self,
            Self::Shell { .. } | Self::Exec { .. } | Self::Connect { .. }
        );
        let runtime = tokio::runtime::Runtime::new()?;
        let ipc = options.ipc;
        let result = runtime.block_on(dispatch(self, ipc));
        if uses_stdio {
            // A canceled Tokio stdin read may still be blocking on the terminal.
            runtime.shutdown_background();
        }
        let code = result?;
        if code != 0 {
            std::process::exit(code as i32);
        }
        Ok(())
    }
}

async fn dispatch(command: Command, selected: Option<PathBuf>) -> Result<u32> {
    if let Command::Gateway { action, config } = command {
        gateway::cli(action, selected.as_deref(), config).await?;
        return Ok(0);
    }
    #[cfg(windows)]
    if let Command::BootGateway { client, node_file } = command {
        return gateway::boot::start(&client, &node_file, selected.as_deref());
    }
    let discover = selected.is_none()
        && std::env::var_os("PEERHATCH_IPC").is_none()
        && !matches!(
            command,
            Command::Config { .. } | Command::Identity { .. } | Command::Probe { .. }
        );
    let mut active = if discover {
        gateway::discover(None, None).await?
    } else {
        None
    };
    let ipc = selected
        .or_else(|| active.as_ref()?.ipc_path.clone())
        .unwrap_or_else(peerhatch_control::default_address);
    match command {
        #[cfg(windows)]
        Command::BootGateway { .. } => unreachable!(),
        Command::Gateway { .. } => unreachable!(),
        Command::Cp(args) => return args.run(&ipc).await,
        Command::Shell { peer } => return shell::run(&ipc, peer, None).await,
        Command::Exec { peer, script, argv, pipe } => {
            use peerhatch_control::terminal::exec::Invocation;
            let command = match script {
                Some(script) => Invocation::Shell { script },
                None => {
                    let mut args = argv.into_iter();
                    Invocation::Program {
                        program: args.next().context("missing remote program")?,
                        args: args.collect(),
                    }
                }
            };
            if pipe {
                return shell::pipe(&ipc, peer, command).await;
            }
            return shell::run(&ipc, peer, Some(command)).await;
        }
        Command::Connect { peer, target } => {
            connect_stdio(&ipc, peer, target).await?;
        }
        Command::Status { peer, check, json } => {
            let mut status = match active.take() {
                Some(status) => status,
                None => {
                    peerhatch_control::request(&ipc, peerhatch_control::Command::Status).await?
                }
            };
            if let Some(selector) = peer {
                let (name, _) = status.resolve_peer(&selector)?;
                let peer = name.to_owned();
                if check {
                    status = peerhatch_control::request(
                        &ipc,
                        peerhatch_control::Command::CheckPeer { peer: peer.clone() },
                    )
                    .await?;
                }
                let id = status
                    .peers
                    .get(&peer)
                    .with_context(|| format!("Unknown paired device {peer}"))?;
                if json {
                    let paths = status.connections_for(&peer);
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&serde_json::json!({
                            "peer": peer, "device_id": id, "platform": status.peer_platform(&peer), "connections": paths,
                        }))?
                    );
                } else {
                    print!("{}", output::peer(&status, &peer, id));
                }
            } else if json {
                println!("{}", serde_json::to_string_pretty(&status)?);
            } else {
                print!("{}", output::status(&status));
            }
        }
        Command::Pair { action } => action.run(&ipc).await?,
        Command::Ticket => println!(
            "{}",
            peerhatch_control::request(&ipc, peerhatch_control::Command::Status)
                .await?
                .ticket
        ),
        Command::Forward { action, owner } => action.run(&ipc, owner).await?,
        Command::Config {
            command: ConfigCommand::Check { file },
        } => {
            let config = load_config(&file)?;
            println!(
                "{}: OK ({} peers, {} forwards)",
                file.display(),
                config.peers.len(),
                config.forwards.len()
            );
        }
        Command::Identity { key } => println!("{}", native::identity::init(&key)?.public()),
        Command::Probe { command } => run_probe(command).await?,
    }
    Ok(0)
}

async fn connect_stdio(ipc: &Path, peer: String, target: String) -> Result<()> {
    let socket = peerhatch_control::connect(ipc, peer, target).await?;
    peerhatch_control::bridge(socket, stdio::input()?, stdio::output()?).await
}

async fn run_probe(command: ProbeCommand) -> Result<()> {
    if let Ok(filter) = tracing_subscriber::EnvFilter::try_from_default_env() {
        tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_writer(std::io::stderr)
            .with_ansi(false)
            .init();
    }
    let node = match &command {
        ProbeCommand::Listen(node) | ProbeCommand::Connect { node, .. } => node,
    };
    let config = load_config(&node.config)?;
    let peer = config
        .resolve_peer(&node.peer)?
        .1
        .endpoint_id
        .parse()
        .context("invalid peer public key")?;
    let key = native::identity::load(&node.key)?;
    let bind = native::bind_probe(key, &config.native, node.no_relay, node.relay_only);
    match command {
        ProbeCommand::Listen(node) => {
            let router = native::listen(bind.await?, peer);
            let ticket = native::ticket(router.endpoint(), node.no_relay).await?;
            if !node.no_relay && ticket.endpoint_addr().relay_urls().next().is_none() {
                eprintln!("Relay unavailable. This ticket contains direct addresses only.");
            }
            println!("ticket={ticket}");
            tokio::signal::ctrl_c().await?;
            router.shutdown().await?;
        }
        ProbeCommand::Connect { ticket, count, .. } => {
            let ticket = match ticket {
                Some(ticket) => ticket,
                None => native::relay_ticket(peer, &config.native)?,
            };
            let endpoint = bind.await?;
            let result = native::probe(&endpoint, ticket, peer, count, |sample| {
                println!(
                    "seq={} echo_ms={:.2} path={} addr={} path_tx_bytes={} path_rx_bytes={}",
                    sample.sequence,
                    sample.elapsed.as_secs_f64() * 1000.0,
                    sample.path,
                    sample.address,
                    sample.path_tx_bytes,
                    sample.path_rx_bytes
                );
            })
            .await;
            endpoint.close().await;
            result?;
        }
    }
    Ok(())
}
