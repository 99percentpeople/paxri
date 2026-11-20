//! Register only the selected profile, using the operating system's startup facilities.
use anyhow::{Context, Result};
use std::path::Path;
use tunnel_config::{file, startup::Startup};

#[cfg(unix)]
mod unix;
#[cfg(windows)]
mod windows;

pub(super) fn configure(config: &Path, ipc: &Path, mode: Startup) -> Result<()> {
    file::init(config)?;
    let old = file::startup(config)?;
    // The name stays the same when the portable binaries move to another folder.
    let path = config.canonicalize()?;
    let id = path
        .as_os_str()
        .as_encoded_bytes()
        .iter()
        .fold(0xcbf29ce484222325u64, |h, &b| {
            (h ^ u64::from(b)).wrapping_mul(0x100000001b3)
        });
    let id = format!("org.p2ptunnel.gateway-{id:016x}");
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
