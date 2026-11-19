use std::{
    io::{self, Write},
    path::PathBuf,
    process::Command,
};

use anyhow::{Context, Result, ensure};
use clap::Args;

#[derive(Args)]
pub struct Options {
    /// Paired device alias, optionally prefixed with user@
    #[arg(value_name = "DESTINATION")]
    target: String,
    /// Override the saved login name
    #[arg(short = 'l', long)]
    user: Option<String>,
    /// Published service name
    #[arg(long)]
    service: Option<String>,
    /// SSH identity file
    #[arg(short = 'i', long)]
    identity: Option<PathBuf>,
    /// OpenSSH client configuration file
    #[arg(short = 'F', long)]
    ssh_config: Option<PathBuf>,
    /// Prompt for login name and service, treating the destination as a device alias
    #[arg(long)]
    prompt: bool,
}

fn prompt(label: &str, default: &str) -> Result<Option<String>> {
    eprint!("{label} [{default}]: ");
    io::stderr().flush()?;
    let mut line = String::new();
    ensure!(io::stdin().read_line(&mut line)? > 0, "login canceled");
    let input = line.trim();
    Ok((!input.is_empty()).then(|| input.to_owned()))
}

fn host_alias(peer: &str, service: &str) -> String {
    if service == "ssh"
        && !peer.starts_with('-')
        && !peer.starts_with("p2p-alias-")
        && peer
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"._-".contains(&c))
    {
        return peer.to_owned();
    }
    // Only this alias appears in OpenSSH config matching and known_hosts.
    let hex = |value: &str| {
        value
            .bytes()
            .map(|c| format!("{c:02x}"))
            .collect::<String>()
    };
    format!("p2p-alias-{}-{}", hex(peer), hex(service))
}

pub fn run(options: Options, ipc: Option<PathBuf>) -> Result<()> {
    let ipc = ipc.unwrap_or_else(tunnel_control::default_address);
    let mut status = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?
        .block_on(tunnel_control::request(
            &ipc,
            tunnel_control::Command::Status,
        ))?;
    let mut user = options.user;
    let mut peer = options.target.as_str();
    if !options.prompt
        && !status.peers.contains_key(peer)
        && let Some((name, alias)) = peer.split_once('@')
    {
        ensure!(!name.is_empty(), "SSH login name is empty");
        user.get_or_insert_with(|| name.to_owned());
        peer = alias;
    }
    ensure!(!peer.is_empty(), "device alias is empty");
    ensure!(status.peers.contains_key(peer), "Unknown peer {peer}");
    let profile = status.ssh_profiles.remove(peer).unwrap_or_default();
    user = user.or(profile.user);
    let mut service = options.service.unwrap_or(profile.service);
    if options.prompt {
        eprintln!("SSH to {peer}");
        if let Some(name) = prompt("Login name", user.as_deref().unwrap_or("OpenSSH default"))? {
            user = Some(name);
        }
        if let Some(name) = prompt("Service", &service)? {
            service = name;
        }
    }
    ensure!(!service.is_empty(), "service name is empty");
    let exe = std::env::current_exe()?;
    let mut ssh = Command::new("ssh");
    // OpenSSH invokes ProxyCommand through a shell on Unix. User-controlled
    // device names, paths and service names travel via the environment.
    #[cfg(unix)]
    let proxy = {
        ssh.env("P2P_TUNNEL_SSH_CLIENT", &exe);
        r#"ProxyCommand="$P2P_TUNNEL_SSH_CLIENT" ssh-proxy"#.to_owned()
    };
    #[cfg(windows)]
    let proxy = {
        let path = exe.to_str().context("CLI path is not Unicode")?;
        let path = if let Some(share) = path.strip_prefix(r"\\?\UNC\") {
            format!("//{share}")
        } else {
            path.strip_prefix(r"\\?\").unwrap_or(path).to_owned()
        };
        let path = path.replace('\\', "/").replace('%', "%%");
        format!(r#"ProxyCommand="{path}" ssh-proxy"#)
    };
    ssh.env("P2P_TUNNEL_SSH_PEER", peer)
        .env("P2P_TUNNEL_SSH_SERVICE", &service)
        .arg("-o")
        .arg(proxy)
        .args(["-o", "ControlPath=none", "-o", "ProxyUseFdpass=no"]);
    ssh.env("TUNNEL_IPC", ipc);
    if let Some(path) = options.ssh_config {
        ssh.arg("-F").arg(path);
    }
    if let Some(path) = options
        .identity
        .or_else(|| profile.identity_file.map(PathBuf::from))
    {
        ssh.arg("-i").arg(path);
    }
    if let Some(user) = user {
        ssh.arg("-l").arg(user);
    }
    ssh.arg("--").arg(host_alias(peer, &service));
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        Err(ssh.exec()).context("start the OpenSSH client (ssh)")
    }
    #[cfg(windows)]
    {
        let status = ssh.status().context("start the OpenSSH client (ssh.exe)")?;
        std::process::exit(status.code().unwrap_or(1))
    }
}
