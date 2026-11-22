use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
};

use anyhow::{Context, Result, ensure};
use peerhatch_config::file::subtable;
use peerhatch_config::{Config, Destination, DevicePlatform, Direction};
use peerhatch_control::Command;
use toml_edit::{Array, DocumentMut, Table, TableLike, value};

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
    let config = peerhatch_config::parse(&text).map_err(anyhow::Error::msg)?;
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
                value(peerhatch_core::connection::unix_time_ms()),
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
        pending: Option<(&str, &peerhatch_config::PendingUnpair)>,
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
        if let Some(services) = doc.get_mut("services").and_then(|s| s.as_table_like_mut()) {
            for (_, service) in services.iter_mut() {
                if let Some(grants) = service
                    .get_mut("allow_peers")
                    .and_then(|p| p.as_array_mut())
                {
                    grants.retain(|v| !v.as_str().is_some_and(|name| aliases.contains(name)));
                }
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
            Command::SetDeviceName { name } => match name {
                Some(name) => doc["device_name"] = value(name.as_str()),
                None => {
                    doc.remove("device_name");
                }
            },
            Command::SetGatewayPortAccess { ports, .. } => {
                if ports.connect.is_empty() && ports.listen.is_empty() {
                    doc.remove("port_access");
                } else {
                    let access = subtable(doc.as_table_mut(), "port_access")?;
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
            Command::SaveService {
                name,
                expected,
                rename_to,
                service,
            } => {
                let destination = rename_to.as_ref().unwrap_or(name);
                ensure!(
                    !destination.trim().is_empty() && !destination.chars().any(char::is_control),
                    "Enter a single-line service name"
                );
                let services = subtable(doc.as_table_mut(), "services")?;
                if expected.is_none() || destination != name {
                    ensure!(
                        !services.contains_key(destination),
                        "Service {destination} already exists"
                    );
                }
                if destination != name {
                    ensure!(
                        expected.is_some(),
                        "Only an existing service can be renamed"
                    );
                    let entry = services
                        .remove(name)
                        .with_context(|| format!("Unknown service {name}"))?;
                    services.insert(destination, entry);
                }
                let entry = subtable(services, destination)?;
                entry.insert("target", value(&service.target));
                entry.insert(
                    "allow_peers",
                    value(
                        service
                            .allow_peers
                            .iter()
                            .map(String::as_str)
                            .collect::<Array>(),
                    ),
                );
            }
            Command::RemoveService { name, .. } => {
                let services = subtable(doc.as_table_mut(), "services")?;
                ensure!(services.remove(name).is_some(), "Unknown service {name}");
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
        self.text = peerhatch_config::file::save_gateway(&self.path, &self.text, text)?;
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

    pub(super) fn persist_peer_changes(&mut self) -> Result<()> {
        let mut times = self.peer_change_times();
        times.retain(|name, time| self.config.peers[name].last_status_change_ms != Some(*time));
        if times.is_empty() {
            return Ok(());
        }
        let mut doc: DocumentMut = self.file.text.parse()?;
        for (name, time) in times {
            peer_table(&mut doc, &name)?.insert("last_status_change_ms", value(time));
        }
        let (text, config) = checked_document(doc)?;
        self.file.persist(text)?;
        self.apply_config(config);
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
            assert!(cfg.port_access.connect.is_empty());
        }
        Ok(())
    }

    #[test]
    fn published_service_edits_preserve_other_rules_and_validate_selected_peers() -> Result<()> {
        use peerhatch_control::Service;

        let dir = tempfile::tempdir()?;
        let path = dir.path().join("test.toml");
        let original = format!(
            "# custom configuration\nversion=1\nservices={{ other = {{ target='localhost:3000', allow_peers=[] }} }}\n[peers.lab]\nendpoint_id='{}'\n[forwards.web]\npeer='lab'\nlisten='127.0.0.1:0'\nservice='other'\n",
            "ab".repeat(32)
        );
        fs::write(&path, &original)?;
        let mut file = SettingsFile {
            path: path.clone(),
            text: original,
        };
        let mut service = Service {
            target: "localhost:8000".into(),
            allow_peers: vec![],
        };
        let create = |service: &Service| Command::SaveService {
            name: "dev.web".into(),
            rename_to: None,
            expected: None,
            service: service.clone(),
        };
        let (text, cfg) = file.prepare(&create(&service))?;
        assert_eq!(cfg.services["dev.web"], service);
        file.persist(text)?;
        assert!(file.prepare(&create(&service)).is_err());
        let initial = service.clone();
        let edit = |to: &str, service: &Service| Command::SaveService {
            name: "dev.web".into(),
            rename_to: Some(to.into()),
            expected: Some(initial.clone()),
            service: service.clone(),
        };
        service.allow_peers.push("missing".into());
        assert!(file.prepare(&edit("renamed", &service)).is_err());
        service.allow_peers = vec!["lab".into()];
        service.target = "[::1]:8080".into();
        assert!(file.prepare(&edit("other", &service)).is_err());
        let (text, cfg) = file.prepare(&edit("renamed", &service))?;
        assert!(!cfg.services.contains_key("dev.web"));
        assert_eq!(cfg.services["renamed"], service);
        file.persist(text)?;
        let (text, _) = file.prepare(&Command::RemoveService {
            name: "renamed".into(),
            expected: service,
        })?;
        file.persist(text)?;
        let saved = fs::read_to_string(path)?;
        assert!(saved.contains("# custom configuration"));
        let cfg = peerhatch_config::parse(&saved).map_err(anyhow::Error::msg)?;
        assert_eq!(cfg.services.keys().collect::<Vec<_>>(), ["other"]);
        assert_eq!(cfg.forwards["web"].destination.address(), "other");
        assert!(cfg.port_access.connect.is_empty());
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
            rule: peerhatch_config::Forward {
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
