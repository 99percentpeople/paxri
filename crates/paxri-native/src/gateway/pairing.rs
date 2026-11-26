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

pub(super) const PROTOCOL: &[u8] = b"paxri/pair/1";
const MAX_FRAME: usize = 16 * 1024;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Contact {
    pub name: String,
    pub ticket: EndpointTicket,
    pub platform: paxri_config::DevicePlatform,
    pub capabilities: paxri_config::DeviceCapabilities,
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
        format!("paxri1.{ticket}.{token}")
    }

    pub fn parse(code: &str) -> Result<Self> {
        ensure!(code.len() <= MAX_FRAME, "Pairing code is too long");
        let (ticket, token) = code
            .trim()
            .strip_prefix("paxri1.")
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

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Decision {
    Accepted(Contact),
    Rejected(String),
}

pub struct PendingApproval {
    connection: Connection,
    response: iroh::endpoint::RecvStream,
}

impl PendingApproval {
    pub async fn decision(&mut self) -> Result<Decision> {
        let bytes = self.response.read_to_end(MAX_FRAME).await?;
        let decision: Decision = serde_json::from_slice(&bytes)?;
        if let Decision::Accepted(contact) = &decision {
            contact.validate(self.connection.remote_id())?;
        }
        Ok(decision)
    }

    pub fn withdraw(self) {
        self.connection.close(0u32.into(), b"pairing withdrawn");
    }
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum PairRequest {
    Join {
        request_id: String,
        token: String,
        contact: Contact,
    },
    Unpair {
        pairing_id: Option<String>,
    },
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub enum UnpairAck {
    Cleared,
    NewerPairing,
}

pub enum Incoming {
    Join(JoinRequest),
    Unpair {
        authenticated_id: String,
        pairing_id: Option<String>,
        reply: oneshot::Sender<Result<UnpairAck, String>>,
    },
}

pub struct JoinRequest {
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
        let bytes = match serde_json::from_slice::<PairRequest>(&bytes)? {
            PairRequest::Join {
                token,
                contact,
                request_id,
            } => {
                contact.validate(conn.remote_id())?;
                ensure!(
                    request_id.len() == 32 && request_id.bytes().all(|b| b.is_ascii_hexdigit()),
                    "Invalid pairing request ID"
                );
                let (reply, response) = oneshot::channel();
                self.incoming
                    .try_send(Incoming::Join(JoinRequest {
                        request_id,
                        token,
                        peer: contact,
                        reply,
                    }))
                    .map_err(|_| anyhow::anyhow!("Pairing is busy"))?;
                let decision = tokio::select! {
                    _ = conn.closed() => return Ok(()),
                    _ = tx.stopped() => return Ok(()),
                    result = response => result.context("Pairing request closed")?,
                };
                serde_json::to_vec(&decision)?
            }
            PairRequest::Unpair { pairing_id } => {
                let (reply, response) = oneshot::channel();
                self.incoming
                    .try_send(Incoming::Unpair {
                        authenticated_id: conn.remote_id().to_string(),
                        pairing_id,
                        reply,
                    })
                    .map_err(|_| anyhow::anyhow!("Pairing is busy"))?;
                serde_json::to_vec(&response.await?)?
            }
        };
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
    pub async fn notify_unpair(
        &self,
        id: &str,
        pending: &paxri_config::PendingUnpair,
    ) -> Result<UnpairAck> {
        let ticket: EndpointTicket = pending.ticket.parse()?;
        let dest = ticket.endpoint_addr();
        ensure!(
            dest.id.to_string().eq_ignore_ascii_case(id),
            "Unpair address does not match the device"
        );
        let request = PairRequest::Unpair {
            pairing_id: pending.pairing_id.clone(),
        };
        timeout(std::time::Duration::from_secs(8), async {
            let conn = super::addresses::dial(&self.endpoint, dest.clone(), PROTOCOL).await?;
            let (mut tx, mut rx) = conn.open_bi().await?;
            tx.write_all(&serde_json::to_vec(&request)?).await?;
            tx.finish()?;
            let answer: Result<UnpairAck, String> =
                serde_json::from_slice(&rx.read_to_end(MAX_FRAME).await?)?;
            answer.map_err(anyhow::Error::msg)
        })
        .await
        .context("Peer unpair confirmation timed out")?
    }

    pub async fn request_pairing(
        &self,
        invite: &Invitation,
        local: &Contact,
        request_id: &str,
    ) -> Result<Decision> {
        self.start_pairing(invite, local, request_id)
            .await?
            .decision()
            .await
    }

    pub async fn start_pairing(
        &self,
        invite: &Invitation,
        local: &Contact,
        request_id: &str,
    ) -> Result<PendingApproval> {
        let dest = invite.ticket.endpoint_addr();
        let bytes = serde_json::to_vec(&PairRequest::Join {
            request_id: request_id.into(),
            token: invite.token.clone(),
            contact: local.clone(),
        })?;
        ensure!(bytes.len() <= MAX_FRAME, "Pairing request too large");
        timeout(CONNECT_TIMEOUT, async {
            let conn = super::addresses::dial(&self.endpoint, dest.clone(), PROTOCOL).await?;
            let (mut tx, rx) = conn.open_bi().await?;
            tx.write_all(&bytes).await?;
            tx.finish()?;
            Ok::<_, anyhow::Error>(PendingApproval { connection: conn, response: rx })
        })
        .await
        .context("Could not reach the device for pairing")?
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use paxri_config::{DeviceCapabilities, DevicePlatform};

    #[test]
    fn pairing_requests_carry_explicit_type_and_initiator_contact() -> Result<()> {
        let id = iroh::SecretKey::from_bytes(&[3; 32]).public();
        let join = serde_json::to_value(PairRequest::Join {
            token: "invitation".into(),
            request_id: "attempt".into(),
            contact: Contact {
                name: "Phone".into(),
                ticket: EndpointTicket::new(iroh::EndpointAddr::new(id)),
                platform: DevicePlatform::Android,
                capabilities: DeviceCapabilities::default(),
            },
        })?;
        assert_eq!(join["type"], "join");
        assert_eq!(join["contact"]["name"], "Phone");
        assert_eq!(join["contact"]["platform"], "android");

        let unpair = serde_json::to_value(PairRequest::Unpair {
            pairing_id: Some("gen".into()),
        })?;
        assert_eq!(unpair["type"], "unpair");
        assert_eq!(unpair["pairing_id"], "gen");
        Ok(())
    }

    #[test]
    fn contacts_validate_identity_and_platforms() -> Result<()> {
        let id = iroh::SecretKey::from_bytes(&[3; 32]).public();
        let mut message = serde_json::json!({
            "name": "Desktop",
            "ticket": EndpointTicket::new(iroh::EndpointAddr::new(id)),
            "platform": "linux",
            "capabilities": DeviceCapabilities::default(),
        });
        for platform in [
            DevicePlatform::Android,
            DevicePlatform::Windows,
            DevicePlatform::Linux,
            DevicePlatform::MacOs,
            DevicePlatform::Web,
        ] {
            message["platform"] = platform.as_str().into();
            let contact: Contact = serde_json::from_value(message.clone())?;
            assert_eq!(contact.platform, platform);
            contact.validate(id)?;
            assert_eq!(
                serde_json::to_value(contact)?["platform"],
                platform.as_str()
            );
        }
        message["platform"] = "future-os".into();
        let contact: Contact = serde_json::from_value(message)?;
        assert_eq!(contact.platform, DevicePlatform::Unknown);
        assert!(
            contact
                .validate(iroh::SecretKey::from_bytes(&[4; 32]).public())
                .is_err()
        );
        Ok(())
    }
}
