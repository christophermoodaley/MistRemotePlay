//! System-audio capture interface.
//!
//! Implementations live in the media module (they are GStreamer device
//! providers); this module holds the backend-neutral types so session and UI
//! code never depend on the media framework.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AudioKind {
    /// WASAPI loopback of a render endpoint.
    WasapiLoopback,
    /// Synthetic tone, for tests.
    TestTone,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AudioDeviceInfo {
    /// Backend-specific device identifier (`None` = system default).
    pub id: Option<String>,
    pub name: String,
    pub kind: AudioKind,
    pub is_default: bool,
}

/// What the media module should capture.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum AudioSourceSpec {
    WasapiLoopback { device: Option<String> },
    TestTone { frequency_hz: u32 },
}

impl AudioSourceSpec {
    pub fn kind(&self) -> AudioKind {
        match self {
            AudioSourceSpec::WasapiLoopback { .. } => AudioKind::WasapiLoopback,
            AudioSourceSpec::TestTone { .. } => AudioKind::TestTone,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum AudioError {
    #[error("no system-audio capture backend is available: {0}")]
    Unavailable(String),
    #[error("audio device error: {0}")]
    Device(String),
}

pub trait AudioCaptureBackend: Send + Sync {
    fn kind(&self) -> AudioKind;
    /// Loopback-capable devices (playback monitors), default first.
    fn devices(&self) -> Result<Vec<AudioDeviceInfo>, AudioError>;
    fn source(&self, device: Option<&str>) -> Result<AudioSourceSpec, AudioError>;
}
