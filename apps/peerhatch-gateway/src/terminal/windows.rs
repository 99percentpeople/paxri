use std::{
    env,
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};

use anyhow::{Context, Result};
use peerhatch_terminal::pty::CommandBuilder;
use tokio::{fs, process::Command, time::timeout};
use windows_sys::Win32::System::Threading::CREATE_NO_WINDOW;

pub(super) fn pipe_job(child: &tokio::process::Child) -> Result<std::os::windows::io::OwnedHandle> {
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
    use windows_sys::Win32::System::JobObjects::{AssignProcessToJobObject, CreateJobObjectW};
    let handle = child.raw_handle().context("missing process handle")?;
    unsafe {
        let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
        if job.is_null() {
            return Err(std::io::Error::last_os_error().into());
        }
        let job = OwnedHandle::from_raw_handle(job);
        if AssignProcessToJobObject(job.as_raw_handle(), handle) == 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        Ok(job)
    }
}

fn candidates() -> Vec<PathBuf> {
    let roots: Vec<_> = ["ProgramW6432", "ProgramFiles", "ProgramFiles(x86)"]
        .into_iter()
        .filter_map(env::var_os)
        .map(PathBuf::from)
        .collect();
    let path = env::var_os("PATH").unwrap_or_default();
    let mut result = Vec::<PathBuf>::new();
    for dir in roots
        .iter()
        .map(|p| p.join(r"PowerShell\7"))
        .chain(env::split_paths(&path))
        .chain(env::var_os("LOCALAPPDATA").map(|p| PathBuf::from(p).join(r"Microsoft\WindowsApps")))
        .chain(roots.iter().map(|p| p.join(r"PowerShell\7-preview")))
        .filter(|p| p.is_absolute())
    {
        let exe = dir.join("pwsh.exe");
        if !result
            .iter()
            .any(|p| p.as_os_str().eq_ignore_ascii_case(exe.as_os_str()))
        {
            result.push(exe);
        }
    }
    result
}

fn system_powershell() -> Option<PathBuf> {
    Some(PathBuf::from(env::var_os("SystemRoot")?).join(r"System32\WindowsPowerShell\v1.0\powershell.exe"))
}

pub(super) fn has_shell() -> bool {
    system_powershell().is_some_and(|p| p.is_file()) || candidates().iter().any(|p| p.is_file())
}

async fn find_seven(paths: &[PathBuf]) -> Option<&Path> {
    for path in paths {
        if fs::symlink_metadata(path).await.is_err() {
            continue;
        }
        // PowerShell 6 also uses pwsh.exe. This works with portable builds and Store aliases.
        let mut probe = Command::new(path);
        probe
            .args([
                "-NoLogo",
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                "exit $PSVersionTable.PSVersion.Major",
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .creation_flags(CREATE_NO_WINDOW)
            .kill_on_drop(true);
        if let Ok(Ok(status)) = timeout(Duration::from_secs(4), probe.status()).await {
            if status.code() == Some(7) {
                return Some(path);
            }
        }
    }
    None
}

const LAUNCH_POWERSHELL: &str = "--launch-powershell";
const UTF8_PIPELINE: &str = "$global:OutputEncoding = [Text.UTF8Encoding]::new($false)";

pub(crate) fn launch_if_requested() -> Result<()> {
    use windows_sys::Win32::System::Console::{
        CTRL_BREAK_EVENT, CTRL_C_EVENT, SetConsoleCP, SetConsoleCtrlHandler, SetConsoleOutputCP,
    };
    let mut args = env::args_os().skip(1);
    if args.next().as_deref() != Some(std::ffi::OsStr::new(LAUNCH_POWERSHELL)) {
        return Ok(());
    }
    let exe = args.next().context("Missing PowerShell executable")?;
    // Ctrl+C belongs to the shell. Registering a handler (rather than ignoring
    // Ctrl+C via a null handler) leaves the child's own signal handling intact.
    unsafe extern "system" fn handle(event: u32) -> i32 {
        i32::from(event == CTRL_C_EVENT || event == CTRL_BREAK_EVENT)
    }
    // This process owns the session's ConPTY or CREATE_NO_WINDOW console. Set
    // its code pages before PowerShell starts: it may decode $input at startup.
    unsafe {
        if SetConsoleCP(65001) == 0
            || SetConsoleOutputCP(65001) == 0
            || SetConsoleCtrlHandler(Some(handle), 1) == 0
        {
            return Err(std::io::Error::last_os_error().into());
        }
    }
    let status = std::process::Command::new(exe).args(args).status()?;
    std::process::exit(status.code().unwrap_or(1));
}

pub(super) async fn shell_command(script: Option<&str>) -> Result<CommandBuilder> {
    let paths = candidates();
    let mut cmd = CommandBuilder::new(env::current_exe()?);
    cmd.arg(LAUNCH_POWERSHELL);
    if let Ok(Some(pwsh)) = timeout(Duration::from_secs(9), find_seven(&paths)).await {
        cmd.arg(pwsh);
        if let Some(script) = script {
            cmd.args(["-NoLogo", "-NoProfile", "-Command", script]);
        }
        return Ok(cmd);
    }
    let exe = system_powershell().context("Windows system directory is unavailable")?;
    cmd.arg(&exe);
    // Windows PowerShell 5.1 also defaults to ASCII for text piped to native
    // programs. Set that session variable while preserving the script prologue.
    if let Some(script) = script {
        let mut prepare = Command::new(exe);
        if let Some(home) = env::var_os("USERPROFILE") {
            prepare.current_dir(home);
        }
        let output = timeout(
            Duration::from_secs(4),
            prepare
                .args([
                    "-NoLogo",
                    "-NoProfile",
                    "-NonInteractive",
                    "-Command",
                    include_str!("prepare-command.ps1"),
                ])
                .env("PEERHATCH_PS_SOURCE", script)
                .env("PEERHATCH_PS_SETUP", UTF8_PIPELINE)
                .stdin(Stdio::null())
                .creation_flags(CREATE_NO_WINDOW)
                .kill_on_drop(true)
                .output(),
        )
        .await??;
        anyhow::ensure!(
            output.status.success(),
            "Could not prepare Windows PowerShell command"
        );
        cmd.args(["-NoLogo", "-NoProfile", "-Command"]);
        cmd.arg(String::from_utf8(output.stdout)?);
    } else {
        cmd.args(["-NoExit", "-Command", UTF8_PIPELINE]);
    }
    Ok(cmd)
}
