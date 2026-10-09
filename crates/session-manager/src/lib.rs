//! Session-level policy, independent of media and transport:
//! pairing codes, trusted devices, the session state machine, reconnect
//! backoff and adaptive bitrate.

pub mod bitrate;
pub mod pairing;
pub mod reconnect;
pub mod session;
pub mod trust;

use mist_protocol::identity::DeviceId;
use serde::Serialize;

pub use session::{EndReason, Session, SessionState};

/// Snapshot of a session for UIs and logs.
#[derive(Clone, Debug, Serialize)]
pub struct SessionInfo {
    pub session_id: String,
    pub client_id: DeviceId,
    pub client_name: String,
    pub state: SessionState,
    pub input_allowed: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    #[error("no such session")]
    NotFound,
    #[error(transparent)]
    Transition(#[from] session::TransitionError),
    #[error("{0}")]
    Other(String),
}

/// Host-side control surface implemented by the application core and used
/// by the UI/CLI. Every privileged change goes through here.
pub trait SessionManager: Send + Sync {
    fn sessions(&self) -> Vec<SessionInfo>;
    /// Explicit host consent for a pending session request.
    fn approve(&self, session_id: &str) -> Result<(), SessionError>;
    fn reject(&self, session_id: &str, reason: &str) -> Result<(), SessionError>;
    fn end(&self, session_id: &str) -> Result<(), SessionError>;
    /// Immediately enable/disable remote input for all sessions (held keys
    /// and buttons are released when disabling).
    fn set_input_allowed(&self, allowed: bool);
    fn input_allowed(&self) -> bool;
    /// Emergency stop: end every session now.
    fn disconnect_all(&self);
}
