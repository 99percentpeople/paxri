use std::net::Ipv4Addr;

use anyhow::Result;
use iroh::{Endpoint, RelayMode, SecretKey, endpoint::presets};
use iroh_relay::server::{RelayConfig, Server, ServerConfig};
use tunnel_config::Native;
use tunnel_native::{bind, identity, listen, probe, relay_ticket, ticket};

#[test]
fn existing_keys_are_preserved_including_unreadable_formats() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("device.key");
    let public = identity::init(&path)?.public();
    assert_eq!(identity::init(&path)?.public(), public);
    std::fs::write(&path, b"damaged")?;
    assert!(identity::init(&path).is_err());
    assert_eq!(std::fs::read(path)?, b"damaged");
    Ok(())
}

#[tokio::test]
async fn direct_probe_checks_both_peer_identities() -> Result<()> {
    let client = bind(SecretKey::generate(), &Native::default(), true).await?;
    let server = listen(
        bind(SecretKey::generate(), &Native::default(), true).await?,
        client.id(),
    );
    let dest = ticket(server.endpoint(), true).await?;
    let remote = server.endpoint().id();
    let mut samples = Vec::new();
    probe(&client, dest.clone(), remote, 2, |sample| {
        samples.push(sample)
    })
    .await?;
    assert_eq!(samples.len(), 2);
    assert!(
        samples
            .iter()
            .all(|s| s.path == "direct" && s.path_rx_bytes > 0)
    );
    assert!(samples[1].path_tx_bytes > samples[0].path_tx_bytes);

    let wrong = probe(&client, dest.clone(), client.id(), 1, |_| {})
        .await
        .unwrap_err();
    assert!(
        wrong
            .to_string()
            .contains("ticket does not match configured peer")
    );

    let stranger = bind(SecretKey::generate(), &Native::default(), true).await?;
    let denied = probe(&stranger, dest, remote, 1, |_| {}).await.unwrap_err();
    assert!(
        format!("{denied:#}").contains("peer not allowed"),
        "{denied:#}"
    );
    stranger.close().await;
    client.close().await;
    server.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn probe_uses_local_relay_when_client_has_no_ip_transport() -> Result<()> {
    let mut config = ServerConfig::default();
    config.relay = Some(RelayConfig::new((Ipv4Addr::LOCALHOST, 0)));
    let relay = Server::spawn(config).await?;
    let relay_url = format!("http://{}", relay.http_addr().unwrap());
    let native = Native {
        relay_url: Some(relay_url.parse()?),
        ..Native::default()
    };
    let client = Endpoint::builder(presets::Minimal)
        .clear_ip_transports()
        .relay_mode(RelayMode::custom([relay_url.parse()?]))
        .bind()
        .await?;
    let server = listen(
        bind(SecretKey::generate(), &native, false).await?,
        client.id(),
    );
    tokio::time::timeout(
        std::time::Duration::from_secs(8),
        server.endpoint().online(),
    )
    .await?;
    let dest = relay_ticket(server.endpoint().id(), &native)?;
    assert_eq!(dest.endpoint_addr().relay_urls().count(), 1);
    assert_eq!(dest.endpoint_addr().ip_addrs().count(), 0);
    let mut paths = Vec::new();
    probe(&client, dest, server.endpoint().id(), 2, |s| {
        paths.push((s.path, s.path_rx_bytes))
    })
    .await?;
    assert_eq!(paths.len(), 2);
    assert!(
        paths
            .iter()
            .all(|(kind, received)| *kind == "relay" && *received > 0)
    );
    client.close().await;
    server.shutdown().await?;
    relay.shutdown().await?;
    Ok(())
}
