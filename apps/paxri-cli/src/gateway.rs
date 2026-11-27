//! Local lifecycle shared by both clients. Only the gateway owns network listeners.
use anyhow::{Context, Result, bail, ensure};
use clap::Subcommand;
use std::{
    fs::{self, File, OpenOptions},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    time::Duration,
};
use tokio::time::{Instant, sleep};
use paxri_config::{file, startup::Startup};
use paxri_control::Status;
#[cfg(windows)]
pub(crate) mod boot;
mod startup;

#[derive(Clone, Copy, Debug, Subcommand)]
pub enum Action {
    /// Start the gateway in the background
    Start,
    /// Stop the background gateway
    Stop,
    /// Restart using the same node identity and configuration
    Restart,
    /// Show whether the selected gateway is running
    Status,
    /// Read or change startup (manual, gui, login, system)
    Startup { mode: Option<Startup> },
}

async fn status(ipc: &Path) -> Result<Option<Status>> {
    match paxri_control::request(ipc, paxri_control::Command::Status).await {
        Ok(s) => Ok(Some(s)),
        Err(e) if paxri_control::is_offline(&e) => Ok(None),
        Err(e) => Err(e),
    }
}

pub async fn discover(ipc: Option<&Path>, config: Option<&Path>) -> Result<Option<Status>> {
    let preferred = ipc
        .map(Path::to_owned)
        .unwrap_or_else(paxri_control::default_address);
    let primary = status(&preferred).await?;
    let mismatch = primary
        .as_ref()
        .zip(config)
        .and_then(|(s, path)| check_profile(s, path).err());
    if primary.is_some() && mismatch.is_none() {
        return Ok(primary);
    }
    if ipc.is_some() || paxri_control::ipc_from_env().is_some() {
        return mismatch.map_or(Ok(None), Err);
    }
    let Some(profile) = config
        .map(Path::to_owned)
        .or_else(file::user_config)
        .and_then(|p| p.canonicalize().ok())
    else {
        return Ok(None);
    };
    if let Some(candidate) = file::gateway_address(&profile)
        && candidate != preferred
        && let Ok(Some(active)) = status(&candidate).await
        && active
            .config_path
            .as_ref()
            .and_then(|p| p.canonicalize().ok())
            .as_ref()
            == Some(&profile)
    {
        return Ok(Some(active));
    }
    Ok(None)
}

pub fn other_installation(status: &Status) -> Option<PathBuf> {
    let actual = status.executable.as_deref()?;
    let expected = gateway_exe().ok()?;
    (!same_path(actual, &expected)).then_some(expected)
}

pub fn running_image(status: &Status) -> Option<String> {
    let actual = status.executable.as_deref()?;
    Some(if let Some(expected) = other_installation(status) {
        format!(
            "Running executable: {} (different installation; this copy uses {})",
            actual.display(),
            expected.display()
        )
    } else {
        format!("Running executable: {}", actual.display())
    })
}

fn same_path(a: &Path, b: &Path) -> bool {
    a == b
        || a.canonicalize()
            .ok()
            .zip(b.canonicalize().ok())
            .is_some_and(|(a, b)| a == b)
}

fn profile(path: Option<PathBuf>) -> Result<PathBuf> {
    Ok(std::path::absolute(
        path.or_else(file::user_config)
            .context("Cannot find the user configuration directory")?,
    )?)
}

fn gateway_exe() -> Result<PathBuf> {
    Ok(std::env::current_exe()?
        .with_file_name(format!("paxri-gateway{}", std::env::consts::EXE_SUFFIX)))
}

fn prepare_dir(config: &Path) -> Result<()> {
    let dir = config.parent().context("Missing profile directory")?;
    let mut create = fs::DirBuilder::new();
    create.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        create.mode(0o700);
    }
    Ok(create.create(dir)?)
}

fn launcher_lock(config: &Path) -> Result<File> {
    prepare_dir(config)?;
    let mut name = config.as_os_str().to_owned();
    name.push(".launcher.lock");
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(name)?;
    lock.lock()?;
    Ok(lock)
}

async fn lock_launcher(config: Option<&Path>) -> Result<File> {
    let path = profile(file::user_config().or_else(|| config.map(Path::to_owned)))?;
    tokio::task::spawn_blocking(move || launcher_lock(&path)).await?
}

pub async fn set_device_name(
    ipc: Option<&Path>,
    config: Option<&Path>,
    name: Option<String>,
) -> Result<Option<Status>> {
    let rename = |status: Status| {
        let command = paxri_control::Command::SetDeviceName { name: name.clone() };
        async move {
            let address = status.ipc_path.context("Missing gateway IPC address")?;
            paxri_control::request(&address, command).await.map(Some)
        }
    };
    if let Some(active) = discover(ipc, config).await? {
        match rename(active).await {
            Err(err) if paxri_control::is_offline(&err) => {}
            result => return result,
        }
    }
    let path = config.context("Use --config to select this offline gateway's node TOML")?;
    // A concurrent desktop or CLI start must read the new name or receive the live rename.
    let _lock = lock_launcher(Some(path)).await?;
    if let Some(active) = discover(ipc, Some(path)).await? {
        return rename(active).await;
    }
    let path = path.to_owned();
    tokio::task::spawn_blocking(move || file::set_device_name(&path, name.as_deref())).await??;
    Ok(None)
}

fn launch(config: &Path, ipc: &Path) -> Result<(Child, PathBuf)> {
    let exe = gateway_exe()?;
    prepare_dir(config)?;
    let log_path = config.with_file_name("gateway.log");
    let log = OpenOptions::new()
        .append(true)
        .create(true)
        .open(&log_path)
        .with_context(|| format!("open {}", log_path.display()))?;
    let mut cmd = Command::new(&exe);
    cmd.arg("--user-managed")
        .arg("--config")
        .arg(config)
        .arg("--key")
        .arg(config.with_file_name("device.key"))
        .arg("--ipc")
        .arg(ipc)
        .stdin(Stdio::null())
        .stdout(log.try_clone()?)
        .stderr(log);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        use windows_sys::Win32::{
            Foundation::{HANDLE_FLAG_INHERIT, INVALID_HANDLE_VALUE, SetHandleInformation},
            System::Console::{
                GetStdHandle, STD_ERROR_HANDLE, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE,
            },
        };
        // Redirected callers may supply inheritable pipes. The child has its own log
        // and NUL handles. Let the caller receive EOF as soon as this launcher exits.
        for which in [STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, STD_ERROR_HANDLE] {
            let handle = unsafe { GetStdHandle(which) };
            if !handle.is_null() && handle != INVALID_HANDLE_VALUE {
                unsafe {
                    SetHandleInformation(handle, HANDLE_FLAG_INHERIT, 0);
                }
            }
        }
        cmd.creation_flags(windows_sys::Win32::System::Threading::DETACHED_PROCESS);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // setsid is async-signal-safe. The child must outlive the invoking terminal.
        unsafe {
            cmd.pre_exec(|| rustix::process::setsid().map(|_| ()).map_err(Into::into));
        }
    }
    Ok((
        cmd.spawn()
            .with_context(|| format!("start {}", exe.display()))?,
        log_path,
    ))
}

async fn start(ipc: &Path, config: &Path) -> Result<Status> {
    let (mut child, log) = launch(config, ipc)?;
    let deadline = Instant::now() + Duration::from_secs(35);
    loop {
        if let Ok(Some(s)) = status(ipc).await {
            // A GUI may outlive many successive gateway processes.
            std::thread::spawn(move || {
                let _ = child.wait();
            });
            check_profile(&s, config)?;
            return Ok(s);
        }
        if let Some(code) = child.try_wait()? {
            bail!("Gateway exited ({code}). See {}", log.display());
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            bail!("Gateway did not become ready. See {}", log.display());
        }
        sleep(Duration::from_millis(100)).await;
    }
}

fn check_profile(status: &Status, config: &Path) -> Result<()> {
    if let Some(active) = &status.config_path {
        ensure!(
            config.canonicalize().ok().as_ref() == Some(&active.canonicalize()?),
            "This IPC address already serves {}",
            active.display()
        );
    }
    Ok(())
}

pub async fn apply(
    action: Action,
    ipc: Option<&Path>,
    config: Option<PathBuf>,
) -> Result<Option<Status>> {
    if matches!(action, Action::Status | Action::Startup { mode: None }) {
        return discover(ipc, config.as_deref()).await;
    }
    // An administrator may open Settings to register a boot task. Keep a manual
    // start or restart in that same window at the gateway user's normal rights.
    #[cfg(windows)]
    if matches!(action, Action::Start | Action::Restart)
        && let Some(token) = boot::user_token()?
    {
        let command = if matches!(action, Action::Restart) {
            "restart"
        } else {
            "start"
        };
        let address = ipc.map(Path::to_owned);
        let path = config.as_deref().map(std::path::absolute).transpose()?;
        let code = tokio::task::spawn_blocking(move || {
            boot::run_client(
                Some(token),
                &std::env::current_exe()?,
                address.as_deref(),
                command,
                path.as_deref(),
            )
        })
        .await??;
        ensure!(
            code == 0,
            "Could not {command} the gateway as the current standard user (exit {code})"
        );
        return discover(ipc, config.as_deref()).await;
    }
    // Serialize local mutations across independent GUI/CLI processes, including first launch.
    let _lock = lock_launcher(config.as_deref()).await?;
    let current = discover(ipc, config.as_deref()).await?;
    let preferred = ipc
        .map(Path::to_owned)
        .unwrap_or_else(paxri_control::default_address);
    if let Some(s) = current.as_ref() {
        if matches!(action, Action::Start) {
            return Ok(current);
        }
        ensure!(
            s.user_managed,
            "This gateway is managed by its external launcher"
        );
    }
    let config = profile(config.or_else(|| current.as_ref().and_then(|s| s.config_path.clone())))?;

    match action {
        Action::Stop | Action::Restart => {
            if let Some(address) = current.as_ref().and_then(|s| s.ipc_path.as_ref()) {
                paxri_control::shutdown(address).await?;
            }
            if matches!(action, Action::Stop) {
                return Ok(None);
            }
        }
        Action::Startup { mode: Some(mode) } => {
            let target = ipc
                .map(Path::to_owned)
                .or_else(paxri_control::ipc_from_env);
            let path = config.clone();
            tokio::task::spawn_blocking(move || startup::configure(&path, target.as_deref(), mode))
                .await??;
            return discover(ipc, Some(&config)).await;
        }
        Action::Start => {}
        Action::Status | Action::Startup { mode: None } => return Ok(current),
    }
    if current.is_none() && let Some(other) = status(&preferred).await? {
        check_profile(&other, &config)?;
        return Ok(Some(other));
    }
    // Restart on the existing address so other clients can reconnect there.
    let target = current
        .as_ref()
        .and_then(|s| s.ipc_path.as_deref())
        .unwrap_or(&preferred);
    Ok(Some(start(target, &config).await?))
}

pub async fn open(ipc: Option<&Path>, config: Option<PathBuf>) -> Result<Option<Status>> {
    if let Some(s) = discover(ipc, config.as_deref()).await? {
        return Ok(Some(s));
    }
    let config = profile(config)?;
    if file::startup(&config)? == Startup::Manual {
        return Ok(None);
    }
    apply(Action::Start, ipc, Some(config)).await
}

/// Ensure a local gateway is running for CLI commands. Honors `gateway.startup = "manual"`
/// by refusing to auto-launch and returning an actionable message instead.
pub async fn ensure_running(ipc: Option<&Path>, config: Option<PathBuf>) -> Result<Status> {
    match open(ipc, config).await? {
        Some(status) => Ok(status),
        None => bail!(
            "Gateway is not running (startup is set to manual). Run `paxri gateway start` first."
        ),
    }
}

pub(crate) fn is_manual_startup(config: Option<PathBuf>) -> bool {
    profile(config)
        .ok()
        .and_then(|path| file::startup(&path).ok())
        .is_some_and(|mode| mode == Startup::Manual)
}

#[cfg(windows)]
pub fn startup_report(path: &Path) -> Vec<String> {
    startup::inspect(path)
        .unwrap_or_else(|err| vec![format!("Windows Task Scheduler unavailable: {err:#}")])
}

pub async fn cli(action: Action, ipc: Option<&Path>, config: Option<PathBuf>) -> Result<()> {
    let active = apply(action, ipc, config.clone()).await?;
    let preferred = ipc
        .map(Path::to_owned)
        .unwrap_or_else(paxri_control::default_address);
    if !matches!(action, Action::Startup { .. }) {
        if let Some(s) = &active {
            let address = s.ipc_path.as_deref().unwrap_or(&preferred);
            println!(
                "Gateway running ({}) at {}",
                if s.user_managed {
                    "background"
                } else {
                    "externally managed"
                },
                address.display()
            );
            if let Some(image) = running_image(s) {
                println!("{image}");
            }
        } else if matches!(action, Action::Stop) {
            println!("Gateway stopped");
        } else {
            println!("Gateway stopped at {}", preferred.display());
        }
    }
    if matches!(action, Action::Startup { .. } | Action::Status) {
        let path = profile(config.or_else(|| active.and_then(|s| s.config_path)))?;
        let mode = file::startup(&path)?;
        println!("Startup: {}", mode.as_str());
        #[cfg(windows)]
        for line in startup_report(&path) {
            println!("{line}");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn ensure_running_rejects_auto_start_when_startup_is_manual() -> Result<()> {
        let dir = tempfile::Builder::new().prefix("paxri-cli-manual-").tempdir()?;
        let config = dir.path().join("node.toml");
        fs::write(&config, "version = 1\n[gateway]\nstartup = \"manual\"\n")?;
        #[cfg(unix)]
        let ipc = dir.path().join("gateway.sock");
        #[cfg(windows)]
        let ipc = PathBuf::from(format!("paxri-test-{}", dir.path().file_name().unwrap().to_string_lossy()));

        assert!(is_manual_startup(Some(config.clone())));
        let error = ensure_running(Some(&ipc), Some(config)).await.unwrap_err();
        let message = error.to_string();
        assert!(
            message.contains("startup is set to manual")
                && message.contains("paxri gateway start"),
            "unexpected error: {message}"
        );
        #[cfg(unix)]
        assert!(!ipc.exists(), "manual startup must not launch a gateway");
        Ok(())
    }
}
