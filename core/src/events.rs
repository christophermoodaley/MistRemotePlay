//! Events published to UIs (desktop app, CLI JSON output). They reflect
//! real state transitions; nothing here is synthesized.

use crate::media::TransportStats;
use crate::protocol::control::{ClientStats, HostStats};
use crate::protocol::settings::StreamSettings;
use crate::sessions::{SessionRole, SessionState};
use serde::Serialize;

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum HostEvent {
    Signaling {
        connected: bool,
        detail: String,
    },
    PairingCode {
        code: String,
        expires_in_secs: u64,
    },
    PairingCodeExpired,
    PairingRequest {
        request_id: String,
        client_name: String,
        fingerprint: String,
    },
    Paired {
        client_name: String,
        device_id: String,
    },
    PairingRejected {
        client_name: String,
        reason: String,
    },
    SessionRequest {
        session_id: String,
        client_name: String,
        fingerprint: String,
        settings: StreamSettings,
    },
    Session {
        session_id: String,
        client_name: String,
        state: SessionState,
    },
    /// Host-wide remote input switch.
    InputPermission {
        allowed: bool,
    },
    /// One person's role or keyboard/mouse permission changed.
    Permissions {
        session_id: String,
        client_name: String,
        role: SessionRole,
        keyboard_mouse: bool,
    },
    Stats(Box<HostStatsEvent>),
    /// This network's public address was found (built-in server only).
    PublicAddress {
        ip: String,
    },
    /// The built-in server's port was opened on the router, or could not be.
    PortMapping {
        mapping: crate::upnp::PortMapping,
    },
    Warning {
        message: String,
    },
    Error {
        message: String,
    },
}

/// Statistics of one person's session. `host` and `encoder_hardware`
/// describe the stream everyone shares.
#[derive(Clone, Debug, Serialize)]
pub struct HostStatsEvent {
    pub session_id: String,
    pub client_name: String,
    pub host: HostStats,
    pub client: Option<ClientStats>,
    pub transport: TransportStats,
    pub encoder_hardware: bool,
    pub input_backend: String,
    pub controller_backend: String,
    /// This person's input counters.
    pub input: crate::input::InputStats,
    /// Host controller slots (0-based) this person's gamepads occupy.
    pub controllers: Vec<u8>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum ClientEvent {
    State {
        state: ClientState,
        detail: String,
    },
    Paired {
        host_name: String,
        device_id: String,
    },
    Stats(Box<ClientStatsEvent>),
    Controllers {
        controllers: Vec<crate::controller::source::ConnectedController>,
    },
    InputPermission {
        allowed: bool,
    },
    /// The host's game sent force feedback to the controller in `index`.
    Rumble {
        index: u8,
        strong: u16,
        weak: u16,
        duration_ms: u16,
    },
    Warning {
        message: String,
    },
    Error {
        message: String,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ClientState {
    ConnectingSignaling,
    WaitingForApproval,
    Negotiating,
    Streaming,
    Reconnecting,
    Ended,
    Failed,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct ClientStatsEvent {
    pub stats: ClientStats,
    /// Clock-sync round trip over the control data channel.
    pub rtt_ms: Option<f32>,
    /// Error bound of `stats.latency_ms` from clock-offset estimation.
    pub latency_uncertainty_ms: Option<f32>,
    pub host: Option<HostStats>,
    pub transport: TransportStats,
    pub video_bitrate_kbps: Option<f32>,
    pub decoder: String,
    pub relayed: Option<bool>,
    /// Where `stats.latency_ms` is spent.
    pub latency_breakdown: crate::client::shared::LatencyBreakdown,
}
