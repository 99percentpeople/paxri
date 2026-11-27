use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
};

use anyhow::{Context, Result, ensure};
use paxri_config::file::subtable;
use paxri_config::{Config, DevicePlatform, Direction};
use paxri_control::Command;
use toml_edit::{DocumentMut, Table, TableLike, value};

pub(crate) fn valid_forward_name(name: &str) -> bool {
    !name.trim().is_empty() && name.len() <= 128 && !name.chars().any(char::is_control)
}

pub(super) enum PeerUpdate<'a> {
    Address,
    Pair(Option<&'a str>),
}

fn clear_pending(doc: &mut DocumentMut, id: &str) {
    if let Some(table) = doc
        .get_mut("pending_unpairs")
        .and_then(|v| v.as_table_like_mut())
    {
        table.remove(id);
        if table.is_empty() {
            doc.remove("pending_unpairs");
        }
    }
}

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

fn checked_document(doc: DocumentMut) -> Result<(String, Config)> {
    let text = doc.to_string();
    let config = paxri_config::parse(&text).map_err(anyhow::Error::msg)?;
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
        change: PeerUpdate<'_>,
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
        } else {
            peer.insert(
                "last_status_change_ms",
                value(paxri_core::connection::unix_time_ms()),
            );
        }
        peer.insert("endpoint_id", value(id));
        peer.insert("ticket", value(ticket));
        if let Some(platform) = platform {
            if platform != DevicePlatform::Unknown || !peer.contains_key("platform") {
                peer.insert("platform", value(platform.as_str()));
            }
        }
        if let PeerUpdate::Pair(generation) = change {
            match generation {
                Some(id) => {
                    peer.insert("pairing_id", value(id));
                }
                None => {
                    peer.remove("pairing_id");
                }
            }
            clear_pending(&mut doc, &id.to_ascii_lowercase());
        }
        checked_document(doc)
    }

    pub fn prepare_unpair(
        &self,
        aliases: &BTreeSet<&str>,
        forwards: &[String],
        pending: Option<(&str, &paxri_config::PendingUnpair)>,
    ) -> Result<(String, Config)> {
        let mut doc: DocumentMut = self.text.parse()?;
        let peers = subtable(doc.as_table_mut(), "peers")?;
        for alias in aliases {
            ensure!(
                peers.remove(alias).is_some(),
                "Unknown paired device {alias}"
            );
        }
        if !forwards.is_empty() {
            let rules = subtable(doc.as_table_mut(), "forwards")?;
            for name in forwards {
                rules.remove(name);
            }
        }
        if let Some((id, notice)) = pending {
            let queue = subtable(doc.as_table_mut(), "pending_unpairs")?;
            let entry = subtable(queue, id)?;
            entry.insert("request_id", value(&notice.request_id));
            entry.insert("ticket", value(&notice.ticket));
            entry.insert("name", value(&notice.name));
            if let Some(generation) = &notice.pairing_id {
                entry.insert("pairing_id", value(generation));
            } else {
                entry.remove("pairing_id");
            }
        }
        checked_document(doc)
    }

    pub fn confirm_unpair(&self, id: &str) -> Result<(String, Config)> {
        let mut doc: DocumentMut = self.text.parse()?;
        clear_pending(&mut doc, id);
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
            Command::SetDeviceName { name } => paxri_config::file::edit_device_name(&mut doc, name.as_deref()),
            Command::SaveForward {
                id,
                edit,
                rule: forward,
                ..
            } => {
                ensure!(valid_forward_name(id), "Enter a valid rule ID");
                if let Some(name) = &forward.name {
                    ensure!(valid_forward_name(name), "Enter a valid rule name");
                }
                if !doc.contains_key("forwards") {
                    doc["forwards"] = Table::new().into();
                }
                let forwards = doc["forwards"]
                    .as_table_like_mut()
                    .context("invalid forwards table")?;
                if *edit {
                    ensure!(
                        forwards.contains_key(id),
                        "Forward {id} no longer exists"
                    );
                } else {
                    ensure!(
                        !forwards.contains_key(id),
                        "Forward {id} already exists"
                    );
                    forwards.insert(id, Table::new().into());
                }
                let rule = forwards
                    .get_mut(id)
                    .unwrap()
                    .as_table_like_mut()
                    .context("invalid forward rule")?;
                if let Some(name) = &forward.name {
                    rule.insert("name", value(name));
                } else {
                    rule.remove("name");
                }
                rule.insert("listen", value(forward.listen.to_string()));
                rule.insert("peer", value(&forward.peer));
                if forward.direction == Direction::Remote {
                    rule.insert("direction", value("remote"));
                } else {
                    rule.remove("direction");
                }
                rule.insert("target", value(&forward.target));
            }
            Command::RemoveForward { id, .. } => {
                ensure!(
                    doc.get_mut("forwards")
                        .and_then(|v| v.as_table_like_mut())
                        .and_then(|v| v.remove(id))
                        .is_some(),
                    "Unknown forward {id}"
                );
            }
            _ => anyhow::bail!("Expected a settings change"),
        }
        let (text, config) = checked_document(doc)?;
        if matches!(change, Command::SaveForward { edit: false, .. }) {
            let max_forwards = config.gateway.limits.max_forwards;
            ensure!(
                config.forwards.len() <= max_forwards,
                "Forwarding rule limit reached ({max_forwards}). Remove an existing rule first."
            );
        }
        Ok((text, config))
    }

    #[cfg(test)]
    pub fn persist(&mut self, text: String) -> Result<()> {
        self.text = paxri_config::file::save_gateway(&self.path, &self.text, text)?;
        Ok(())
    }
}

impl super::Gateway {
    pub(super) fn peer_change_times(&self) -> BTreeMap<String, i64> {
        let live = self.peer_changes.borrow();
        self.config
            .peers
            .iter()
            .filter_map(|(name, peer)| {
                let time = live
                    .get(&peer.endpoint_id.to_ascii_lowercase())
                    .copied()
                    .or(peer.last_status_change_ms)?;
                Some((name.clone(), time))
            })
            .collect()
    }

    pub(super) async fn persist_peer_info(gateway: &tokio::sync::Mutex<Self>) -> Result<()> {
        Self::update_config(
            gateway,
            |g| {
                let mut changed = g
                    .config
                    .peers
                    .iter()
                    .filter_map(|(name, peer)| {
                        let info = g.device_info_for(peer)?;
                        let platform = Some(info.platform)
                            .filter(|p| *p != DevicePlatform::Unknown && Some(*p) != peer.platform);
                        let device_name = info
                            .device_name
                            .filter(|n| Some(n) != peer.device_name.as_ref());
                        (platform.is_some() || device_name.is_some())
                            .then_some((name, platform, device_name))
                    })
                    .peekable();
                if changed.peek().is_none() {
                    return Ok(None);
                }
                let mut doc: DocumentMut = g.file.text.parse()?;
                for (name, platform, device_name) in changed {
                    let table = peer_table(&mut doc, name)?;
                    if let Some(platform) = platform {
                        table.insert("platform", value(platform.as_str()));
                    }
                    if let Some(device_name) = device_name {
                        table.insert("device_name", value(device_name));
                    }
                }
                let (text, config) = checked_document(doc)?;
                Ok(Some((text, config, ())))
            },
            |_, ()| (),
        )
        .await?;
        Ok(())
    }

    pub(super) async fn persist_peer_changes(gateway: &tokio::sync::Mutex<Self>) -> Result<()> {
        Self::update_config(
            gateway,
            |g| {
                let mut times = g.peer_change_times();
                times.retain(|name, time| g.config.peers[name].last_status_change_ms != Some(*time));
                if times.is_empty() {
                    return Ok(None);
                }
                let mut doc: DocumentMut = g.file.text.parse()?;
                for (name, time) in times {
                    peer_table(&mut doc, &name)?.insert("last_status_change_ms", value(time));
                }
                let (text, config) = checked_document(doc)?;
                Ok(Some((text, config, ())))
            },
            |_, ()| (),
        )
        .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn pairing_with_older_peers_preserves_platform_without_promoting_legacy_grants() -> Result<()> {
        let id = "ab".repeat(32);
        let file = SettingsFile {
            path: PathBuf::new(),
            text: format!(
                "version=1\n[peers.lab]\nendpoint_id='{id}'\nplatform='macos'\nports.connect=['127.0.0.1:8080']\n"
            ),
        };
        for reported in [
            None,
            Some(DevicePlatform::Unknown),
            Some(DevicePlatform::Web),
        ] {
            let (_, cfg) =
                file.prepare_peer("lab", &id, "updated-ticket", reported, PeerUpdate::Address)?;
            let peer = &cfg.peers["lab"];
            let expected = if reported == Some(DevicePlatform::Web) {
                DevicePlatform::Web
            } else {
                DevicePlatform::MacOs
            };
            assert_eq!(peer.platform, Some(expected));
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
            id: "web-ui".into(),
            expected_peer: None,
            edit: false,
            rule: paxri_config::Forward {
                name: Some("Web UI".into()),
                direction: Direction::Local,
                listen: "[::1]:0".parse()?,
                peer: "lab".into(),
                target: "localhost:8080".into(),
            },
        };
        let (text, cfg) = file.prepare(&rule)?;
        assert_eq!(cfg.forwards["web-ui"].peer, "lab");
        assert_eq!(cfg.forwards["web-ui"].name.as_deref(), Some("Web UI"));
        assert!(text.contains("# paired manually"));
        file.persist(text)?;
        assert!(file.prepare(&rule).is_err());
        if let Command::SaveForward { edit, rule, .. } = &mut rule {
            *edit = true;
            rule.name = Some("SSH".into());
            rule.target = "22".into();
        }
        let (text, cfg) = file.prepare(&rule)?;
        assert_eq!(cfg.forwards["web-ui"].name.as_deref(), Some("SSH"));
        assert_eq!(cfg.forwards["web-ui"].target, "127.0.0.1:22");
        file.persist(text)?;
        for name in [Some("Lab PC".to_owned()), None] {
            let (text, cfg) = file.prepare(&Command::SetDeviceName { name: name.clone() })?;
            assert_eq!(cfg.device_name, name);
            file.persist(text)?;
        }
        let (text, cfg) = file.prepare(&Command::RemoveForward {
            id: "web-ui".into(),
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

    #[test]
    fn save_forward_enforces_max_forwards_only_on_creation() -> Result<()> {
        let id = "ab".repeat(32);
        let mut file = SettingsFile {
            path: PathBuf::new(),
            text: format!(
                "version=1\n[gateway.limits]\nmax_forwards=1\n[peers.lab]\nendpoint_id='{id}'\n"
            ),
        };
        let (text, _) = file.prepare(&Command::SaveForward {
            id: "r1".into(),
            expected_peer: None,
            edit: false,
            rule: paxri_config::Forward {
                name: None,
                direction: Direction::Local,
                listen: "127.0.0.1:0".parse()?,
                peer: "lab".into(),
                target: "127.0.0.1:8080".into(),
            },
        })?;
        file.text = text;
        let overflow = file.prepare(&Command::SaveForward {
            id: "r2".into(),
            expected_peer: None,
            edit: false,
            rule: paxri_config::Forward {
                name: None,
                direction: Direction::Local,
                listen: "127.0.0.1:0".parse()?,
                peer: "lab".into(),
                target: "127.0.0.1:8081".into(),
            },
        });
        assert!(overflow.is_err());
        assert!(
            file.prepare(&Command::SaveForward {
                id: "r1".into(),
                expected_peer: None,
                edit: true,
                rule: paxri_config::Forward {
                    name: Some("Edited".into()),
                    direction: Direction::Local,
                    listen: "127.0.0.1:0".parse()?,
                    peer: "lab".into(),
                    target: "127.0.0.1:8082".into(),
                },
            })
            .is_ok()
        );
        assert!(
            file.prepare(&Command::RemoveForward {
                id: "r1".into(),
                expected_peer: None,
            })
            .is_ok()
        );
        Ok(())
    }
}
