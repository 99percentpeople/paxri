use anyhow::{Context, Result, ensure};
use std::{fs, io::Write, path::Path, process::Command};
use tunnel_config::startup::Startup;

fn put(path: &Path, text: &str) -> Result<()> {
    let dir = path.parent().context("Missing startup directory")?;
    fs::create_dir_all(dir)?;
    let mut file = tempfile::NamedTempFile::new_in(dir)?;
    file.write_all(text.as_bytes())?;
    file.persist(path)?;
    Ok(())
}

fn remove(path: &Path) -> Result<()> {
    match fs::remove_file(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        r => Ok(r?),
    }
}

#[cfg(target_os = "linux")]
fn run(program: &str, args: &[&str]) -> Result<()> {
    let result = Command::new(program)
        .args(args)
        .output()
        .with_context(|| format!("Run {program}"))?;
    ensure!(
        result.status.success(),
        "{program}: {}",
        String::from_utf8_lossy(&result.stderr).trim()
    );
    Ok(())
}

pub(super) fn register(
    id: &str,
    config: &Path,
    ipc: &Path,
    old: Startup,
    mode: Startup,
) -> Result<()> {
    if ![old, mode]
        .iter()
        .any(|s| matches!(s, Startup::Login | Startup::System))
    {
        return Ok(());
    }
    let exe = super::super::gateway_exe()?;
    let key = config.with_file_name("device.key");
    let ipc = std::path::absolute(ipc)?;
    let args = [
        exe.to_str().context("Invalid executable path")?,
        "--user-managed",
        "--config",
        config.to_str().context("Invalid configuration path")?,
        "--key",
        key.to_str().context("Invalid key path")?,
        "--ipc",
        ipc.to_str().context("Invalid IPC path")?,
    ];
    ensure!(
        args.iter().all(|s| !s.chars().any(char::is_control)),
        "Unsupported control character in startup path"
    );
    if let Err(error) = register_native(id, old, mode, &args) {
        if old != mode {
            register_native(id, mode, old, &args).with_context(|| {
                format!("{error:#}. Could not restore the previous startup registration")
            })?;
        }
        return Err(error);
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn register_native(id: &str, old: Startup, mode: Startup, args: &[&str]) -> Result<()> {
    use std::os::unix::fs::symlink;
    let cfg = dirs::config_dir().context("Cannot find the login configuration directory")?;
    let desktop = cfg.join("autostart").join(format!("{id}.desktop"));
    let user = cfg.join("systemd/user");
    let unit = user.join(format!("{id}.service"));
    let want = user
        .join("default.target.wants")
        .join(format!("{id}.service"));
    if mode == Startup::Login {
        // Desktop entries unescape strings before processing the quoted Exec arguments.
        let command = args
            .iter()
            .map(|s| {
                let escaped = s
                    .replace('\\', "\\\\")
                    .replace('"', "\\\"")
                    .replace('$', "\\$")
                    .replace('`', "\\`")
                    .replace('%', "%%");
                format!("\"{}\"", escaped.replace('\\', "\\\\"))
            })
            .collect::<Vec<_>>()
            .join(" ");
        put(
            &desktop,
            &format!(
                "[Desktop Entry]\nType=Application\nName=P2P Tunnel Gateway\nExec={command}\nTerminal=false\n"
            ),
        )?;
    }
    if mode == Startup::System {
        ensure!(
            !rustix::process::getuid().is_root(),
            "Set startup from the gateway user's own account"
        );
        let uid = rustix::process::getuid().as_raw().to_string();
        let command = args
            .iter()
            .map(|s| {
                format!(
                    "\"{}\"",
                    s.replace('\\', "\\\\")
                        .replace('"', "\\\"")
                        .replace('%', "%%")
                        .replace('$', "$$")
                )
            })
            .collect::<Vec<_>>()
            .join(" ");
        let text = format!(
            "[Unit]\nDescription=P2P Tunnel background gateway\n[Service]\nExecStart={command}\n[Install]\nWantedBy=default.target\n"
        );
        put(&unit, &text)?;
        fs::create_dir_all(want.parent().unwrap())?;
        match symlink(&unit, &want) {
            Err(e)
                if e.kind() == std::io::ErrorKind::AlreadyExists
                    && fs::read_link(&want)? == unit => {}
            r => r?,
        }
        let already_enabled = Command::new("loginctl")
            .args(["show-user", &uid, "--property=Linger", "--value"])
            .output()
            .is_ok_and(|r| r.status.success() && r.stdout.trim_ascii() == b"yes");
        if !already_enabled {
            run("loginctl", &["--no-ask-password", "enable-linger", &uid])?;
        }
    }
    if old == Startup::Login && mode != old {
        remove(&desktop)?;
    }
    if old == Startup::System && mode != old {
        remove(&want)?;
        remove(&unit)?;
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn launch_agent(path: &Path, text: Option<&str>, uid: u32, gid: u32) -> Result<()> {
    if uid == rustix::process::geteuid().as_raw() {
        return match text {
            Some(text) => put(path, text),
            None => remove(path),
        };
    }
    // When switching a system job into the login domain, even the temporary
    // file and its parent directory must be created with the user's rights.
    use std::{os::unix::process::CommandExt, process::Stdio};
    let install = r#"set -eu
umask 077
/bin/mkdir -p "$1"
draft=$(/usr/bin/mktemp "$1/.p2p-tunnel.XXXXXXXX")
trap '/bin/rm -f "$draft"' EXIT
/bin/cat > "$draft"
/bin/mv -f "$draft" "$2""#;
    let script = if text.is_some() {
        install
    } else {
        r#"exec /bin/rm -f "$2""#
    };
    let mut child = Command::new("/bin/sh")
        .env_clear()
        .uid(uid)
        .gid(gid)
        .args(["-c", script, "p2p-launch-agent"])
        .arg(path.parent().context("Missing LaunchAgents directory")?)
        .arg(path)
        .stdin(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    if let Some(text) = text {
        child
            .stdin
            .take()
            .context("Missing launch agent input")?
            .write_all(text.as_bytes())?;
    }
    let output = child.wait_with_output()?;
    ensure!(
        output.status.success(),
        "Update LaunchAgent: {}",
        String::from_utf8_lossy(&output.stderr).trim()
    );
    Ok(())
}

#[cfg(target_os = "macos")]
fn register_native(id: &str, old: Startup, mode: Startup, args: &[&str]) -> Result<()> {
    use std::os::unix::fs::MetadataExt;
    let owner = fs::metadata(args[3])?;
    let uid = owner.uid();
    let name = Command::new("/usr/bin/id")
        .args(["-nu", &uid.to_string()])
        .output()?;
    ensure!(name.status.success(), "Cannot find the gateway user");
    let name = std::str::from_utf8(&name.stdout)?.trim();
    let home = if uid == rustix::process::getuid().as_raw() {
        dirs::home_dir().context("Cannot find user home")?
    } else {
        let record = Command::new("/usr/bin/dscl")
            .args([
                "/Search",
                "-read",
                &format!("/Users/{name}"),
                "NFSHomeDirectory",
            ])
            .output()?;
        ensure!(
            record.status.success(),
            "Cannot find home directory for {name}"
        );
        std::path::PathBuf::from(
            std::str::from_utf8(&record.stdout)?
                .trim()
                .strip_prefix("NFSHomeDirectory: ")
                .context("Invalid account home directory")?,
        )
    };
    let login = home
        .join("Library/LaunchAgents")
        .join(format!("{id}.plist"));
    let system = std::path::PathBuf::from("/Library/LaunchDaemons").join(format!("{id}.plist"));
    let xml = |s: &str| {
        s.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
    };
    let update = |m, text: Option<&str>| {
        if m == Startup::System {
            match text {
                Some(text) => put(&system, text),
                None => remove(&system),
            }
        } else {
            launch_agent(&login, text, uid, owner.gid())
        }
    };
    if matches!(mode, Startup::System | Startup::Login) {
        let user = if mode == Startup::System {
            ensure!(
                uid != 0,
                "Choose a configuration owned by the gateway's regular user"
            );
            ensure!(
                rustix::process::geteuid().is_root(),
                "Installing a LaunchDaemon requires administrator permission"
            );
            format!("<key>UserName</key><string>{}</string>", xml(name))
        } else {
            String::new()
        };
        let arguments = args
            .iter()
            .map(|s| format!("<string>{}</string>", xml(s)))
            .collect::<String>();
        let text = format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<plist version=\"1.0\"><dict><key>Label</key><string>{id}</string><key>ProgramArguments</key><array>{arguments}</array>{user}<key>RunAtLoad</key><true/></dict></plist>"
        );
        update(mode, Some(&text))?;
    }
    if matches!(old, Startup::System | Startup::Login) && old != mode {
        update(old, None)?;
    }
    Ok(())
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn register_native(_: &str, _: Startup, _: Startup, _: &[&str]) -> Result<()> {
    anyhow::bail!("OS startup registration is unavailable on this platform")
}
