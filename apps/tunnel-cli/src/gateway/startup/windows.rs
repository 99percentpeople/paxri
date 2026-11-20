use anyhow::{Context, Result, ensure};
use std::{os::windows::process::CommandExt, path::Path, process::Command};
use tunnel_config::startup::Startup;
use windows_sys::Win32::System::Threading::CREATE_NO_WINDOW;

pub(super) fn register(
    id: &str,
    config: &Path,
    ipc: &Path,
    old: Startup,
    mode: Startup,
) -> Result<()> {
    use Startup::{Login, System};
    if !matches!(old, Login | System) && !matches!(mode, Login | System) {
        return Ok(());
    }
    // Both paths are individual argv entries. Windows file and local pipe names cannot contain a quote.
    let ipc = ipc.to_str().context("Invalid Windows pipe name")?;
    ensure!(
        !ipc.contains(['"', '\\', '/']) && !ipc.is_empty(),
        "Use a local pipe name"
    );
    let config = config
        .to_str()
        .context("Invalid Windows configuration path")?;
    let args = format!(r#"--ipc "{ipc}" gateway start --config "{config}""#);
    let gui = std::env::current_exe()?.with_file_name("tunnel-gui.exe");
    let exe = if gui.is_file() {
        gui
    } else {
        std::env::current_exe()?
    };
    let client = exe.to_str().context("Invalid Windows executable path")?;
    let boot_args =
        format!(r#"--ipc "{ipc}" boot-gateway --client "{client}" --node-file "{config}""#);
    let system_root = std::env::var_os("SystemRoot").context("Cannot find Windows")?;
    let shell = Path::new(&system_root)
        .join("System32")
        .join("WindowsPowerShell")
        .join("v1.0")
        .join("powershell.exe");
    let output = Command::new(shell)
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            include_str!("task.ps1"),
        ])
        .env("P2P_STARTUP_TASK", id)
        .env("P2P_STARTUP_EXE", exe)
        .env("P2P_BOOT_SOURCE", std::env::current_exe()?)
        .env("P2P_BOOT_ARGS", boot_args)
        .env("P2P_PREVIOUS_MODE", old.as_str())
        .env("P2P_STARTUP_ARGS", args)
        .env("P2P_STARTUP_MODE", mode.as_str())
        .creation_flags(CREATE_NO_WINDOW)
        .output()?;
    ensure!(
        output.status.success(),
        "Windows Task Scheduler: {}",
        String::from_utf8_lossy(&output.stderr).trim()
    );
    Ok(())
}
