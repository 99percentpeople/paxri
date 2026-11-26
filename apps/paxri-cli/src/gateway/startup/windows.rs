use anyhow::{Context, Result, ensure};
use std::{
    os::windows::process::CommandExt,
    path::{Path, PathBuf},
    process::Command,
};
use paxri_config::startup::Startup;
use windows_sys::Win32::System::Threading::CREATE_NO_WINDOW;

fn powershell() -> Result<Command> {
    let system_root = std::env::var_os("SystemRoot").context("Cannot find Windows")?;
    let shell = Path::new(&system_root).join("System32/WindowsPowerShell/v1.0/powershell.exe");
    let mut command = Command::new(shell);
    command
        .args(["-NoProfile", "-NonInteractive", "-Command"])
        .creation_flags(CREATE_NO_WINDOW);
    Ok(command)
}

fn startup_client() -> Result<PathBuf> {
    let current = std::env::current_exe()?;
    let gui = current.with_file_name("paxri-gui.exe");
    Ok(if gui.is_file() { gui } else { current })
}

pub(super) fn inspect(id: &str, mode: Startup) -> Result<Vec<String>> {
    let output = powershell()?
        .arg(include_str!("inspect.ps1"))
        .env("PAXRI_STARTUP_TASK", id)
        .env("PAXRI_STARTUP_EXE", startup_client()?)
        .env("PAXRI_STARTUP_MODE", mode.as_str())
        .output()?;
    ensure!(
        output.status.success(),
        "PowerShell exited with {}. {}",
        output.status,
        String::from_utf8_lossy(&output.stderr).trim()
    );
    Ok(String::from_utf8(output.stdout)?
        .lines()
        .map(str::to_owned)
        .collect())
}

pub(super) fn register(
    id: &str,
    config: &Path,
    ipc: Option<&Path>,
    old: Startup,
    mode: Startup,
) -> Result<()> {
    // Both paths are individual argv entries. Windows file and local pipe names cannot contain a quote.
    let ipc_arg = if let Some(path) = ipc {
        let name = path.to_str().context("Invalid Windows pipe name")?;
        ensure!(
            !name.contains(['"', '\\', '/']) && !name.is_empty(),
            "Use a local pipe name"
        );
        format!(r#"--ipc "{name}" "#)
    } else {
        String::new()
    };
    let config = config
        .to_str()
        .context("Invalid Windows configuration path")?;
    let args = format!(r#"{ipc_arg}gateway start --config "{config}""#);
    let exe = startup_client()?;
    let client = exe.to_str().context("Invalid Windows executable path")?;
    let boot_args = format!(r#"{ipc_arg}boot-gateway --client "{client}" --node-file "{config}""#);
    let output = powershell()?
        .arg(include_str!("task.ps1"))
        .env("PAXRI_STARTUP_TASK", id)
        .env("PAXRI_STARTUP_EXE", exe)
        .env("PAXRI_BOOT_SOURCE", std::env::current_exe()?)
        .env("PAXRI_BOOT_ARGS", boot_args)
        .env("PAXRI_PREVIOUS_MODE", old.as_str())
        .env("PAXRI_STARTUP_ARGS", args)
        .env("PAXRI_STARTUP_MODE", mode.as_str())
        .output()?;
    ensure!(
        output.status.success(),
        "Windows Task Scheduler: {}",
        String::from_utf8_lossy(&output.stderr).trim()
    );
    Ok(())
}
