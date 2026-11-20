//! An invitation opens only the pairing protocol. A gateway operator decides whether to trust the caller.
use super::NativeTransport;
use crate::{CONNECT_TIMEOUT, EndpointTicket};
use anyhow::{Context, Result, ensure};
use iroh::{
    EndpointId,
    endpoint::Connection,
    protocol::{AcceptError, ProtocolHandler},
};
use serde::{Deserialize, Serialize};
use tokio::{
    sync::{Semaphore, mpsc, oneshot},
    time::timeout,
};

pub(super) const PROTOCOL: &[u8] = b"p2p-tunnel/pair/1";
const MAX_FRAME: usize = 16 * 1024;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Contact {
    pub name: String,
    pub ticket: EndpointTicket,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub platform: Option<peerhatch_config::DevicePlatform>,
}

impl Contact {
    pub fn id(&self) -> EndpointId {
        self.ticket.endpoint_addr().id
    }

    fn validate(&self, remote: EndpointId) -> Result<()> {
        ensure!(
            self.id() == remote,
            "Pairing identity does not match the authenticated device"
        );
        ensure!(
            !self.name.trim().is_empty()
                && self.name.len() <= 256
                && !self.name.chars().any(char::is_control),
            "Invalid device name"
        );
        Ok(())
    }
}

pub struct Invitation {
    pub ticket: EndpointTicket,
    pub token: String,
}

impl Invitation {
    pub fn encode(ticket: &str, token: &str) -> String {
        format!("p2p1.{ticket}.{token}")
    }

    pub fn parse(code: &str) -> Result<Self> {
        ensure!(code.len() <= MAX_FRAME, "Pairing code is too long");
        let (ticket, token) = code
            .trim()
            .strip_prefix("p2p1.")
            .and_then(|rest| rest.split_once('.'))
            .context("Invalid pairing code. Paste the entire code from the other device.")?;
        ensure!(
            token.len() == 32 && token.bytes().all(|b| b.is_ascii_hexdigit()),
            "Invalid pairing code"
        );
        Ok(Self {
            ticket: ticket
                .parse()
                .context("Invalid device address in pairing code")?,
            token: token.into(),
        })
    }
}

#[derive(Serialize, Deserialize)]
struct Request {
    request_id: String,
    token: String,
    contact: Contact,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Decision {
    Accepted(Contact),
    Rejected(String),
}

pub struct Incoming {
    pub request_id: String,
    pub token: String,
    pub peer: Contact,
    pub reply: oneshot::Sender<Decision>,
}

#[derive(Debug)]
pub(super) struct PairingHandler {
    incoming: mpsc::Sender<Incoming>,
    slots: Semaphore,
}

impl PairingHandler {
    pub(super) fn new(incoming: mpsc::Sender<Incoming>) -> Self {
        Self {
            incoming,
            slots: Semaphore::new(16),
        }
    }

    async fn serve(&self, conn: Connection) -> Result<()> {
        let _permit = self.slots.try_acquire().context("Pairing is busy")?;
        let (mut tx, mut rx) = timeout(CONNECT_TIMEOUT, conn.accept_bi()).await??;
        let bytes = timeout(CONNECT_TIMEOUT, rx.read_to_end(MAX_FRAME)).await??;
        let Request {
            token,
            contact,
            request_id,
        } = serde_json::from_slice(&bytes)?;
        contact.validate(conn.remote_id())?;
        ensure!(
            request_id.len() == 32 && request_id.bytes().all(|b| b.is_ascii_hexdigit()),
            "Invalid pairing request ID"
        );
        let (reply, response) = oneshot::channel();
        self.incoming
            .try_send(Incoming {
                request_id,
                token,
                peer: contact,
                reply,
            })
            .map_err(|_| anyhow::anyhow!("Pairing is busy"))?;
        let decision = tokio::select! {
            _ = conn.closed() => return Ok(()),
            _ = tx.stopped() => return Ok(()),
            result = response => result.context("Pairing request closed")?,
        };
        let bytes = serde_json::to_vec(&decision)?;
        ensure!(bytes.len() <= MAX_FRAME, "Pairing response too large");
        timeout(CONNECT_TIMEOUT, tx.write_all(&bytes)).await??;
        tx.finish()?;
        let _ = timeout(CONNECT_TIMEOUT, tx.stopped()).await;
        Ok(())
    }
}

impl ProtocolHandler for PairingHandler {
    async fn accept(&self, conn: Connection) -> Result<(), AcceptError> {
        self.serve(conn)
            .await
            .map_err(|e| AcceptError::from_boxed(e.into()))
    }
}

impl NativeTransport {
    pub async fn request_pairing(
        &self,
        invite: &Invitation,
        local: &Contact,
        request_id: &str,
    ) -> Result<Decision> {
        let dest = invite.ticket.endpoint_addr();
        let bytes = serde_json::to_vec(&Request {
            request_id: request_id.into(),
            token: invite.token.clone(),
            contact: local.clone(),
        })?;
        ensure!(bytes.len() <= MAX_FRAME, "Pairing request too large");
        let (_conn, mut rx) = timeout(CONNECT_TIMEOUT, async {
            let conn = super::addresses::dial(&self.endpoint, dest.clone(), PROTOCOL).await?;
            let (mut tx, rx) = conn.open_bi().await?;
            tx.write_all(&bytes).await?;
            tx.finish()?;
            Ok::<_, anyhow::Error>((conn, rx))
        })
        .await
        .context("Could not reach the device for pairing")??;
        let bytes = rx.read_to_end(MAX_FRAME).await?;
        let decision: Decision = serde_json::from_slice(&bytes)?;
        if let Decision::Accepted(contact) = &decision {
            contact.validate(dest.id)?;
        }
        Ok(decision)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use peerhatch_config::DevicePlatform;

    #[test]
    fn contacts_accept_legacy_and_future_platforms() -> Result<()> {
        let id = iroh::SecretKey::from_bytes(&[3; 32]).public();
        let mut message = serde_json::json!({
            "name": "Desktop",
            "ticket": EndpointTicket::new(iroh::EndpointAddr::new(id))
        });
        let legacy: Contact = serde_json::from_value(message.clone())?;
        assert_eq!(legacy.platform, None);
        legacy.validate(id)?;
        for platform in [
            DevicePlatform::Windows,
            DevicePlatform::Linux,
            DevicePlatform::MacOs,
            DevicePlatform::Web,
        ] {
            message["platform"] = platform.as_str().into();
            let contact: Contact = serde_json::from_value(message.clone())?;
            assert_eq!(contact.platform, Some(platform));
            assert_eq!(
                serde_json::to_value(contact)?["platform"],
                platform.as_str()
            );
        }
        message["platform"] = "future-os".into();
        let contact: Contact = serde_json::from_value(message)?;
        assert_eq!(contact.platform, Some(DevicePlatform::Unknown));
        assert!(
            contact
                .validate(iroh::SecretKey::from_bytes(&[4; 32]).public())
                .is_err()
        );
        Ok(())
    }
}
