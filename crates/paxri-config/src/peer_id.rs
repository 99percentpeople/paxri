//! Local selectors for devices that already have a configured, full public key.
use anyhow::{Result, bail, ensure};
use std::collections::BTreeMap;

const ID_LEN: usize = 64;
const MIN_PREFIX: usize = 4;
const DISPLAY_PREFIX: usize = 8;

fn is_device_id(id: &str) -> bool {
    id.len() == ID_LEN && id.bytes().all(|b| b.is_ascii_hexdigit())
}

/// The default display for this device's own ID.
pub fn default_prefix(id: &str) -> &str {
    if is_device_id(id) {
        &id[..DISPLAY_PREFIX]
    } else {
        id
    }
}

pub fn resolve<'a, T>(
    peers: &'a BTreeMap<String, T>,
    selector: &str,
    id_of: impl Fn(&T) -> &str,
) -> Result<(&'a String, &'a T)> {
    if let Some(peer) = peers.get_key_value(selector) {
        return Ok(peer);
    }
    ensure!(
        !selector.is_empty()
            && selector.len() <= ID_LEN
            && selector.bytes().all(|b| b.is_ascii_hexdigit()),
        "Unknown paired device {selector:?}"
    );
    ensure!(
        selector.len() >= MIN_PREFIX,
        "Device ID prefixes need at least {MIN_PREFIX} hex digits"
    );
    let mut matches = peers.iter().filter(|(_, peer)| {
        let id = id_of(peer);
        id.as_bytes()
            .get(..selector.len())
            .is_some_and(|p| p.eq_ignore_ascii_case(selector.as_bytes()))
            && is_device_id(id)
    });
    let Some(first) = matches.next() else {
        bail!("No paired device matches ID {selector:?}");
    };
    if let Some(second) = matches.next() {
        let names = [first, second]
            .into_iter()
            .chain(matches)
            .map(|(name, _)| format!("{name:?}"))
            .collect::<Vec<_>>()
            .join(", ");
        bail!(
            "Ambiguous device ID {selector:?} matches {names}. Use a longer unique prefix or an exact device name."
        );
    }
    Ok(first)
}

/// Resolve a full device ID or unique hexadecimal prefix against an ID-keyed map.
pub fn resolve_id<'a, T>(
    items: &'a BTreeMap<String, T>,
    selector: &str,
    label: impl Fn(&T) -> &str,
) -> Result<(&'a String, &'a T)> {
    ensure!(
        !selector.is_empty()
            && selector.len() <= ID_LEN
            && selector.bytes().all(|b| b.is_ascii_hexdigit()),
        "Invalid device ID {selector:?}"
    );
    ensure!(
        selector.len() >= MIN_PREFIX,
        "Device ID prefixes need at least {MIN_PREFIX} hex digits"
    );
    let mut matches = items.iter().filter(|(id, _)| {
        is_device_id(id)
            && id
                .as_bytes()
                .get(..selector.len())
                .is_some_and(|p| p.eq_ignore_ascii_case(selector.as_bytes()))
    });
    let Some(first) = matches.next() else {
        bail!("No device matches ID {selector:?}");
    };
    if let Some(second) = matches.next() {
        let all: Vec<_> = [first, second].into_iter().chain(matches).collect();
        let prefixes = unique_prefixes(all.iter().map(|(id, _)| id.as_str()));
        let descriptions = all
            .into_iter()
            .map(|(id, value)| {
                let short = prefixes
                    .get(&id.to_ascii_lowercase())
                    .map_or(default_prefix(id), String::as_str);
                format!("{} ({short})", label(value))
            })
            .collect::<Vec<_>>()
            .join(", ");
        bail!(
            "Ambiguous device ID {selector:?} matches {descriptions}. Use a longer unique prefix."
        );
    }
    Ok(first)
}

/// Shortest unique lowercase prefixes (at least 8 hex digits) for a set of full device IDs.
pub fn unique_prefixes<'a>(ids: impl IntoIterator<Item = &'a str>) -> BTreeMap<String, String> {
    let mut ordered: Vec<String> = ids
        .into_iter()
        .filter(|id| is_device_id(id))
        .map(str::to_ascii_lowercase)
        .collect();
    ordered.sort_unstable();
    ordered.dedup();
    let mut lengths = vec![DISPLAY_PREFIX; ordered.len()];
    for (i, pair) in ordered.windows(2).enumerate() {
        let shared = pair[0]
            .bytes()
            .zip(pair[1].bytes())
            .take_while(|(a, b)| a == b)
            .count();
        let need = (shared + 1).min(ID_LEN);
        lengths[i] = lengths[i].max(need);
        lengths[i + 1] = lengths[i + 1].max(need);
    }
    ordered
        .into_iter()
        .zip(lengths)
        .map(|(id, len)| {
            let prefix = id[..len].to_owned();
            (id, prefix)
        })
        .collect()
}

/// Prefixes that can be copied directly from the device list. Two aliases for
/// the same key must be addressed by name, so neither gets an ID shortcut.
pub fn short_ids(peers: &BTreeMap<String, String>) -> BTreeMap<&str, String> {
    let mut hex_names: BTreeMap<String, Vec<&str>> = BTreeMap::new();
    for name in peers.keys().filter(|name| {
        (DISPLAY_PREFIX..=ID_LEN).contains(&name.len())
            && name.bytes().all(|b| b.is_ascii_hexdigit())
    }) {
        hex_names
            .entry(name.to_ascii_lowercase())
            .or_default()
            .push(name);
    }
    let mut ordered: Vec<_> = peers
        .iter()
        .filter(|(_, id)| is_device_id(id))
        .map(|(name, id)| (id.to_ascii_lowercase(), name.as_str()))
        .collect();
    ordered.sort_unstable();
    // In ID order, only the two neighbors can share the longest prefix.
    let mut lengths = vec![DISPLAY_PREFIX; ordered.len()];
    for (i, pair) in ordered.windows(2).enumerate() {
        let shared = pair[0]
            .0
            .bytes()
            .zip(pair[1].0.bytes())
            .take_while(|(a, b)| a == b)
            .count();
        lengths[i] = lengths[i].max(shared + 1);
        lengths[i + 1] = lengths[i + 1].max(shared + 1);
    }
    ordered
        .into_iter()
        .zip(lengths)
        .filter_map(|((id, name), len)| {
            let prefix = (len..=ID_LEN).map(|n| &id[..n]).find(|&prefix| {
                hex_names
                    .get(prefix)
                    .is_none_or(|owners| owners.as_slice() == [name])
            })?;
            Some((name, prefix.to_owned()))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(prefix: &str) -> String {
        format!("{prefix:0<64}")
    }

    #[test]
    fn names_full_ids_and_unique_prefixes() {
        let peers = BTreeMap::from([("lab".into(), key("ABCD")), ("ab".into(), key("ff00"))]);
        let lookup = |s: &str| resolve(&peers, s, String::as_str).map(|(name, _)| name.as_str());
        assert_eq!(lookup("ab").unwrap(), "ab");
        assert_eq!(lookup("abcd").unwrap(), "lab");
        assert_eq!(lookup(&key("abcd")).unwrap(), "lab");
        for invalid in [
            "",
            "abc",
            "1234",
            "ABCD…0000",
            "lab1",
            "树莓",
            &"a".repeat(65),
        ] {
            assert!(lookup(invalid).is_err(), "{invalid}");
        }
    }

    #[test]
    fn printed_prefixes_handle_colliding_keys_and_hex_names() {
        let peers = BTreeMap::from([
            ("desk".into(), key("deadbeef01")),
            ("树莓派".into(), key("DEADBEEF02")),
            ("deadbeef01".into(), key("12345678")),
            ("DEADBEEF010".into(), key("fee1")),
        ]);
        let ambiguous = resolve(&peers, "DeAdBeEf", String::as_str)
            .unwrap_err()
            .to_string();
        assert!(
            ambiguous.contains("Ambiguous")
                && ambiguous.contains("desk")
                && ambiguous.contains("树莓派")
        );
        let shorts = short_ids(&peers);
        assert_eq!(shorts["desk"], "deadbeef0100");
        assert_eq!(shorts["树莓派"], "deadbeef02");
        assert_eq!(shorts["deadbeef01"], "12345678");
        for (name, short) in shorts {
            assert_eq!(resolve(&peers, &short, String::as_str).unwrap().0, name);
            assert_eq!(
                resolve(&peers, &short.to_uppercase(), String::as_str)
                    .unwrap()
                    .0,
                name
            );
        }
    }

    #[test]
    fn duplicate_keys_require_an_alias() {
        let id = key("deca");
        let peers = BTreeMap::from([
            ("alpha".into(), id.clone()),
            ("beta".into(), id.to_uppercase()),
        ]);
        assert!(short_ids(&peers).is_empty());
        assert!(resolve(&peers, &id, String::as_str).is_err());
        assert_eq!(resolve(&peers, "beta", String::as_str).unwrap().0, "beta");
    }

    #[test]
    fn resolve_id_and_unique_prefixes_match_pending_requests() {
        let a = key("deadbeef01");
        let b = key("deadbeef02");
        let c = key("1234567890");
        let items = BTreeMap::from([
            (a.clone(), "Phone"),
            (b.clone(), "Laptop"),
            (c.clone(), "Tablet"),
        ]);
        let prefixes = unique_prefixes(items.keys().map(String::as_str));
        assert_eq!(prefixes[&a], "deadbeef01");
        assert_eq!(prefixes[&b], "deadbeef02");
        assert_eq!(prefixes[&c], "12345678");
        assert_eq!(resolve_id(&items, "1234", |n| n).unwrap().0, &c);
        assert_eq!(resolve_id(&items, "DEADBEEF02", |n| n).unwrap().0, &b);
        assert_eq!(resolve_id(&items, &a.to_uppercase(), |n| n).unwrap().0, &a);
        let ambiguous = resolve_id(&items, "deadbeef", |n| n)
            .unwrap_err()
            .to_string();
        assert!(
            ambiguous.contains("Ambiguous")
                && ambiguous.contains("Phone (deadbeef01)")
                && ambiguous.contains("Laptop (deadbeef02)")
        );
        for invalid in ["", "123", "zzzz", "not-hex", &"a".repeat(65), "0000"] {
            assert!(resolve_id(&items, invalid, |n| n).is_err(), "{invalid}");
        }
    }
}
