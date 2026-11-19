use std::{fs, io::Write, path::PathBuf};

use anyhow::{Context, Result, ensure};
use tempfile::NamedTempFile;
use toml_edit::{Array, DocumentMut, Table, TableLike, value};
use tunnel_config::{Config, Destination, DevicePlatform, Direction};
use tunnel_control::Command;

pub struct SettingsFile {
    pub path: PathBuf,
    pub text: String,
}

fn peer_table<'a>(doc: &'a mut DocumentMut, name: &str) -> Result<&'a mut dyn TableLike> {
    doc.get_mut("peers")
        .and_then(|p| p.get_mut(name))
        .and_then(|p| p.as_table_like_mut())
        .with_context(|| format!("Unknown peer {name}"))
}

fn subtable<'a>(table: &'a mut dyn TableLike, key: &str) -> Result<&'a mut dyn TableLike> {
    if !table.contains_key(key) {
        table.insert(key, Table::new().into());
    }
    table
        .get_mut(key)
        .and_then(|t| t.as_table_like_mut())
        .context("Expected a settings table")
}

fn checked_document(doc: DocumentMut) -> Result<(String, Config)> {
    let text = doc.to_string();
    let config = tunnel_config::parse(&text).map_err(anyhow::Error::msg)?;
    Ok((text, config))
}

impl SettingsFile {
    pub fn prepare_pairing_code(&self, code: &str) -> Result<(String, Config)> {
        let mut doc: DocumentMut = self.text.parse()?;
        doc["pairing_code"] = value(code);
        checked_document(doc)
    }

    pub fn prepare_peer(
        &self,
        alias: &str,
        id: &str,
        ticket: &str,
        platform: Option<DevicePlatform>,
    ) -> Result<(String, Config)> {
        let mut doc: DocumentMut = self.text.parse()?;
        let peers = subtable(doc.as_table_mut(), "peers")?;
        let peer = subtable(peers, alias)?;
        if let Some(existing) = peer.get("endpoint_id") {
            ensure!(
                existing
                    .as_str()
                    .is_some_and(|v| v.eq_ignore_ascii_case(id)),
                "Device name already belongs to a different identity"
            );
        }
        peer.insert("endpoint_id", value(id));
        peer.insert("ticket", value(ticket));
        if let Some(platform) = platform {
            if platform != DevicePlatform::Unknown || !peer.contains_key("platform") {
                peer.insert("platform", value(platform.as_str()));
            }
        }
        checked_document(doc)
    }

    pub fn prepare_peer_platform(
        &self,
        alias: &str,
        platform: DevicePlatform,
    ) -> Result<(String, Config)> {
        let mut doc: DocumentMut = self.text.parse()?;
        peer_table(&mut doc, alias)?.insert("platform", value(platform.as_str()));
        checked_document(doc)
    }

    /// Edit only the affected fields so comments, aliases and custom settings survive.
    pub fn prepare(&self, change: &Command) -> Result<(String, Config)> {
        let mut doc: DocumentMut = self.text.parse()?;
        match change {
            Command::SetDeviceName { name } => match name {
                Some(name) => doc["device_name"] = value(name.as_str()),
                None => {
                    doc.remove("device_name");
                }
            },
            Command::SetTerminalAccess { peer, allowed } => {
                let table = peer_table(&mut doc, peer)?;
                if *allowed {
                    table.insert("allow_terminal", value(true));
                } else {
                    table.remove("allow_terminal");
                }
            }
            Command::SetPortAccess { peer, ports } => {
                let peer = peer_table(&mut doc, peer)?;
                if ports.connect.is_empty() && ports.listen.is_empty() {
                    peer.remove("ports");
                } else {
                    let access = subtable(peer, "ports")?;
                    access.insert(
                        "connect",
                        value(ports.connect.iter().map(String::as_str).collect::<Array>()),
                    );
                    access.insert(
                        "listen",
                        value(
                            ports
                                .listen
                                .iter()
                                .map(ToString::to_string)
                                .collect::<Array>(),
                        ),
                    );
                }
            }
            Command::SaveForward {
                name,
                edit,
                rule: forward,
                ..
            } => {
                ensure!(
                    !name.trim().is_empty() && !name.chars().any(char::is_control),
                    "Enter a rule name"
                );
                if !doc.contains_key("forwards") {
                    doc["forwards"] = Table::new().into();
                }
                let forwards = doc["forwards"]
                    .as_table_like_mut()
                    .context("invalid forwards table")?;
                if *edit {
                    ensure!(
                        forwards.contains_key(name),
                        "Forward {name} no longer exists"
                    );
                } else {
                    ensure!(
                        !forwards.contains_key(name),
                        "Forward {name} already exists"
                    );
                    forwards.insert(name, Table::new().into());
                }
                let rule = forwards
                    .get_mut(name)
                    .unwrap()
                    .as_table_like_mut()
                    .context("invalid forward rule")?;
                rule.insert("listen", value(forward.listen.to_string()));
                rule.insert("peer", value(&forward.peer));
                if forward.direction == Direction::Remote {
                    rule.insert("direction", value("remote"));
                } else {
                    rule.remove("direction");
                }
                let (key, other) = match forward.destination {
                    Destination::Service(_) => ("service", "target"),
                    Destination::Target(_) => ("target", "service"),
                };
                rule.remove(other);
                rule.insert(key, value(forward.destination.address()));
            }
            Command::RemoveForward { name, .. } => {
                ensure!(
                    doc.get_mut("forwards")
                        .and_then(|v| v.as_table_like_mut())
                        .and_then(|v| v.remove(name))
                        .is_some(),
                    "Unknown forward {name}"
                );
            }
            _ => anyhow::bail!("Expected a settings change"),
        }
        checked_document(doc)
    }

    pub fn persist(&mut self, text: String) -> Result<()> {
        let path = &self.path;
        ensure!(
            fs::read_to_string(path)? == self.text,
            "Configuration changed on disk. Restart the gateway before saving."
        );
        let mut temp =
            NamedTempFile::new_in(path.parent().context("missing configuration directory")?)?;
        temp.write_all(text.as_bytes())?;
        temp.as_file()
            .set_permissions(fs::metadata(path)?.permissions())?;
        temp.as_file().sync_all()?;
        temp.persist(path)
            .with_context(|| format!("save {}", path.display()))?;
        self.text = text;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pairing_with_older_peers_preserves_saved_platform_and_grants() -> Result<()> {
        let id = "ab".repeat(32);
        let file = SettingsFile {
            path: PathBuf::new(),
            text: format!(
                "version=1\n[peers.lab]\nendpoint_id='{id}'\nplatform='macos'\nallow_terminal=true\n"
            ),
        };
        for reported in [
            None,
            Some(DevicePlatform::Unknown),
            Some(DevicePlatform::Web),
        ] {
            let (_, cfg) = file.prepare_peer("lab", &id, "updated-ticket", reported)?;
            let peer = &cfg.peers["lab"];
            let expected = if reported == Some(DevicePlatform::Web) {
                DevicePlatform::Web
            } else {
                DevicePlatform::MacOs
            };
            assert_eq!(peer.platform, Some(expected));
            assert!(peer.allow_terminal);
        }
        Ok(())
    }

    #[test]
    fn edit_native_config_and_preserve_external_changes() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("node.toml");
        let original = format!(
            "# paired manually\nversion=1\nforwards={{}}\n[peers.lab]\nendpoint_id='{}'\n",
            "ab".repeat(32)
        );
        fs::write(&path, &original)?;
        let mut file = SettingsFile {
            path: path.clone(),
            text: original,
        };
        let mut rule = Command::SaveForward {
            name: "Web UI".into(),
            expected_peer: None,
            edit: false,
            rule: tunnel_config::Forward {
                direction: Direction::Local,
                listen: "[::1]:0".parse()?,
                peer: "lab".into(),
                destination: Destination::Service("web".into()),
            },
        };
        let (text, cfg) = file.prepare(&rule)?;
        assert_eq!(cfg.forwards["Web UI"].peer, "lab");
        assert!(text.contains("# paired manually"));
        file.persist(text)?;
        assert!(file.prepare(&rule).is_err());
        if let Command::SaveForward { edit, rule, .. } = &mut rule {
            *edit = true;
            rule.destination = Destination::Service("ssh".into());
        }
        let (text, cfg) = file.prepare(&rule)?;
        assert_eq!(cfg.forwards["Web UI"].destination.address(), "ssh");
        file.persist(text)?;
        for name in [Some("Lab PC".to_owned()), None] {
            let (text, cfg) = file.prepare(&Command::SetDeviceName { name: name.clone() })?;
            assert_eq!(cfg.device_name, name);
            file.persist(text)?;
        }
        let (text, cfg) = file.prepare(&Command::RemoveForward {
            name: "Web UI".into(),
            expected_peer: None,
        })?;
        assert!(cfg.forwards.is_empty());
        fs::write(&path, "# updated externally\nversion=1\n")?;
        assert!(file.persist(text).is_err());
        assert_eq!(
            fs::read_to_string(path)?,
            "# updated externally\nversion=1\n"
        );
        Ok(())
    }
}
