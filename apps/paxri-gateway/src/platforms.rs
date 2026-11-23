use std::{sync::Arc, time::Duration};

use tokio::{sync::Mutex, task::JoinSet};
use paxri_config::DevicePlatform;

use super::Gateway;

/// Older pairing records only contain identity and addressing information.
pub(super) async fn fill_missing(gateway: Arc<Mutex<Gateway>>) {
    let transport = gateway.lock().await.transport.clone();
    let mut settings = transport.subscribe_config();
    let mut retry = Duration::from_secs(15);
    loop {
        let Ok(config) = settings
            .wait_for(|cfg| cfg.peers.values().any(|p| p.platform.unwrap_or_default() == DevicePlatform::Unknown))
            .await
            .map(|snapshot| Arc::clone(&snapshot))
        else {
            return;
        };
        let mut queries = JoinSet::new();
        for (name, peer) in config.peers.iter().filter(|(_, p)| p.platform.unwrap_or_default() == DevicePlatform::Unknown) {
            if queries.len() == 4 {
                queries.join_next().await;
            }
            let transport = transport.clone();
            let gateway = gateway.clone();
            let (name, id) = (name.clone(), peer.endpoint_id.clone());
            queries.spawn(async move {
                let Ok(platform) = paxri_core::peer_platform(&*transport, &name).await else {
                    return;
                };
                if let Err(err) = gateway
                    .lock()
                    .await
                    .save_peer_platform(&name, &id, platform)
                {
                    eprintln!("save platform for {name}: {err:#}");
                }
            });
        }
        while queries.join_next().await.is_some() {}
        if settings
            .borrow()
            .peers
            .values()
            .any(|p| p.platform.unwrap_or_default() == DevicePlatform::Unknown)
        {
            tokio::time::sleep(retry).await;
            retry = (retry * 2).min(Duration::from_secs(600));
        } else {
            retry = Duration::from_secs(15);
        }
    }
}

impl Gateway {
    fn save_peer_platform(
        &mut self,
        name: &str,
        id: &str,
        platform: DevicePlatform,
    ) -> anyhow::Result<()> {
        // Pairing may have updated the alias or supplied fresher metadata while the query was in flight.
        if platform != DevicePlatform::Unknown && self
            .config
            .peers
            .get(name)
            .is_some_and(|p| p.endpoint_id == id && p.platform.unwrap_or_default() == DevicePlatform::Unknown)
        {
            let (text, config) = self.file.prepare_peer_platform(name, platform)?;
            self.file.persist(text)?;
            self.apply_config(config);
        }
        Ok(())
    }
}
