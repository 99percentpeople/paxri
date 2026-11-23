use clap::Parser;
use paxri_cli::{Command, GatewayOptions};

#[derive(Parser)]
#[command(
    name = "paxri",
    version,
    about = "Open terminals, copy files, and forward ports between paired devices"
)]
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
