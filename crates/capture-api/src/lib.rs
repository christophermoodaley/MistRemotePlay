//! Screen capture backend interface.
//!
//! A [`CaptureBackend`] discovers displays and turns a selected display into
//! a [`CaptureSourceSpec`]: a backend-neutral description that the media
//! engine turns into a concrete pipeline. Backends are detected at runtime;
//! no single capture API is assumed to work on every display server.

use serde::{Deserialize, Serialize};
use std::sync::Arc;

pub mod test_pattern;
#[cfg(target_os = "linux")]
pub mod x11;
#[cfg(all(target_os = "linux", feature = "wayland-portal"))]
pub mod wayland;
#[cfg(windows)]
pub mod windows;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptureKind {
    X11,
    WaylandPortal,
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

/// Something the capture source owns that must outlive the pipeline (e.g. a
/// portal session or PipeWire file descriptor).
pub trait CaptureLease: Send + Sync + std::fmt::Debug {}

#[derive(Clone, Debug)]
pub enum CaptureSourceSpec {
    /// X11 screen region via XGetImage/XShm (`ximagesrc`).
    X11 { display_name: Option<String>, x: i32, y: i32, width: u32, height: u32, show_cursor: bool },
    /// PipeWire stream negotiated through xdg-desktop-portal.
    PipeWire { fd: i32, node_id: u32, width: Option<u32>, height: Option<u32>, lease: Arc<dyn CaptureLease> },
    /// DXGI Desktop Duplication / Windows Graphics Capture via `d3d11screencapturesrc`.
    D3D11 { monitor_handle: u64, width: u32, height: u32, show_cursor: bool },
    /// Synthetic moving pattern; for tests and environments without a display.
    TestPattern { width: u32, height: u32, pattern: String },
}

impl CaptureSourceSpec {
    pub fn native_size(&self) -> Option<(u32, u32)> {
        match self {
            CaptureSourceSpec::X11 { width, height, .. }
            | CaptureSourceSpec::D3D11 { width, height, .. }
            | CaptureSourceSpec::TestPattern { width, height, .. } => Some((*width, *height)),
            CaptureSourceSpec::PipeWire { width: Some(w), height: Some(h), .. } => Some((*w, *h)),
            CaptureSourceSpec::PipeWire { .. } => None,
        }
    }

    pub fn kind(&self) -> CaptureKind {
        match self {
            CaptureSourceSpec::X11 { .. } => CaptureKind::X11,
            CaptureSourceSpec::PipeWire { .. } => CaptureKind::WaylandPortal,
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
    #[allow(unused_mut)]
    let mut out: Vec<Box<dyn CaptureBackend>> = Vec::new();
    #[cfg(target_os = "linux")]
    {
        let wayland = std::env::var_os("WAYLAND_DISPLAY").is_some()
            || std::env::var("XDG_SESSION_TYPE").map(|s| s == "wayland").unwrap_or(false);
        #[cfg(feature = "wayland-portal")]
        if wayland {
            out.push(Box::new(wayland::PortalCapture::default()));
        }
        // Under Wayland, X11 capture only sees XWayland windows, so it is
        // offered only when no Wayland session is detected.
        if !wayland {
            match x11::X11Capture::connect(None) {
                Ok(b) => out.push(Box::new(b)),
                Err(e) => tracing::info!("X11 capture unavailable: {e}"),
            }
        }
    }
    #[cfg(windows)]
    out.push(Box::new(windows::DxgiCapture));
    out
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
