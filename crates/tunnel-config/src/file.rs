//! Serialized, atomic edits to a node TOML shared by the gateway and its desktop client.
use crate::gui::{GuiChange, GuiSettings};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::{ErrorKind, Write},
    path::{Path, PathBuf},
};
use tempfile::NamedTempFile;
use toml_edit::{DocumentMut, Item, Table, TableLike, ser::ValueSerializer};

#[derive(Deserialize)]
pub struct LocalSettings {
    #[serde(default)]
    pub gateway: crate::startup::GatewaySettings,
    #[serde(default)]
    pub gui: GuiSettings,
}

pub fn read_local(path: &Path) -> Result<LocalSettings> {
    match fs::read_to_string(path) {
        Ok(text) => Ok(toml::from_str(&text)?),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(LocalSettings {
            gui: Default::default(),
            gateway: Default::default(),
        }),
        Err(e) => Err(e.into()),
    }
}

pub fn read_gui(path: &Path) -> Result<GuiSettings> {
    Ok(read_local(path)?.gui)
}

fn edit(path: &Path, create: bool, update: impl FnOnce(&str) -> Result<String>) -> Result<String> {
    let path = match path.canonicalize() {
        Ok(path) => path,
        Err(e) if create && e.kind() == ErrorKind::NotFound => std::path::absolute(path)?,
        Err(e) => return Err(e.into()),
    };
    let parent = path.parent().context("missing configuration directory")?;
    if create {
        fs::create_dir_all(parent)?;
    }
    // The lock must survive replacement of the TOML inode.
    let mut lock_name = path.as_os_str().to_owned();
    lock_name.push(".lock");
    let lock = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(lock_name)?;
    lock.lock()?;
    let (current, missing) = match fs::read_to_string(&path) {
        Ok(text) => (text, false),
        Err(e) if create && e.kind() == ErrorKind::NotFound => {
            (String::from("version = 1\n"), true)
        }
        Err(e) => return Err(e.into()),
    };
    let next = update(&current)?;
    if missing || next != current {
        let mut temp = NamedTempFile::new_in(parent)?;
        temp.write_all(next.as_bytes())?;
        match fs::metadata(&path) {
            Ok(meta) => {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::{MetadataExt, fchown};
                    // A privileged startup-policy edit must leave the user's TOML writable.
                    if temp.as_file().metadata()?.uid() == 0 && meta.uid() != 0 {
                        fchown(temp.as_file(), Some(meta.uid()), Some(meta.gid()))?;
                    }
                }
                temp.as_file().set_permissions(meta.permissions())?;
            }
            Err(err) if create && err.kind() == ErrorKind::NotFound => {}
            Err(err) => return Err(err.into()),
        }
        temp.as_file().sync_all()?;
        temp.persist(&path)
            .with_context(|| format!("save {}", path.display()))?;
    }
    Ok(next)
}

pub fn user_config() -> Option<PathBuf> {
    #[cfg(windows)]
    let dir = dirs::data_local_dir()?.join("P2PTunnel");
    #[cfg(not(windows))]
    let dir = dirs::config_dir()?.join("p2p-tunnel");
    Some(dir.join("node.toml"))
}

pub fn init(path: &Path) -> Result<()> {
    edit(path, true, |current| Ok(current.to_owned()))?;
    Ok(())
}

pub fn startup(path: &Path) -> Result<crate::startup::Startup> {
    Ok(read_local(path)?.gateway.startup)
}

pub fn set_startup(path: &Path, mode: crate::startup::Startup) -> Result<()> {
    edit(path, true, |current| {
        let mut doc: DocumentMut = current.parse()?;
        subtable(doc.as_table_mut(), "gateway")?.insert("startup", toml_edit::value(mode.as_str()));
        let text = doc.to_string();
        crate::parse(&text).map_err(anyhow::Error::msg)?;
        Ok(text)
    })?;
    Ok(())
}

pub fn subtable<'a>(table: &'a mut dyn TableLike, key: &str) -> Result<&'a mut dyn TableLike> {
    if !table.contains_key(key) {
        table.insert(key, Item::Table(Table::new()));
    }
    table
        .get_mut(key)
        .and_then(Item::as_table_like_mut)
        .context("expected a settings table")
}

/// Each edit changes one key and reads its siblings from the latest revision.
pub fn set_gui(path: &Path, change: GuiChange, create: bool) -> Result<GuiSettings> {
    let text = edit(path, create, |current| {
        let mut doc: DocumentMut = current.parse()?;
        let gui = subtable(doc.as_table_mut(), "gui")?;
        let ser = ValueSerializer::new();
        let (table, key, mut value) = match change {
            GuiChange::Appearance(v) => (gui, "appearance", v.serialize(ser)?),
            GuiChange::Scheme(v) => (subtable(gui, "terminal")?, "scheme", v.serialize(ser)?),
            GuiChange::CursorShape(v) => (
                subtable(gui, "terminal")?,
                "cursor_shape",
                v.serialize(ser)?,
            ),
            GuiChange::BlinkCursor(v) => (
                subtable(gui, "terminal")?,
                "blink_cursor",
                v.serialize(ser)?,
            ),
        };
        if let Some(old) = table.get(key).and_then(Item::as_value) {
            *value.decor_mut() = old.decor().clone();
        }
        table.insert(key, Item::Value(value));
        let text = doc.to_string();
        crate::parse(&text).map_err(anyhow::Error::msg)?;
        Ok(text)
    })?;
    Ok(toml::from_str::<LocalSettings>(&text)?.gui)
}

/// Preserve concurrent desktop and startup-policy edits while retaining the gateway's existing
/// conflict check for manually edited peers, rules, comments, and other settings.
pub fn save_gateway(path: &Path, expected: &str, proposed: String) -> Result<String> {
    edit(path, false, |current| {
        if current == expected {
            return Ok(proposed);
        }
        let mut old: DocumentMut = expected.parse()?;
        let mut disk: DocumentMut = current.parse()?;
        old.remove("gui");
        let gui = disk.remove("gui");
        old.remove("gateway");
        let settings = disk.remove("gateway");
        ensure!(
            old.to_string() == disk.to_string(),
            "Configuration changed on disk. Restart the gateway before saving."
        );
        let mut next: DocumentMut = proposed.parse()?;
        next.remove("gui");
        if let Some(gui) = gui {
            next.insert("gui", gui);
        }
        next.remove("gateway");
        if let Some(settings) = settings {
            next.insert("gateway", settings);
        }
        let text = next.to_string();
        crate::parse(&text).map_err(anyhow::Error::msg)?;
        Ok(text)
    })
}
