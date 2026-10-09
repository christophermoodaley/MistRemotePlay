//! Media-framework-neutral view of a WebRTC peer connection.

use mist_protocol::peer::SdpKind;
use mist_protocol::signaling::IceServer;
use serde::Serialize;
use tokio::sync::{mpsc, oneshot};

use crate::stats::TransportStats;
use crate::Result;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DataChannelKind {
    /// Reliable, ordered: clock sync, stats, permissions.
    Control,
    /// Reliable, ordered: keyboard and mouse.
    Input,
    /// Unordered, no retransmits: full gamepad state snapshots.
    Gamepad,
}

impl DataChannelKind {
    pub const ALL: [DataChannelKind; 3] = [DataChannelKind::Control, DataChannelKind::Input, DataChannelKind::Gamepad];

    pub fn label(self) -> &'static str {
        match self {
            DataChannelKind::Control => "control",
            DataChannelKind::Input => "input",
            DataChannelKind::Gamepad => "gamepad",
        }
    }

    pub fn from_label(label: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|k| k.label() == label)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnectionState {
    New,
    Connecting,
    Connected,
    Disconnected,
    Failed,
    Closed,
}

#[derive(Debug)]
pub enum TransportEvent {
    LocalSdp {
        kind: SdpKind,
        sdp: String,
    },
    LocalCandidate {
        mline_index: u32,
        candidate: String,
    },
    IceGatheringComplete,
    IceState(ConnectionState),
    PeerState(ConnectionState),
    ChannelOpen(DataChannelKind),
    ChannelClosed(DataChannelKind),
    ChannelMessage(DataChannelKind, String),
    /// Non-fatal media problem (e.g. audio device lost); stream continues.
    Warning(String),
    /// Fatal pipeline error.
    Error(String),
}

pub type EventSender = mpsc::UnboundedSender<TransportEvent>;

#[derive(Clone, Debug, Default)]
pub struct IceConfig {
    pub servers: Vec<IceServer>,
    /// Force TURN relay (ICE transport policy "relay").
    pub relay_only: bool,
}

/// Operations the session layer needs from a peer connection. Implemented
/// by the GStreamer host/client media sessions and by test doubles.
pub trait MediaTransport: Send + Sync {
    fn set_remote_sdp(&self, kind: SdpKind, sdp: &str) -> Result<()>;
    fn add_remote_candidate(&self, mline_index: u32, candidate: &str) -> Result<()>;
    fn send(&self, channel: DataChannelKind, text: &str) -> Result<()>;
    fn request_stats(&self) -> oneshot::Receiver<TransportStats>;
    fn close(&self);
}
