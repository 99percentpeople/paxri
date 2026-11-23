//! Register only the selected profile, using the operating system's startup facilities.
use anyhow::{Context, Result};
use std::path::Path;
use paxri_config::{file, startup::Startup};

#[cfg(unix)]
mod unix;
#[cfg(windows)]
mod windows;

fn task_id(config: &Path) -> Result<String> {
    let path = match config.canonicalize() {
        Ok(path) => path,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            let parent = config.parent().and_then(|p| p.canonicalize().ok());
            match (parent, config.file_name()) {
                (Some(parent), Some(name)) => parent.join(name),
                _ => std::path::absolute(config)?,
            }
        }
        Err(err) => return Err(err.into()),
    };
    let hash = path
        .as_os_str()
        .as_encoded_bytes()
        .iter()
        .fold(0xcbf29ce484222325u64, |hash, &byte| {
            (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3)
        });
    Ok(format!("org.paxri.gateway-{hash:016x}"))
}

#[cfg(windows)]
pub(super) fn inspect(config: &Path) -> Result<Vec<String>> {
    windows::inspect(&task_id(config)?, file::startup(config)?)
}

pub(super) fn configure(config: &Path, ipc: Option<&Path>, mode: Startup) -> Result<()> {
    file::init(config)?;
    let old = file::startup(config)?;
    let id = task_id(config)?;
    #[cfg(unix)]
    let default_ipc = paxri_control::default_address();
    #[cfg(unix)]
    let ipc = ipc.unwrap_or(&default_ipc);
    #[cfg(unix)]
    use unix::register;
    #[cfg(windows)]
    use windows::register;
    register(&id, config, ipc, old, mode)?;
    if let Err(e) = file::set_startup(config, mode) {
        register(&id, config, ipc, mode, old)
            .context("Could not restore the previous startup registration")?;
        return Err(e);
    }
    Ok(())
}
