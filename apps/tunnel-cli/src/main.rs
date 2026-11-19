use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use clap::{Args, Parser, Subcommand};
use tunnel_config::Config;
use tunnel_native::{self as native, EndpointTicket};

mod management;
mod shell;
mod stdio;
use management::{ForwardAction, PairAction, PortAction};

#[derive(Parser)]
#[command(name = "tunnel", about = "Native P2P Tunnel tools")]
struct Cli {
    /// Gateway socket path on Unix or local pipe name on Windows
    #[arg(long, global = true)]
    ipc: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Show the running gateway, peers, published services and bound ports
    Status {
        #[arg(long)]
        json: bool,
    },
    /// Pair two devices using a device code and explicit approval
    Pair {
        #[command(subcommand)]
        action: PairAction,
    },
    /// Export the running gateway connection ticket
    Ticket,
    /// Open a peer service on stdin/stdout
    Connect { peer: String, service: String },
    /// Open the paired device's native interactive shell
    Shell { peer: String },
    /// Run a command in a paired device's native terminal and return its exit status
    #[command(
        after_help = "Examples\n  tunnel exec home top\n  tunnel exec home -- vim notes.txt\n  tunnel exec home -c 'uname -a'"
    )]
    Exec {
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
    /// Manage a paired device's permission to open a shell on this device
    TerminalAccess {
        #[command(subcommand)]
        action: management::TerminalAccess,
    },
    /// Manage the TCP addresses a peer may connect to or listen on here
    PortAccess {
        #[command(subcommand)]
        action: PortAction,
    },
    /// Manage local and remote TCP listeners
    #[command(
        after_help = "Examples\n  tunnel forward add web --peer home --listen 8080 --target 3000\n  tunnel forward add preview --remote --peer home --listen 9000 --target 5000"
    )]
    Forward {
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
enum ConfigCommand {
    /// Validate a TOML file
    Check { file: PathBuf },
}

#[derive(Args)]
struct Node {
    /// Path to this device's private identity file
    #[arg(long)]
    key: PathBuf,
    /// Local TOML configuration
    #[arg(long)]
    config: PathBuf,
    /// Configured peer allowed to participate in this probe
    #[arg(long)]
    peer: String,
    /// Use direct IP connectivity only
    #[arg(long)]
    no_relay: bool,
}

#[derive(Subcommand)]
enum ProbeCommand {
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
    tunnel_config::parse(&data)
        .map_err(anyhow::Error::msg)
        .with_context(|| format!("parse {}", path.display()))
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let uses_stdio = matches!(
        cli.command,
        Command::Shell { .. } | Command::Exec { .. } | Command::Connect { .. }
    );
    let runtime = tokio::runtime::Runtime::new()?;
    let result = runtime.block_on(run(cli));
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

async fn run(cli: Cli) -> Result<u32> {
    let ipc = cli.ipc.unwrap_or_else(tunnel_control::default_address);
    match cli.command {
        Command::Shell { peer } => return shell::run(&ipc, peer, None).await,
        Command::Exec { peer, script, argv } => {
            use tunnel_control::terminal::exec::Invocation;
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
            return shell::run(&ipc, peer, Some(command)).await;
        }
        Command::TerminalAccess { action } => action.run(&ipc).await?,
        Command::Connect { peer, service } => {
            connect_stdio(&ipc, peer, service).await?;
        }
        Command::Status { json } => {
            let status = tunnel_control::request(&ipc, tunnel_control::Command::Status).await?;
            if json {
                println!("{}", serde_json::to_string_pretty(&status)?);
            } else {
                println!(
                    "gateway {} ({})\n{} paired devices, {} services",
                    status.display_name(),
                    status.device_id,
                    status.peers.len(),
                    status.services.len()
                );
                if let Some(path) = &status.config_path {
                    println!("config {}", path.display());
                }
                for (name, id) in &status.peers {
                    println!("peer {name} {id} ({})", status.peer_platform(name).label());
                }
                for (name, target) in status.services {
                    println!("service {name} -> {target}");
                }
                for (name, rule) in status.forwards {
                    println!("forward {name}: {} ({})", rule.route(), rule.state());
                }
            }
        }
        Command::Pair { action } => action.run(&ipc).await?,
        Command::Ticket => println!(
            "{}",
            tunnel_control::request(&ipc, tunnel_control::Command::Status)
                .await?
                .ticket
        ),
        Command::Forward { action } => action.run(&ipc).await?,
        Command::PortAccess { action } => action.run(&ipc).await?,
        Command::Config {
            command: ConfigCommand::Check { file },
        } => {
            let config = load_config(&file)?;
            println!(
                "{}: OK ({} peers, {} services, {} forwards)",
                file.display(),
                config.peers.len(),
                config.services.len(),
                config.forwards.len()
            );
        }
        Command::Identity { key } => println!("{}", native::identity::init(&key)?.public()),
        Command::Probe { command } => run_probe(command).await?,
    }
    Ok(0)
}

async fn connect_stdio(ipc: &Path, peer: String, service: String) -> Result<()> {
    let socket = tunnel_control::connect(ipc, peer, service).await?;
    tunnel_control::bridge(socket, stdio::input()?, stdio::output()?).await
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
        .peers
        .get(&node.peer)
        .with_context(|| format!("unknown peer {}", node.peer))?
        .endpoint_id
        .parse()
        .context("invalid peer public key")?;
    let key = native::identity::load(&node.key)?;
    let bind = native::bind(key, &config.native, node.no_relay);
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
