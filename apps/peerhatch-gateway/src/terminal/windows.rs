use std::{
    env,
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};

use anyhow::{Context, Result};
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

pub(super) async fn default_shell() -> Result<PathBuf> {
    let paths = candidates();
    if let Ok(Some(pwsh)) = timeout(Duration::from_secs(9), find_seven(&paths)).await {
        return Ok(pwsh.to_path_buf());
    }
    let root = env::var_os("SystemRoot").context("Windows system directory is unavailable")?;
    Ok(PathBuf::from(root).join(r"System32\WindowsPowerShell\v1.0\powershell.exe"))
}
