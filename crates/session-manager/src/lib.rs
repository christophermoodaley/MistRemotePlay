//! Session-level policy, independent of media and transport:
//! pairing codes, trusted devices, the session state machine, reconnect
//! backoff and adaptive bitrate.

pub mod bitrate;
pub mod pairing;
pub mod reconnect;
pub mod session;
pub mod trust;

use mist_protocol::identity::DeviceId;
use serde::{Deserialize, Serialize};

pub use session::{EndReason, Session, SessionState};

/// What a connected person does in a hosting session; the host decides.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionRole {
    /// Sees the stream and controls the game with gamepads (and keyboard
    /// and mouse, if the host allows it).
    #[default]
    Play,
    /// Sees and hears the stream only.
    Watch,
}

/// Snapshot of a session for UIs and logs.
#[derive(Clone, Debug, Serialize)]
pub struct SessionInfo {
    pub session_id: String,
    pub client_id: DeviceId,
    pub client_name: String,
    pub state: SessionState,
    /// Whether this person's input reaches the host right now (host-wide
    /// switch and role combined).
    pub input_allowed: bool,
    pub role: SessionRole,
    /// Keyboard and mouse allowed for this person (players only).
    pub keyboard_mouse: bool,
    /// Host controller slots (0-based) this person's gamepads occupy.
    pub controllers: Vec<u8>,
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
    /// Explicit host consent for a pending session request, as a player.
    fn approve(&self, session_id: &str) -> Result<(), SessionError> {
        self.approve_as(session_id, SessionRole::Play)
    }
    /// Explicit host consent for a pending session request.
    fn approve_as(&self, session_id: &str, role: SessionRole) -> Result<(), SessionError>;
    fn reject(&self, session_id: &str, reason: &str) -> Result<(), SessionError>;
    fn end(&self, session_id: &str) -> Result<(), SessionError>;
    /// Switch a connected person between playing and watching. Becoming a
    /// watcher releases their held input and removes their controllers.
    fn set_role(&self, session_id: &str, role: SessionRole) -> Result<(), SessionError>;
    /// Allow or deny one player's keyboard and mouse.
    fn set_keyboard_mouse(&self, session_id: &str, allowed: bool) -> Result<(), SessionError>;
    /// Immediately enable/disable remote input for all sessions (held keys
    /// and buttons are released when disabling).
    fn set_input_allowed(&self, allowed: bool);
    fn input_allowed(&self) -> bool;
    /// Emergency stop: end every session now.
    fn disconnect_all(&self);
}
