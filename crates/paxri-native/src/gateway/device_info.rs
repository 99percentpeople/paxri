//! After the admission acknowledgement, each end publishes length-prefixed JSON on the
//! selected connection's admission stream. Old peers finish that stream after the handshake.
use std::time::Duration;

use anyhow::{Result, ensure};
use iroh::EndpointId;
use paxri_config::DeviceInfo;
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    sync::watch,
    time::{sleep, timeout},
};

const MAX_INFO: usize = 4096;

pub(super) fn record(
    peers: &watch::Sender<super::PeerDeviceInfo>,
    id: EndpointId,
    info: DeviceInfo,
) {
    let key = id.to_string();
    peers.send_if_modified(|peers| {
        if peers.get(&key) == Some(&info) { return false; }
        peers.insert(key, info);
        true
    });
}

pub(super) async fn publish(
    mut tx: impl AsyncWrite + Unpin,
    mut local: watch::Receiver<DeviceInfo>,
) -> Result<()> {
    loop {
        let bytes = serde_json::to_vec(&*local.borrow_and_update())?;
        ensure!(bytes.len() <= MAX_INFO, "Device information too large");
        tx.write_u16(bytes.len() as u16).await?;
        tx.write_all(&bytes).await?;
        tx.flush().await?;
        local.changed().await?;
    }
}

pub(super) async fn receive(
    mut rx: impl AsyncRead + Unpin,
    mut update: impl FnMut(DeviceInfo),
) -> Result<()> {
    loop {
        let len = rx.read_u16().await? as usize;
        ensure!(
            (1..=MAX_INFO).contains(&len),
            "Invalid device information size"
        );
        let mut buf = vec![0; len];
        timeout(crate::CONNECT_TIMEOUT, rx.read_exact(&mut buf)).await??;
        let mut info: DeviceInfo = serde_json::from_slice(&buf)?;
        info.device_name = info.device_name.filter(|n|
            !n.trim().is_empty() && n.len() <= 256 && !n.chars().any(char::is_control));
        update(info);
        sleep(Duration::from_millis(250)).await;
    }
}
