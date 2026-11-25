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

#[derive(Default, Deserialize)]
pub struct LocalSettings {
    pub device_name: Option<String>,
    #[serde(default)]
    pub native: crate::Native,
    #[serde(default)]
    pub gateway: crate::startup::GatewaySettings,
    #[serde(default)]
    pub gui: GuiSettings,
}

pub fn read_local(path: &Path) -> Result<LocalSettings> {
    match fs::read_to_string(path) {
        Ok(text) => {
            let settings: LocalSettings = toml::from_str(&text)?;
            settings.gui.validate().map_err(anyhow::Error::msg)?;
            Ok(settings)
        }
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(LocalSettings::default()),
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
    let (base, previous) = (dirs::data_local_dir()?, ["PeerHatch", "P2PTunnel"]);
    #[cfg(not(windows))]
    let (base, previous) = (dirs::config_dir()?, ["peerhatch", "p2p-tunnel"]);
    let path = base.join("paxri").join("node.toml");
    // Existing nodes keep the configuration and device key in the same directory.
    if !path.exists() {
        for name in previous {
            let legacy = base.join(name).join("node.toml");
            if legacy.is_file() {
                return Some(legacy);
            }
        }
    }
    Some(path)
}

// This hint can survive shutdown. Clients verify the live endpoint's configuration
// before using it, including after a crash or a move to another installation.
pub fn gateway_address(config: &Path) -> Option<PathBuf> {
    let bytes = fs::read(config.with_extension("gateway-ipc")).ok()?;
    serde_json::from_slice(&bytes).ok()
}

pub fn record_gateway_address(config: &Path, ipc: &Path) -> Result<()> {
    let path = config.with_extension("gateway-ipc");
    let mut temp = NamedTempFile::new_in(path.parent().context("missing profile directory")?)?;
    #[cfg(unix)]
    let ipc = std::path::absolute(ipc)?;
    serde_json::to_writer(temp.as_file_mut(), &ipc)?;
    temp.persist(path)?;
    Ok(())
}

pub fn init(path: &Path) -> Result<()> {
    edit(path, true, |current| Ok(current.to_owned()))?;
    Ok(())
}

pub fn edit_device_name(doc: &mut DocumentMut, name: Option<&str>) {
    if let Some(name) = name {
        let mut value = toml_edit::Value::from(name);
        if let Some(old) = doc.get("device_name").and_then(Item::as_value) {
            *value.decor_mut() = old.decor().clone();
        }
        doc.insert("device_name", Item::Value(value));
    } else {
        doc.remove("device_name");
    }
}

pub fn set_device_name(path: &Path, name: Option<&str>) -> Result<()> {
    edit(path, true, |current| {
        let mut doc: DocumentMut = current.parse()?;
        edit_device_name(&mut doc, name);
        let text = doc.to_string();
        crate::parse(&text).map_err(anyhow::Error::msg)?;
        Ok(text)
    })?;
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
        if let Some(controls) = gui.get_mut("controls").and_then(Item::as_table_like_mut) {
            controls.remove("copy_selection_on_ctrl_c");
        }
        let ser = ValueSerializer::new();
        let (table, key, value) = match change {
            GuiChange::Shortcut(action, shortcut) => (
                subtable(subtable(gui, "controls")?, "shortcuts")?,
                action.key(),
                shortcut.map(|chord| chord.serialize(ser)).transpose()?,
            ),
            GuiChange::RemoteClipboard(enabled) => (
                subtable(gui, "controls")?,
                "remote_clipboard",
                Some(enabled.serialize(ser)?),
            ),
            GuiChange::ClickableLinks(enabled) => (
                subtable(gui, "controls")?,
                "clickable_links",
                Some(enabled.serialize(ser)?),
            ),
            GuiChange::TerminalBell(enabled) => (
                subtable(gui, "controls")?,
                "terminal_bell",
                Some(enabled.serialize(ser)?),
            ),
            GuiChange::Keybar(v) => {
                let keys = subtable(gui, "keybar")?;
                for (name, mut value) in [
                    ("row_height", v.row_height.serialize(ValueSerializer::new())?),
                    ("haptics", v.haptics.serialize(ValueSerializer::new())?),
                ] {
                    if let Some(old) = keys.get(name).and_then(Item::as_value) {
                        *value.decor_mut() = old.decor().clone();
                    }
                    keys.insert(name, Item::Value(value));
                }
                let mut rows = toml_edit::Array::new();
                for row in v.rows {
                    let mut value = row.serialize(ValueSerializer::new())?;
                    value.decor_mut().set_prefix("\n  ");
                    rows.push_formatted(value);
                }
                if !rows.is_empty() {
                    rows.set_trailing_comma(true);
                    rows.set_trailing("\n");
                }
                (keys, "rows", Some(toml_edit::Value::Array(rows)))
            }
            GuiChange::Appearance(v) => (gui, "appearance", Some(v.serialize(ser)?)),
            GuiChange::TabPlacement(v) => (gui, "tab_placement", Some(v.serialize(ser)?)),
            GuiChange::Scheme(v) => (subtable(gui, "terminal")?, "scheme", Some(v.serialize(ser)?)),
            GuiChange::CursorShape(v) => (
                subtable(gui, "terminal")?,
                "cursor_shape",
                Some(v.serialize(ser)?),
            ),
            GuiChange::FontFamily(v) => (subtable(gui, "terminal")?, "font_family", Some(v.serialize(ser)?)),
            GuiChange::FontSize(v) => (subtable(gui, "terminal")?, "font_size", v.map(|v| v.serialize(ser)).transpose()?),
            GuiChange::LineHeight(v) => (subtable(gui, "terminal")?, "line_height", v.map(|v| v.serialize(ser)).transpose()?),
            GuiChange::LetterSpacing(v) => (subtable(gui, "terminal")?, "letter_spacing", Some(v.serialize(ser)?)),
            GuiChange::BlinkCursor(v) => (
                subtable(gui, "terminal")?,
                "blink_cursor",
                Some(v.serialize(ser)?),
            ),
        };
        if let Some(mut value) = value {
            if let Some(old) = table.get(key).and_then(Item::as_value) {
                *value.decor_mut() = old.decor().clone();
            }
            table.insert(key, Item::Value(value));
        } else {
            table.remove(key);
        }
        let text = doc.to_string();
        crate::parse(&text).map_err(anyhow::Error::msg)?;
        Ok(text)
    })?;
    Ok(toml::from_str::<LocalSettings>(&text)?.gui)
}

pub fn set_native(path: &Path, change: crate::NativeChange, create: bool) -> Result<crate::Native> {
    use crate::NativeChange;
    let text = edit(path, create, |current| {
        let mut doc: DocumentMut = current.parse()?;
        let native = subtable(doc.as_table_mut(), "native")?;
        let (key, value) = match change {
            NativeChange::Upnp(enabled) => ("upnp", Some(toml_edit::Value::from(enabled))),
            NativeChange::LanPortMapping(enabled) => {
                ("lan_port_mapping", Some(toml_edit::Value::from(enabled)))
            }
            NativeChange::RelayUrl(url) => ("relay_url", url.map(toml_edit::Value::from)),
        };
        if let Some(mut value) = value {
            if let Some(old) = native.get(key).and_then(Item::as_value) {
                *value.decor_mut() = old.decor().clone();
            }
            native.insert(key, Item::Value(value));
        } else {
            native.remove(key);
        }
        let text = doc.to_string();
        crate::parse(&text).map_err(anyhow::Error::msg)?;
        Ok(text)
    })?;
    Ok(toml::from_str::<LocalSettings>(&text)?.native)
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
        let mut next: DocumentMut = proposed.parse()?;
        for key in ["gui", "gateway", "native"] {
            old.remove(key);
            next.remove(key);
            if let Some(settings) = disk.remove(key) {
                next.insert(key, settings);
            }
        }
        ensure!(
            old.to_string() == disk.to_string(),
            "Configuration changed on disk. Restart the gateway before saving."
        );
        let text = next.to_string();
        crate::parse(&text).map_err(anyhow::Error::msg)?;
        Ok(text)
    })
}

#[cfg(test)]
mod network_tests {
    use super::*;
    use crate::NativeChange;

    #[test]
    fn offline_device_name_round_trips_without_rewriting_other_settings() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("node.toml");
        assert_eq!(read_local(&path)?.device_name, None);
        set_device_name(&path, Some("First name"))?;
        assert_eq!(read_local(&path)?.device_name.as_deref(), Some("First name"));

        let other_settings = "version = 1\n[gateway]\nstartup = 'manual' # keep this\n";
        fs::write(&path, format!("device_name = 'Old' # display name\n{other_settings}"))?;
        set_device_name(&path, Some("工作站"))?;
        let saved = fs::read_to_string(&path)?;
        assert!(saved.contains("# display name"));
        assert_eq!(crate::parse(&saved).unwrap().device_name.as_deref(), Some("工作站"));
        assert!(set_device_name(&path, Some("two\nlines")).is_err());
        assert_eq!(fs::read_to_string(&path)?, saved);
        set_device_name(&path, None)?;
        assert_eq!(fs::read_to_string(&path)?, other_settings);
        Ok(())
    }

    #[test]
    fn native_edits_survive_gateway_writes_and_invalid_relay_input() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("node.toml");
        let initial = "version = 1\n[native]\nlan_port_mapping = true # keep this\n";
        fs::write(&path, initial)?;
        assert!(read_local(&path)?.native.upnp);
        set_native(&path, NativeChange::Upnp(false), false)?;
        let url = "https://relay.example.org/";
        set_native(&path, NativeChange::RelayUrl(Some(url.into())), false)?;
        save_gateway(&path, initial, format!("device_name = 'test'\n{initial}"))?;
        let before = fs::read_to_string(&path)?;
        assert!(before.contains("# keep this"));
        assert!(
            set_native(
                &path,
                NativeChange::RelayUrl(Some("ftp://invalid".into())),
                false
            )
            .is_err()
        );
        assert_eq!(fs::read_to_string(&path)?, before);
        let native = crate::parse(&before).unwrap().native;
        assert!(!native.upnp && native.lan_port_mapping);
        assert_eq!(native.relay_url.unwrap().as_str(), url);
        assert!(
            set_native(&path, NativeChange::RelayUrl(None), false)?
                .relay_url
                .is_none()
        );
        Ok(())
    }
}
