//! Screen capture backend interface.
//!
//! A [`CaptureBackend`] discovers displays and turns a selected display into
//! a [`CaptureSourceSpec`]: a backend-neutral description that the media
//! engine turns into a concrete pipeline.

use serde::{Deserialize, Serialize};

pub mod test_pattern;
pub mod windows;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptureKind {
    WindowsDxgi,
    TestPattern,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DisplayInfo {
    /// Stable within one backend for the lifetime of the display layout.
    pub id: String,
    pub name: String,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub primary: bool,
    pub backend: CaptureKind,
}

#[derive(Clone, Debug)]
pub enum CaptureSourceSpec {
    /// DXGI Desktop Duplication / Windows Graphics Capture via `d3d11screencapturesrc`.
    D3D11 { monitor_handle: u64, width: u32, height: u32, show_cursor: bool },
    /// Synthetic moving pattern; for tests and environments without a display.
    TestPattern { width: u32, height: u32, pattern: String },
}

impl CaptureSourceSpec {
    pub fn native_size(&self) -> Option<(u32, u32)> {
        match self {
            CaptureSourceSpec::D3D11 { width, height, .. } | CaptureSourceSpec::TestPattern { width, height, .. } => {
                Some((*width, *height))
            }
        }
    }

    pub fn kind(&self) -> CaptureKind {
        match self {
            CaptureSourceSpec::D3D11 { .. } => CaptureKind::WindowsDxgi,
            CaptureSourceSpec::TestPattern { .. } => CaptureKind::TestPattern,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum CaptureError {
    #[error("capture backend unavailable: {0}")]
    Unavailable(String),
    #[error("display `{0}` not found")]
    UnknownDisplay(String),
    #[error("user cancelled the screen-sharing request")]
    Cancelled,
    #[error("capture backend error: {0}")]
    Backend(String),
}

pub type Result<T> = std::result::Result<T, CaptureError>;

pub trait CaptureBackend: Send + Sync {
    fn kind(&self) -> CaptureKind;
    /// Enumerate displays. Portal backends return a single "ask the user"
    /// entry because the compositor's picker selects the monitor.
    fn displays(&self) -> Result<Vec<DisplayInfo>>;
    /// Prepare capture of `display_id`. May block on a user prompt (portal).
    fn open(&self, display_id: &str) -> Result<CaptureSourceSpec>;
}

/// Backends usable in the current session, best first.
pub fn detect_backends() -> Vec<Box<dyn CaptureBackend>> {
    vec![Box::new(windows::DxgiCapture)]
}

/// Find a backend by kind (including the test pattern).
pub fn backend_for(kind: CaptureKind) -> Result<Box<dyn CaptureBackend>> {
    if kind == CaptureKind::TestPattern {
        return Ok(Box::new(test_pattern::TestPatternCapture::default()));
    }
    detect_backends()
        .into_iter()
        .find(|b| b.kind() == kind)
        .ok_or_else(|| CaptureError::Unavailable(format!("{kind:?} capture is not available in this session")))
}
