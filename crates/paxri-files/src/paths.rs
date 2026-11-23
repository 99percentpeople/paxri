use crate::wire::Attributes;
use anyhow::{Context, Result, bail, ensure};
use filetime::FileTime;
use std::{
    fs::Metadata,
    io::ErrorKind,
    path::{Path, PathBuf},
};
use tokio::fs;

pub fn host_path(path: &str) -> Result<PathBuf> {
    let input = Path::new(path);
    if input.is_absolute() {
        return Ok(input.to_owned());
    }
    #[cfg(windows)]
    ensure!(
        !input.has_root()
            && !matches!(
                input.components().next(),
                Some(std::path::Component::Prefix(_))
            ),
        "Use a full drive path or a path relative to the home directory"
    );
    let home = std::env::home_dir().context("The gateway user's home directory is unavailable")?;
    Ok(home.join(input.strip_prefix("~").unwrap_or(input)))
}

pub fn trailing_separator(path: &str) -> bool {
    path.ends_with(std::path::is_separator)
}

pub fn validate_name(name: &str) -> Result<()> {
    // A wire entry is always one literal child, including when crossing platforms.
    ensure!(
        !name.is_empty()
            && name != "."
            && name != ".."
            && !name.contains(['/', '\\', '\0'])
            && !name.chars().any(char::is_control),
        "Invalid file name {name:?}"
    );
    #[cfg(windows)]
    {
        let device = name
            .split('.')
            .next()
            .unwrap_or("")
            .trim_end()
            .to_ascii_uppercase();
        let reserved = matches!(
            device.as_str(),
            "NUL" | "CON" | "PRN" | "AUX" | "CONIN$" | "CONOUT$"
        ) || ["COM", "LPT"].iter().any(|prefix| {
            device.strip_prefix(prefix).is_some_and(|n| {
                matches!(
                    n,
                    "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
                )
            })
        });
        ensure!(
            !reserved
                && !name.ends_with(['.', ' '])
                && !name.contains([':', '*', '?', '"', '<', '>', '|']),
            "Invalid Windows file name {name:?}"
        );
    }
    Ok(())
}

pub fn name(path: &Path) -> Result<&str> {
    path.file_name().and_then(|s| s.to_str()).with_context(|| {
        format!(
            "Expected a named UTF-8 file or directory: {}",
            path.display()
        )
    })
}

pub fn is_link(meta: &Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        meta.file_attributes() & 0x400 != 0 // FILE_ATTRIBUTE_REPARSE_POINT, including junctions
    }
    #[cfg(not(windows))]
    meta.is_symlink()
}

pub async fn metadata(path: &Path) -> Result<Option<Metadata>> {
    match fs::symlink_metadata(path).await {
        Ok(meta) => Ok(Some(meta)),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).with_context(|| format!("Inspect {}", path.display())),
    }
}

pub struct Destination {
    path: PathBuf,
    directory: bool,
}

impl Destination {
    pub async fn new(path: &Path, require_directory: bool) -> Result<Self> {
        let require_directory = require_directory || path.to_str().is_some_and(trailing_separator);
        // Strip a trailing separator for lstat so it cannot hide a file or follow a link.
        let inspect = if require_directory {
            path.components().as_path()
        } else {
            path
        };
        let meta = metadata(inspect).await?;
        if meta.as_ref().is_some_and(is_link) {
            bail!(
                "Destination is a symbolic link or junction: {}",
                path.display()
            );
        }
        let directory = meta.as_ref().is_some_and(Metadata::is_dir);
        if require_directory && !directory {
            if meta.is_none() {
                bail!("Destination directory does not exist: {}", path.display());
            }
            bail!("Destination is not a directory: {}", path.display());
        }
        Ok(Self {
            path: path.to_owned(),
            directory,
        })
    }

    pub fn target(&self, name: &str) -> PathBuf {
        if self.directory {
            self.path.join(name)
        } else {
            self.path.clone()
        }
    }
}

impl Attributes {
    pub fn from_metadata(meta: &Metadata) -> Result<Self> {
        fn stamp(time: FileTime) -> (i64, u32) {
            (time.unix_seconds(), time.nanoseconds())
        }
        #[cfg(unix)]
        let mode = Some(std::os::unix::fs::PermissionsExt::mode(&meta.permissions()) & 0o777);
        #[cfg(not(unix))]
        let mode = None;
        Ok(Self {
            accessed: stamp(FileTime::from_system_time(meta.accessed()?)),
            modified: stamp(FileTime::from_system_time(meta.modified()?)),
            mode,
        })
    }

    pub fn apply(&self, path: &Path) -> Result<()> {
        fn time((secs, nanos): (i64, u32)) -> Result<FileTime> {
            use std::time::{Duration, UNIX_EPOCH};
            ensure!(nanos < 1_000_000_000, "Invalid file timestamp");
            let offset = Duration::from_secs(secs.unsigned_abs());
            let whole = if secs < 0 {
                UNIX_EPOCH.checked_sub(offset)
            } else {
                UNIX_EPOCH.checked_add(offset)
            };
            whole
                .and_then(|t| t.checked_add(Duration::from_nanos(nanos.into())))
                .context("File timestamp is out of range on this platform")?;
            Ok(FileTime::from_unix_time(secs, nanos))
        }
        filetime::set_file_times(path, time(self.accessed)?, time(self.modified)?)?;
        #[cfg(unix)]
        if let Some(mode) = self.mode {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode & 0o777))?;
        }
        Ok(())
    }
}
