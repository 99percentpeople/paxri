//! Print one local gateway snapshot of the selected peer.
use crate::{gateway, output};
use anyhow::Result;
use paxri_control::{default_address, is_offline, peer_status};
use std::path::Path;

pub(super) async fn show_peer(
    selected: Option<&Path>,
    selector: &str,
    check: bool,
    json: bool,
) -> Result<()> {
    let ipc = selected.map(Path::to_owned).unwrap_or_else(default_address);
    let snapshot = match peer_status(&ipc, selector, check).await {
        Err(e)
            if is_offline(&e)
                && selected.is_none()
                && paxri_control::ipc_from_env().is_none() =>
        {
            // Keep profile-aware discovery for a gateway listening on a non-default address.
            let Some(other) = gateway::discover(None, None)
                .await?
                .and_then(|s| s.ipc_path)
            else {
                return Err(e);
            };
            peer_status(&other, selector, check).await?
        }
        result => result?,
    };
    if json {
        println!("{}", serde_json::to_string_pretty(&snapshot)?);
    } else {
        print!("{}", output::peer(&snapshot));
    }
    Ok(())
}
