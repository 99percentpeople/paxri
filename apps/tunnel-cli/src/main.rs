use clap::Parser;
use tunnel_cli::{Command, GatewayOptions};

#[derive(Parser)]
#[command(name = "tunnel", about = "Native P2P Tunnel tools")]
struct Cli {
    #[command(flatten)]
    gateway: GatewayOptions,
    #[command(subcommand)]
    command: Command,
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    cli.command.run(cli.gateway)
}
