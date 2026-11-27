mod store;
use gpui::{App, Entity, SharedString};
use std::collections::HashSet;
pub use store::SessionTitles;

pub fn next_session_number(
    hosted: impl IntoIterator<Item = paxri_control::terminal::managed::Key>,
    last_number: usize,
) -> usize {
    hosted
        .into_iter()
        .map(|k| k.number)
        .max()
        .unwrap_or(0)
        .max(last_number)
        .saturating_add(1)
}

#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct TabTitle {
    pty: Option<SharedString>,
    custom: Option<SharedString>,
    fallback: Option<SharedString>,
}

impl TabTitle {
    fn explicit(&self) -> Option<&SharedString> {
        self.custom.as_ref().or(self.pty.as_ref())
    }

    pub fn label(&self, peer: &SharedString) -> SharedString {
        self.explicit()
            .or(self.fallback.as_ref())
            .cloned()
            .unwrap_or_else(|| peer.clone())
    }

    pub fn assign_defaults(titles: &[(SharedString, Entity<Self>)], cx: &mut App) {
        Self::assign_with_reserved(titles, Vec::new(), cx);
    }

    pub fn assign_with_reserved(
        titles: &[(SharedString, Entity<Self>)],
        reserved: Vec<SharedString>,
        cx: &mut App,
    ) {
        let changes = missing_defaults(
            titles.iter().map(|(peer, title)| (peer, title.read(cx))),
            reserved,
        );
        for (index, name) in changes {
            titles[index].1.update(cx, |title, cx| {
                title.fallback = Some(name);
                cx.notify();
            });
        }
    }

    /// Keep the latest PTY title available if the user returns to automatic naming.
    pub fn set_pty(&mut self, value: String) -> bool {
        let value = (!value.trim().is_empty()).then(|| SharedString::from(value));
        if self.pty == value {
            return false;
        }
        self.pty = value;
        if self.custom.is_none() {
            self.fallback = None;
        }
        true
    }

    pub fn rename(&mut self, text: &str) -> bool {
        let text = text.trim();
        let value = (!text.is_empty()).then(|| SharedString::from(text.to_owned()));
        if self.custom == value {
            return false;
        }
        self.custom = value;
        self.fallback = None;
        true
    }

    /// Drop a generated default label so it can be re-derived after a peer rename.
    pub fn clear_fallback(&mut self) -> bool {
        self.fallback.take().is_some()
    }
}

fn missing_defaults<'a>(
    titles: impl Iterator<Item = (&'a SharedString, &'a TabTitle)> + Clone,
    reserved: Vec<SharedString>,
) -> Vec<(usize, SharedString)> {
    let mut used: HashSet<_> = titles
        .clone()
        .filter_map(|(_, t)| t.explicit().cloned())
        .chain(reserved)
        .collect();
    // Reserve surviving defaults first so a vacant name never renumbers other tabs.
    let mut pending = Vec::new();
    for (index, (peer, title)) in titles.enumerate() {
        if title.explicit().is_none()
            && !title
                .fallback
                .as_ref()
                .is_some_and(|name| used.insert(name.clone()))
        {
            pending.push((index, peer));
        }
    }
    pending
        .into_iter()
        .map(|(index, peer)| {
            let mut name = peer.clone();
            let mut suffix = 1usize;
            while !used.insert(name.clone()) {
                name = format!("{peer}-{suffix}").into();
                suffix += 1;
            }
            (index, name)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn labels(peer: &SharedString, tabs: &mut [TabTitle]) -> Vec<String> {
        for (index, name) in missing_defaults(tabs.iter().map(|t| (peer, t)), vec![]) {
            tabs[index].fallback = Some(name);
        }
        tabs.iter()
            .map(|title| title.label(peer).to_string())
            .collect()
    }

    #[test]
    fn session_names_follow_the_pty_until_manually_named() {
        let peer = SharedString::from("Desk");
        let mut tabs = [TabTitle::default(), TabTitle::default()];
        assert_eq!(labels(&peer, &mut tabs), ["Desk", "Desk-1"]);
        let title = &mut tabs[1];
        assert!(title.set_pty("build".into()));
        assert!(title.rename("build"));
        assert!(title.set_pty("editor".into()));
        assert!(!title.set_pty("editor".into()));
        assert_eq!(title.label(&peer).as_ref(), "build");
        assert!(title.rename(""));
        assert_eq!(title.label(&peer).as_ref(), "editor");
        assert!(title.set_pty(String::new()));
        assert_eq!(labels(&peer, &mut tabs), ["Desk", "Desk-1"]);
    }

    #[test]
    fn defaults_reuse_visible_name_gaps_and_leave_other_tabs_in_place() {
        let peer = SharedString::from("pi");
        let mut tabs: Vec<_> = (0..3).map(|_| TabTitle::default()).collect();
        assert_eq!(labels(&peer, &mut tabs), ["pi", "pi-1", "pi-2"]);
        tabs.remove(1);
        tabs.push(TabTitle::default());
        assert_eq!(labels(&peer, &mut tabs), ["pi", "pi-2", "pi-1"]);
        tabs[0].rename("build");
        tabs.push(TabTitle::default());
        assert_eq!(labels(&peer, &mut tabs), ["build", "pi-2", "pi-1", "pi"]);
        tabs[1].set_pty("editor".into());
        tabs.push(TabTitle::default());
        assert_eq!(
            labels(&peer, &mut tabs),
            ["build", "editor", "pi-1", "pi", "pi-2"]
        );
        tabs[1].set_pty(String::new());
        assert_eq!(
            labels(&peer, &mut tabs),
            ["build", "pi-3", "pi-1", "pi", "pi-2"]
        );
        tabs[0].rename("pi-1");
        assert_eq!(
            labels(&peer, &mut tabs),
            ["pi-1", "pi-3", "pi-4", "pi", "pi-2"]
        );
    }

    #[test]
    fn peer_rename_refreshes_automatic_defaults_without_overwriting_explicit_titles() {
        let old = SharedString::from("OldPC");
        let mut tabs = [TabTitle::default(), TabTitle::default(), TabTitle::default()];
        assert_eq!(labels(&old, &mut tabs), ["OldPC", "OldPC-1", "OldPC-2"]);
        tabs[1].rename("manual");
        tabs[2].set_pty("vim".into());

        let renamed = SharedString::from("Studio");
        for tab in &mut tabs {
            tab.clear_fallback();
        }
        assert_eq!(labels(&renamed, &mut tabs), ["Studio", "manual", "vim"]);
    }
}
