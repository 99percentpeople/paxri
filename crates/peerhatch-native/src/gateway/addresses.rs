//! Refresh bootstrap addresses without opening another application connection.
use crate::{CONNECT_TIMEOUT, EndpointTicket};
use anyhow::{Context, Result, ensure};
use futures_util::StreamExt;
use iroh::{
    Endpoint, EndpointAddr, EndpointId, RelayMode, TransportAddr, Watcher, endpoint::Connection,
};
use std::time::Duration;
use tokio::{
    io::AsyncReadExt,
    sync::mpsc,
    time::{sleep, timeout},
};

const MAX_TICKET: usize = 8192;
const ADDRESS_VERSION: u8 = 1;

fn refreshed(saved: &EndpointAddr, published: EndpointAddr) -> Option<EndpointAddr> {
    if published.id != saved.id || published.relay_urls().next().is_none() {
        return None;
    }
    let mut address = published;
    // The public directory may publish only a relay. Preserve the private/LAN and
    // mapped addresses shared directly by the authenticated peer.
    address
        .addrs
        .extend(saved.ip_addrs().copied().map(TransportAddr::Ip));
    (address != *saved).then_some(address)
}

pub(super) async fn dial(ep: &Endpoint, saved: EndpointAddr, alpn: &[u8]) -> Result<Connection> {
    let public = RelayMode::Default.relay_map();
    if saved.relay_urls().any(|u| public.get(u).is_some()) {
        let lookup = async {
            let services = ep.address_lookup().ok()?;
            let mut items = std::pin::pin!(services.resolve(saved.id));
            while let Some(item) = items.next().await {
                if let Ok(Ok(item)) = item {
                    let addr = item.into_endpoint_addr();
                    if addr.id == saved.id && addr.relay_urls().next().is_some() {
                        return refreshed(&saved, addr);
                    }
                }
            }
            None
        };
        if let Ok(Some(addr)) = timeout(Duration::from_millis(850), lookup).await
            && let Ok(Ok(conn)) = timeout(Duration::from_secs(5), ep.connect(addr, alpn)).await
        {
            return Ok(conn);
        }
    }
    Ok(ep.connect(saved, alpn).await?)
}

pub(super) fn exchange(
    ep: Endpoint,
    connection: Connection,
    updates: mpsc::Sender<EndpointTicket>,
) {
    tokio::spawn(async move {
        tokio::select! {
            _ = connection.closed() => {}
            _ = async { tokio::join!(publish(ep, &connection), receive(&connection, updates)) } => {}
        }
    });
}

async fn publish(ep: Endpoint, connection: &Connection) -> Result<()> {
    let mut changes = ep.watch_addr();
    let mut address = changes.get();
    let mut tx = connection.open_uni().await?;
    tx.write_all(&[ADDRESS_VERSION]).await?;
    loop {
        if !address.is_empty() {
            let ticket = EndpointTicket::new(address.clone()).to_string();
            if ticket.len() <= MAX_TICKET {
                tx.write_all(&(ticket.len() as u16).to_be_bytes()).await?;
                tx.write_all(ticket.as_bytes()).await?;
            }
        }
        loop {
            let next = changes.updated().await?;
            if next != address {
                address = next;
                break;
            }
        }
    }
}

async fn receive(connection: &Connection, updates: mpsc::Sender<EndpointTicket>) -> Result<()> {
    let mut rx = connection.accept_uni().await?;
    ensure!(
        rx.read_u8().await? == ADDRESS_VERSION,
        "Unknown address update version"
    );
    loop {
        let len = rx.read_u16().await? as usize;
        ensure!(
            (1..=MAX_TICKET).contains(&len),
            "Invalid peer address update size"
        );
        let mut bytes = vec![0; len];
        timeout(CONNECT_TIMEOUT, rx.read_exact(&mut bytes)).await??;
        let ticket = checked_ticket(&bytes, connection.remote_id())?;
        updates.send(ticket).await.context("Gateway stopped")?;
        sleep(Duration::from_secs(1)).await;
    }
}

fn checked_ticket(bytes: &[u8], peer: EndpointId) -> Result<EndpointTicket> {
    let ticket: EndpointTicket = std::str::from_utf8(bytes)?.parse()?;
    let addr = ticket.endpoint_addr();
    ensure!(
        addr.id == peer && !addr.is_empty() && addr.addrs.len() <= 64,
        "Invalid addresses for authenticated peer"
    );
    Ok(ticket)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn new_home_replaces_old_home_and_retains_direct_candidates() {
        let peer = iroh::SecretKey::generate().public();
        let saved = EndpointAddr::new(peer)
            .with_relay_url("https://west.example".parse().unwrap())
            .with_ip_addr("127.0.0.1:3001".parse().unwrap());
        let current =
            EndpointAddr::new(peer).with_relay_url("https://east.example".parse().unwrap());
        let addr = refreshed(&saved, current.clone()).unwrap();
        assert_eq!(
            addr.relay_urls().collect::<Vec<_>>(),
            current.relay_urls().collect::<Vec<_>>()
        );
        assert_eq!(addr.ip_addrs().count(), 1);
        let ticket = EndpointTicket::new(addr).to_string();
        assert!(checked_ticket(ticket.as_bytes(), peer).is_ok());
        assert!(checked_ticket(ticket.as_bytes(), iroh::SecretKey::generate().public()).is_err());
    }
}
