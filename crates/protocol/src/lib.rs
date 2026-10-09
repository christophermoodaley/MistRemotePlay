//! Mist Remote Play wire protocol.
//!
//! Three channels carry typed messages:
//! * signaling (device <-> signaling server, WebSocket/TLS): [`signaling`]
//! * peer envelopes (host <-> client, relayed by the server but signed end to
//!   end with the device identity keys): [`peer`]
//! * WebRTC data channels (client <-> host, inside DTLS): [`input`], [`control`]
//!
//! Every privileged message is validated with [`Validate`] before use.

pub mod control;
pub mod identity;
pub mod input;
pub mod keys;
pub mod pairing;
pub mod peer;
pub mod settings;
pub mod signaling;

/// Bumped whenever a message changes incompatibly.
pub const PROTOCOL_VERSION: u16 = 1;

/// Upper bound for any single signaling frame (SDP offers are ~4-8 KiB).
pub const MAX_SIGNALING_MESSAGE_BYTES: usize = 64 * 1024;
/// Upper bound for a single data channel message.
pub const MAX_DATA_CHANNEL_MESSAGE_BYTES: usize = 2 * 1024;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ProtocolError {
    #[error("message too large: {0} bytes")]
    TooLarge(usize),
    #[error("malformed message: {0}")]
    Malformed(String),
    #[error("invalid field `{field}`: {reason}")]
    InvalidField { field: &'static str, reason: String },
    #[error("unsupported protocol version {0}")]
    UnsupportedVersion(u16),
    #[error("bad signature")]
    BadSignature,
    #[error("message expired or from the future")]
    Stale,
    #[error("replayed message")]
    Replay,
}

pub type Result<T> = std::result::Result<T, ProtocolError>;

/// Semantic validation applied after deserialization.
pub trait Validate {
    fn validate(&self) -> Result<()>;
}

pub(crate) fn invalid(field: &'static str, reason: impl Into<String>) -> ProtocolError {
    ProtocolError::InvalidField { field, reason: reason.into() }
}

/// Decode a JSON message with a size bound, then validate it.
pub fn decode<T>(bytes: &[u8], max_len: usize) -> Result<T>
where
    T: serde::de::DeserializeOwned + Validate,
{
    if bytes.len() > max_len {
        return Err(ProtocolError::TooLarge(bytes.len()));
    }
    let msg: T = serde_json::from_slice(bytes).map_err(|e| ProtocolError::Malformed(e.to_string()))?;
    msg.validate()?;
    Ok(msg)
}

pub fn encode<T: serde::Serialize>(msg: &T) -> String {
    // Serialization of our own plain data types cannot fail.
    serde_json::to_string(msg).expect("protocol types always serialize")
}

pub(crate) fn check_len(field: &'static str, s: &str, max: usize) -> Result<()> {
    if s.len() > max {
        return Err(invalid(field, format!("longer than {max} bytes")));
    }
    Ok(())
}

pub(crate) fn check_printable(field: &'static str, s: &str, max: usize) -> Result<()> {
    check_len(field, s, max)?;
    if s.chars().any(|c| c.is_control()) {
        return Err(invalid(field, "contains control characters"));
    }
    Ok(())
}
