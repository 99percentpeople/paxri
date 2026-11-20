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
use tunnel_config::{file, startup::Startup};
use tunnel_control::Status;
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

pub async fn status(ipc: &Path) -> Result<Option<Status>> {
    match tunnel_control::request(ipc, tunnel_control::Command::Status).await {
        Ok(s) => Ok(Some(s)),
        Err(e) if tunnel_control::is_offline(&e) => Ok(None),
        Err(e) => Err(e),
    }
}

fn profile(path: Option<PathBuf>) -> Result<PathBuf> {
    Ok(std::path::absolute(
        path.or_else(file::user_config)
            .context("Cannot find the user configuration directory")?,
    )?)
}

fn gateway_exe() -> Result<PathBuf> {
    Ok(std::env::current_exe()?
        .with_file_name(format!("tunnel-gateway{}", std::env::consts::EXE_SUFFIX)))
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

pub async fn apply(action: Action, ipc: &Path, config: Option<PathBuf>) -> Result<Option<Status>> {
    if matches!(action, Action::Status) {
        return status(ipc).await;
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
        let address = ipc.to_owned();
        let path = config.as_deref().map(std::path::absolute).transpose()?;
        let code = tokio::task::spawn_blocking(move || {
            boot::run_client(
                Some(token),
                &std::env::current_exe()?,
                &address,
                command,
                path.as_deref(),
            )
        })
        .await??;
        ensure!(
            code == 0,
            "Could not {command} the gateway as the current standard user (exit {code})"
        );
        return status(ipc).await;
    }
    // Serialize local mutations across independent GUI/CLI processes, including first launch.
    let lock_path = profile(file::user_config().or_else(|| config.clone()))?;
    let _lock = tokio::task::spawn_blocking(move || launcher_lock(&lock_path)).await??;
    let current = status(ipc).await?;
    if let Some(s) = current.as_ref() {
        if let Some(path) = &config {
            check_profile(s, path)?;
        }
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
            if current.is_some() {
                tunnel_control::shutdown(ipc).await?;
            }
            if matches!(action, Action::Stop) {
                return Ok(None);
            }
        }
        Action::Startup { mode: Some(mode) } => {
            let target = ipc.to_owned();
            tokio::task::spawn_blocking(move || startup::configure(&config, &target, mode))
                .await??;
            return status(ipc).await;
        }
        Action::Start => {}
        Action::Status | Action::Startup { mode: None } => return Ok(current),
    }
    Ok(Some(start(ipc, &config).await?))
}

pub async fn open(ipc: &Path, config: Option<PathBuf>) -> Result<Option<Status>> {
    if let Some(s) = status(ipc).await? {
        return Ok(Some(s));
    }
    let config = profile(config)?;
    if file::startup(&config)? == Startup::Manual {
        return Ok(None);
    }
    apply(Action::Start, ipc, Some(config)).await
}

pub async fn cli(action: Action, ipc: &Path, config: Option<PathBuf>) -> Result<()> {
    if let Action::Startup { mode } = action {
        let active = status(ipc).await?;
        let path = profile(config.or_else(|| active.and_then(|s| s.config_path)))?;
        if mode.is_some() {
            apply(action, ipc, Some(path.clone())).await?;
        }
        println!("Startup: {}", file::startup(&path)?.as_str());
    } else {
        match apply(action, ipc, config).await? {
            Some(s) => println!(
                "Gateway running ({}) at {}",
                if s.user_managed {
                    "background"
                } else {
                    "externally managed"
                },
                ipc.display()
            ),
            None => println!("Gateway stopped at {}", ipc.display()),
        }
    }
    Ok(())
}
