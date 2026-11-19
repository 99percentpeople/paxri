use std::{
    fs,
    io::{ErrorKind, Write},
    path::Path,
};

use anyhow::{Context, Result};
use iroh::SecretKey;
use tempfile::NamedTempFile;

pub fn load(path: &Path) -> Result<SecretKey> {
    let bytes = fs::read(path).with_context(|| format!("read device key {}", path.display()))?;
    let bytes: [u8; 32] = bytes
        .try_into()
        .map_err(|_| anyhow::anyhow!("invalid device key length in {}", path.display()))?;
    Ok(SecretKey::from_bytes(&bytes))
}

pub fn init(path: &Path) -> Result<SecretKey> {
    match fs::metadata(path) {
        Ok(_) => return load(path),
        Err(err) if err.kind() == ErrorKind::NotFound => {}
        Err(err) => return Err(err).context("inspect device key"),
    }

    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let mut file = NamedTempFile::new_in(parent).context("create private key file")?;
    let key = SecretKey::generate();
    file.write_all(&key.to_bytes())?;
    file.as_file().sync_all()?;
    match file.persist_noclobber(path) {
        Ok(_) => Ok(key),
        Err(err) if err.error.kind() == ErrorKind::AlreadyExists => load(path),
        Err(err) => Err(err.error).context("save device key"),
    }
}
