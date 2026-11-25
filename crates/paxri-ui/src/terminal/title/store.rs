use super::TabTitle;
use paxri_control::terminal::managed::Key;
use std::{
    collections::BTreeMap,
    fs, io,
    path::{Path, PathBuf},
};

type Entries = BTreeMap<String, Vec<(Key, TabTitle)>>;

/// Titles belong to gateway sessions, including sessions with no attached view.
#[derive(Default)]
pub struct SessionTitles {
    path: Option<PathBuf>,
    entries: Entries,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_titles_survive_reopening_until_the_same_gateway_session_ends() {
        let dir = tempfile::tempdir().unwrap();
        let path = Some(dir.path().join("session-titles.json"));
        let open = || SessionTitles::open(path.clone());
        let peer = "owner/remote";
        let key = Key {
            number: 1,
            nonce: 42,
        };
        let next = Key {
            number: 1,
            nonce: 43,
        };
        let mut a = open();
        let mut b = open();
        let mut title = TabTitle::default();
        title.set_pty("shell".into());
        title.rename("构建");
        title.set_pty("editor".into());
        a.set(peer, key, &title).unwrap();
        b.set("other/remote", key, &TabTitle::default()).unwrap();
        let mut c = open();
        let mut restored = c.get(peer, key).unwrap().clone();
        assert_eq!(restored.label(&"device".into()).as_ref(), "构建");
        restored.rename("");
        assert_eq!(restored.label(&"device".into()).as_ref(), "editor");
        c.set(peer, key, &restored).unwrap();
        assert_eq!(open().get(peer, key), Some(&restored));
        a.retain(peer, |k| k == next).unwrap();
        let d = open();
        assert!(d.get(peer, key).is_none());
        assert!(d.get(peer, next).is_none());
        assert!(d.get("other/remote", key).is_some());
    }
}

impl SessionTitles {
    pub fn open(path: Option<PathBuf>) -> Self {
        let entries = path
            .as_deref()
            .map(Self::read)
            .transpose()
            .unwrap_or_else(|e| {
                eprintln!("Loading session titles: {e}");
                None
            })
            .unwrap_or_default();
        Self { path, entries }
    }

    fn read(path: &Path) -> io::Result<Entries> {
        match fs::File::open(path) {
            Ok(file) => Ok(serde_json::from_reader(file)?),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Entries::new()),
            Err(e) => Err(e),
        }
    }

    pub fn entries(&self, peer: &str) -> &[(Key, TabTitle)] {
        self.entries
            .get(peer)
            .map(Vec::as_slice)
            .unwrap_or_default()
    }

    pub fn get(&self, peer: &str, key: Key) -> Option<&TabTitle> {
        self.entries(peer)
            .iter()
            .find(|(k, _)| *k == key)
            .map(|(_, t)| t)
    }

    pub fn reserved_labels(
        &self,
        peer: &str,
        fallback: &gpui::SharedString,
        include: impl Fn(Key) -> bool,
    ) -> Vec<gpui::SharedString> {
        self.entries(peer)
            .iter()
            .filter(|(key, _)| include(*key))
            .map(|(_, title)| title.label(fallback))
            .collect()
    }

    pub fn set(&mut self, peer: &str, key: Key, title: &TabTitle) -> io::Result<()> {
        if self.get(peer, key) == Some(title) {
            return Ok(());
        }
        self.edit(peer, |list| {
            if let Some((_, stored)) = list.iter_mut().find(|(k, _)| *k == key) {
                stored.clone_from(title);
            } else {
                list.push((key, title.clone()));
            }
        })
    }

    pub fn retain(&mut self, peer: &str, keep: impl Fn(Key) -> bool) -> io::Result<()> {
        let removed: Vec<_> = self
            .entries(peer)
            .iter()
            .map(|(k, _)| *k)
            .filter(|k| !keep(*k))
            .collect();
        if removed.is_empty() {
            return Ok(());
        }
        self.edit(peer, |list| list.retain(|(k, _)| !removed.contains(k)))
    }

    fn edit(
        &mut self,
        peer: &str,
        change: impl FnOnce(&mut Vec<(Key, TabTitle)>),
    ) -> io::Result<()> {
        let edit = |entries: &mut Entries| {
            let list = entries.entry(peer.to_owned()).or_default();
            change(list);
            if list.is_empty() {
                entries.remove(peer);
            }
        };
        let Some(path) = &self.path else {
            edit(&mut self.entries);
            return Ok(());
        };
        let dir = path
            .parent()
            .ok_or_else(|| io::Error::other("Missing session title directory"))?;
        fs::create_dir_all(dir)?;
        // Merge with other GUI processes before replacing the file.
        let lock = fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(path.with_extension("lock"))?;
        lock.lock()?;
        let mut entries = Self::read(path)?;
        edit(&mut entries);
        let mut temp = tempfile::NamedTempFile::new_in(dir)?;
        serde_json::to_writer(temp.as_file_mut(), &entries)?;
        temp.persist(path)?;
        self.entries = entries;
        Ok(())
    }
}
