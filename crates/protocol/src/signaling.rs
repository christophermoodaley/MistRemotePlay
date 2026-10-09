//! Device <-> signaling server messages (JSON text frames over WebSocket/TLS).
//!
//! The server authenticates devices with an Ed25519 challenge, routes signed
//! peer envelopes, and hands out STUN/TURN configuration. It never sees
//! media, input events or pairing codes.

use serde::{Deserialize, Serialize};

use crate::identity::{DeviceId, PublicKeyB64};
use crate::peer::SignedEnvelope;
use crate::{check_len, check_printable, invalid, pairing, Result, Validate};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Host,
    Client,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ClientToServer {
    Hello {
        protocol_version: u16,
        device_id: DeviceId,
        public_key: PublicKeyB64,
        device_name: String,
        role: Role,
        /// Optional shared secret for private deployments (MIST_ACCESS_TOKEN).
        #[serde(default)]
        access_token: Option<String>,
    },
    /// Signature over [`crate::identity::server_auth_message`].
    AuthResponse { signature: String },
    /// Host announces a pairing code lookup handle.
    RegisterPairing { lookup: String, ttl_secs: u32 },
    CancelPairing { lookup: String },
    /// Client asks to pair with whichever host registered `lookup`.
    PairRequest { lookup: String, envelope: SignedEnvelope },
    /// Forward a signed envelope to an online device.
    Relay { to: DeviceId, envelope: SignedEnvelope },
    QueryPresence { device_ids: Vec<DeviceId> },
    Ping {},
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ServerToClient {
    Challenge { nonce: String, server_version: String },
    Welcome { ice_servers: Vec<IceServer>, connection_ttl_secs: u32 },
    PairingRegistered { lookup: String, expires_in_secs: u32 },
    PairRequest { lookup: String, from: DeviceId, from_public_key: PublicKeyB64, envelope: SignedEnvelope },
    Relay { from: DeviceId, from_public_key: PublicKeyB64, envelope: SignedEnvelope },
    Presence { online: Vec<DeviceId>, offline: Vec<DeviceId> },
    Error { code: ErrorCode, message: String },
    Pong {},
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    BadRequest,
    Unauthorized,
    UnsupportedVersion,
    RateLimited,
    PeerOffline,
    UnknownPairingCode,
    TooManyConnections,
    Internal,
}

/// A WebRTC ICE server entry (STUN or TURN).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IceServer {
    pub urls: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credential: Option<String>,
}

impl IceServer {
    pub fn is_turn(&self) -> bool {
        self.urls.iter().any(|u| u.starts_with("turn:") || u.starts_with("turns:"))
    }
}

impl std::fmt::Display for IceServer {
    /// Display without credentials (safe to log).
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.urls.join(","))?;
        if self.username.is_some() {
            f.write_str(" (with credentials)")?;
        }
        Ok(())
    }
}

impl Validate for IceServer {
    fn validate(&self) -> Result<()> {
        if self.urls.is_empty() || self.urls.len() > 8 {
            return Err(invalid("ice_server.urls", "must contain 1..=8 urls"));
        }
        for u in &self.urls {
            check_printable("ice_server.url", u, 512)?;
            if !(u.starts_with("stun:") || u.starts_with("stuns:") || u.starts_with("turn:") || u.starts_with("turns:")) {
                return Err(invalid("ice_server.url", "scheme must be stun, stuns, turn or turns"));
            }
        }
        if let Some(u) = &self.username {
            check_printable("ice_server.username", u, 256)?;
        }
        if let Some(c) = &self.credential {
            check_printable("ice_server.credential", c, 256)?;
        }
        Ok(())
    }
}

impl Validate for ClientToServer {
    fn validate(&self) -> Result<()> {
        match self {
            ClientToServer::Hello { device_id, public_key, device_name, access_token, .. } => {
                device_id.validate()?;
                if &public_key.device_id()? != device_id {
                    return Err(invalid("device_id", "does not match public key"));
                }
                check_printable("device_name", device_name, 64)?;
                if let Some(t) = access_token {
                    check_len("access_token", t, 256)?;
                }
                Ok(())
            }
            ClientToServer::AuthResponse { signature } => check_len("signature", signature, 128),
            ClientToServer::RegisterPairing { lookup, ttl_secs } => {
                pairing::validate_lookup(lookup)?;
                if !(30..=900).contains(ttl_secs) {
                    return Err(invalid("ttl_secs", "must be in 30..=900"));
                }
                Ok(())
            }
            ClientToServer::CancelPairing { lookup } => pairing::validate_lookup(lookup),
            ClientToServer::PairRequest { lookup, envelope } => {
                pairing::validate_lookup(lookup)?;
                envelope.validate()
            }
            ClientToServer::Relay { to, envelope } => {
                to.validate()?;
                envelope.validate()
            }
            ClientToServer::QueryPresence { device_ids } => {
                if device_ids.len() > 64 {
                    return Err(invalid("device_ids", "at most 64 per query"));
                }
                device_ids.iter().try_for_each(DeviceId::validate)
            }
            ClientToServer::Ping {} => Ok(()),
        }
    }
}

impl Validate for ServerToClient {
    fn validate(&self) -> Result<()> {
        match self {
            ServerToClient::Challenge { nonce, server_version } => {
                check_len("nonce", nonce, 128)?;
                check_printable("server_version", server_version, 64)
            }
            ServerToClient::Welcome { ice_servers, .. } => {
                if ice_servers.len() > 16 {
                    return Err(invalid("ice_servers", "too many"));
                }
                ice_servers.iter().try_for_each(Validate::validate)
            }
            ServerToClient::PairingRegistered { lookup, .. } => pairing::validate_lookup(lookup),
            ServerToClient::PairRequest { lookup, from, from_public_key, envelope } => {
                pairing::validate_lookup(lookup)?;
                from.validate()?;
                if &from_public_key.device_id()? != from {
                    return Err(invalid("from", "does not match public key"));
                }
                envelope.validate()
            }
            ServerToClient::Relay { from, from_public_key, envelope } => {
                from.validate()?;
                if &from_public_key.device_id()? != from {
                    return Err(invalid("from", "does not match public key"));
                }
                envelope.validate()
            }
            ServerToClient::Presence { online, offline } => {
                online.iter().chain(offline).try_for_each(DeviceId::validate)
            }
            ServerToClient::Error { message, .. } => check_printable("message", message, 512),
            ServerToClient::Pong {} => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::DeviceIdentity;
    use crate::{decode, encode, MAX_SIGNALING_MESSAGE_BYTES};

    fn hello(id: &DeviceIdentity) -> ClientToServer {
        ClientToServer::Hello {
            protocol_version: crate::PROTOCOL_VERSION,
            device_id: id.device_id(),
            public_key: id.public_key_b64(),
            device_name: "Desk PC".into(),
            role: Role::Host,
            access_token: None,
        }
    }

    #[test]
    fn hello_roundtrip() {
        let id = DeviceIdentity::generate();
        let msg = hello(&id);
        let parsed: ClientToServer = decode(encode(&msg).as_bytes(), MAX_SIGNALING_MESSAGE_BYTES).unwrap();
        assert_eq!(parsed, msg);
    }

    #[test]
    fn hello_with_foreign_device_id_rejected() {
        let id = DeviceIdentity::generate();
        let mut msg = hello(&id);
        if let ClientToServer::Hello { device_id, .. } = &mut msg {
            *device_id = DeviceIdentity::generate().device_id();
        }
        assert!(msg.validate().is_err());
    }

    #[test]
    fn unknown_types_and_fields_rejected() {
        assert!(decode::<ClientToServer>(br#"{"type":"exec","cmd":"rm -rf /"}"#, 1024).is_err());
        assert!(decode::<ClientToServer>(br#"{"type":"ping","extra":1}"#, 1024).is_err());
        assert!(decode::<ClientToServer>(br#"{"type":"ping"}"#, 1024).is_ok());
        let big = vec![b' '; 2048];
        assert!(matches!(decode::<ClientToServer>(&big, 1024), Err(crate::ProtocolError::TooLarge(_))));
    }

    #[test]
    fn ice_server_validation() {
        let ok = IceServer { urls: vec!["turn:example.org:3478?transport=udp".into()], username: Some("u".into()), credential: Some("secretpass".into()) };
        ok.validate().unwrap();
        assert!(ok.is_turn());
        assert!(!format!("{ok}").contains("secretpass"));
        let bad = IceServer { urls: vec!["http://evil".into()], username: None, credential: None };
        assert!(bad.validate().is_err());
    }
}
