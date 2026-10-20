//! Host <-> client messages relayed by the signaling server.
//!
//! Each message is wrapped in a [`SignedEnvelope`] signed by the sender's
//! device key. Because SDP offers/answers (which carry the DTLS certificate
//! fingerprints) are signed, a compromised signaling server cannot
//! man-in-the-middle the WebRTC session of an already-paired device pair.

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};

use crate::protocol::identity::{verify_signature, DeviceId, DeviceIdentity, PublicKeyB64};
use crate::protocol::settings::StreamSettings;
use crate::protocol::{check_len, check_printable, invalid, ProtocolError, Result, Validate};

/// Maximum accepted clock difference between peers.
pub const MAX_CLOCK_SKEW_MS: u64 = 5 * 60 * 1000;
const MAX_SDP_BYTES: usize = 32 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SignedEnvelope {
    /// JSON-encoded [`EnvelopeBody`]; signed verbatim.
    pub body: String,
    /// Base64 Ed25519 signature over `"mist-envelope-v1\n" || body`.
    pub signature: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EnvelopeBody {
    pub from: DeviceId,
    /// `None` only for pairing requests, whose recipient is unknown yet.
    pub to: Option<DeviceId>,
    pub sent_at_ms: u64,
    pub nonce: String,
    pub message: PeerMessage,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SdpKind {
    Offer,
    Answer,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClientCapabilities {
    pub decoders: Vec<String>,
    pub max_width: u32,
    pub max_height: u32,
    pub platform: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum PeerMessage {
    /// `proof` = HMAC(code, client_public_key, nonce), see [`crate::protocol::pairing`].
    PairRequest {
        client_name: String,
        client_public_key: PublicKeyB64,
        nonce: String,
        proof: String,
    },
    PairAccepted {
        host_name: String,
        host_public_key: PublicKeyB64,
        proof: String,
    },
    PairRejected {
        reason: String,
    },
    /// `resume` asks to re-attach to a session the host approved earlier
    /// whose connection failed moments ago (no second consent prompt).
    SessionRequest {
        session_id: String,
        settings: StreamSettings,
        capabilities: ClientCapabilities,
        #[serde(default)]
        resume: bool,
    },
    SessionAccepted {
        session_id: String,
        expires_at_ms: u64,
        settings: StreamSettings,
        input_allowed: bool,
    },
    SessionRejected {
        session_id: String,
        reason: String,
    },
    Sdp {
        session_id: String,
        kind: SdpKind,
        sdp: String,
    },
    IceCandidate {
        session_id: String,
        mline_index: u32,
        candidate: String,
    },
    EndSession {
        session_id: String,
        reason: String,
    },
}

impl PeerMessage {
    pub fn session_id(&self) -> Option<&str> {
        match self {
            PeerMessage::SessionRequest { session_id, .. }
            | PeerMessage::SessionAccepted { session_id, .. }
            | PeerMessage::SessionRejected { session_id, .. }
            | PeerMessage::Sdp { session_id, .. }
            | PeerMessage::IceCandidate { session_id, .. }
            | PeerMessage::EndSession { session_id, .. } => Some(session_id),
            _ => None,
        }
    }
}

pub fn validate_session_id(id: &str) -> Result<()> {
    if id.len() != 32 || !id.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(invalid("session_id", "must be 32 hex characters"));
    }
    Ok(())
}

impl Validate for PeerMessage {
    fn validate(&self) -> Result<()> {
        if let Some(id) = self.session_id() {
            validate_session_id(id)?;
        }
        match self {
            PeerMessage::PairRequest { client_name, client_public_key, nonce, proof } => {
                check_printable("client_name", client_name, 64)?;
                client_public_key.to_key()?;
                check_len("nonce", nonce, 64)?;
                check_len("proof", proof, 64)
            }
            PeerMessage::PairAccepted { host_name, host_public_key, proof } => {
                check_printable("host_name", host_name, 64)?;
                host_public_key.to_key()?;
                check_len("proof", proof, 64)
            }
            PeerMessage::PairRejected { reason }
            | PeerMessage::SessionRejected { reason, .. }
            | PeerMessage::EndSession { reason, .. } => check_printable("reason", reason, 256),
            PeerMessage::SessionRequest { settings, capabilities, .. } => {
                settings.validate()?;
                if capabilities.decoders.len() > 32 {
                    return Err(invalid("capabilities.decoders", "too many"));
                }
                for d in &capabilities.decoders {
                    check_printable("capabilities.decoders", d, 64)?;
                }
                check_printable("capabilities.platform", &capabilities.platform, 64)
            }
            PeerMessage::SessionAccepted { settings, .. } => settings.validate(),
            PeerMessage::Sdp { sdp, .. } => {
                check_len("sdp", sdp, MAX_SDP_BYTES)?;
                if !sdp.starts_with("v=0") {
                    return Err(invalid("sdp", "not an SDP document"));
                }
                Ok(())
            }
            PeerMessage::IceCandidate { mline_index, candidate, .. } => {
                if *mline_index > 16 {
                    return Err(invalid("mline_index", "out of range"));
                }
                check_printable("candidate", candidate, 1024)?;
                if !(candidate.is_empty() || candidate.starts_with("candidate:")) {
                    return Err(invalid("candidate", "not an ICE candidate line"));
                }
                Ok(())
            }
        }
    }
}

fn signing_input(body: &str) -> Vec<u8> {
    let mut v = b"mist-envelope-v1\n".to_vec();
    v.extend_from_slice(body.as_bytes());
    v
}

pub fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

impl SignedEnvelope {
    pub fn seal(identity: &DeviceIdentity, to: Option<DeviceId>, message: PeerMessage) -> Self {
        let body = EnvelopeBody {
            from: identity.device_id(),
            to,
            sent_at_ms: now_ms(),
            nonce: crate::protocol::identity::random_hex(16),
            message,
        };
        let body = crate::protocol::encode(&body);
        let signature = identity.sign(&signing_input(&body));
        SignedEnvelope { body, signature }
    }

    /// Verify signature, sender, recipient, freshness and message validity.
    /// Replay protection is applied separately with a [`ReplayGuard`].
    pub fn open(&self, sender_key: &PublicKeyB64, expected_to: Option<&DeviceId>) -> Result<EnvelopeBody> {
        let key = sender_key.to_key()?;
        verify_signature(&key, &signing_input(&self.body), &self.signature)?;
        let body: EnvelopeBody =
            serde_json::from_str(&self.body).map_err(|e| ProtocolError::Malformed(e.to_string()))?;
        if body.from != sender_key.device_id()? {
            return Err(invalid("from", "does not match signing key"));
        }
        match (&body.to, expected_to) {
            (Some(to), Some(me)) if to == me => {}
            (None, _) if matches!(body.message, PeerMessage::PairRequest { .. }) => {}
            _ => return Err(invalid("to", "envelope addressed to another device")),
        }
        check_len("nonce", &body.nonce, 64)?;
        let now = now_ms();
        if body.sent_at_ms + MAX_CLOCK_SKEW_MS < now || body.sent_at_ms > now + MAX_CLOCK_SKEW_MS {
            return Err(ProtocolError::Stale);
        }
        body.message.validate()?;
        Ok(body)
    }
}

impl Validate for SignedEnvelope {
    fn validate(&self) -> Result<()> {
        check_len("envelope.body", &self.body, MAX_SDP_BYTES + 4096)?;
        check_len("envelope.signature", &self.signature, 128)
    }
}

/// Rejects envelopes whose (sender, nonce) was already seen within the
/// freshness window.
#[derive(Default)]
pub struct ReplayGuard {
    seen: HashMap<(DeviceId, String), u64>,
    order: VecDeque<((DeviceId, String), u64)>,
}

impl ReplayGuard {
    const CAPACITY: usize = 50_000;

    pub fn check(&mut self, body: &EnvelopeBody) -> Result<()> {
        let now = now_ms();
        while let Some((_, t)) = self.order.front() {
            if *t + 2 * MAX_CLOCK_SKEW_MS < now || self.order.len() > Self::CAPACITY {
                let (k, _) = self.order.pop_front().expect("front exists");
                self.seen.remove(&k);
            } else {
                break;
            }
        }
        let key = (body.from.clone(), body.nonce.clone());
        if self.seen.contains_key(&key) {
            return Err(ProtocolError::Replay);
        }
        self.seen.insert(key.clone(), now);
        self.order.push_back((key, now));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sdp_msg() -> PeerMessage {
        PeerMessage::Sdp { session_id: "a".repeat(32), kind: SdpKind::Offer, sdp: "v=0\r\n".into() }
    }

    #[test]
    fn seal_and_open() {
        let host = DeviceIdentity::generate();
        let client = DeviceIdentity::generate();
        let env = SignedEnvelope::seal(&client, Some(host.device_id()), sdp_msg());
        let body = env.open(&client.public_key_b64(), Some(&host.device_id())).unwrap();
        assert_eq!(body.message, sdp_msg());
    }

    #[test]
    fn wrong_recipient_or_key_rejected() {
        let host = DeviceIdentity::generate();
        let client = DeviceIdentity::generate();
        let mallory = DeviceIdentity::generate();
        let env = SignedEnvelope::seal(&client, Some(host.device_id()), sdp_msg());
        assert!(env.open(&client.public_key_b64(), Some(&mallory.device_id())).is_err());
        assert_eq!(env.open(&mallory.public_key_b64(), Some(&host.device_id())), Err(ProtocolError::BadSignature));
        let mut tampered = env.clone();
        tampered.body = tampered.body.replace("v=0", "v=1");
        assert!(tampered.open(&client.public_key_b64(), Some(&host.device_id())).is_err());
    }

    #[test]
    fn unaddressed_envelope_only_for_pairing() {
        let host = DeviceIdentity::generate();
        let client = DeviceIdentity::generate();
        let env = SignedEnvelope::seal(&client, None, sdp_msg());
        assert!(env.open(&client.public_key_b64(), Some(&host.device_id())).is_err());
        let pair = PeerMessage::PairRequest {
            client_name: "laptop".into(),
            client_public_key: client.public_key_b64(),
            nonce: "n".into(),
            proof: "00".into(),
        };
        let env = SignedEnvelope::seal(&client, None, pair);
        assert!(env.open(&client.public_key_b64(), Some(&host.device_id())).is_ok());
    }

    #[test]
    fn replay_guard_rejects_duplicates() {
        let host = DeviceIdentity::generate();
        let client = DeviceIdentity::generate();
        let env = SignedEnvelope::seal(&client, Some(host.device_id()), sdp_msg());
        let body = env.open(&client.public_key_b64(), Some(&host.device_id())).unwrap();
        let mut guard = ReplayGuard::default();
        guard.check(&body).unwrap();
        assert_eq!(guard.check(&body), Err(ProtocolError::Replay));
        let env2 = SignedEnvelope::seal(&client, Some(host.device_id()), sdp_msg());
        guard.check(&env2.open(&client.public_key_b64(), Some(&host.device_id())).unwrap()).unwrap();
    }

    #[test]
    fn stale_envelopes_rejected() {
        let host = DeviceIdentity::generate();
        let client = DeviceIdentity::generate();
        let body = EnvelopeBody {
            from: client.device_id(),
            to: Some(host.device_id()),
            sent_at_ms: now_ms() - MAX_CLOCK_SKEW_MS - 10_000,
            nonce: "n".into(),
            message: sdp_msg(),
        };
        let body = crate::protocol::encode(&body);
        let env = SignedEnvelope { signature: client.sign(&signing_input(&body)), body };
        assert_eq!(env.open(&client.public_key_b64(), Some(&host.device_id())), Err(ProtocolError::Stale));
    }

    #[test]
    fn invalid_candidates_rejected() {
        let m = PeerMessage::IceCandidate { session_id: "a".repeat(32), mline_index: 0, candidate: "a=evil".into() };
        assert!(m.validate().is_err());
        let m = PeerMessage::IceCandidate {
            session_id: "a".repeat(32),
            mline_index: 0,
            candidate: "candidate:1 1 UDP 2015363327 192.168.1.2 51234 typ host".into(),
        };
        m.validate().unwrap();
    }
}
