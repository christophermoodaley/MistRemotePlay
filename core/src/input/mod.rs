//! Remote keyboard/mouse input on the host.
//!
//! [`dispatcher::InputDispatcher`] is the only path from the network to an
//! [`InputBackend`]: it decodes, validates and rate-limits messages, enforces
//! the host's input permission, tracks held keys/buttons and releases them on
//! disconnect or revocation. Backends only translate already-validated events
//! into OS injection calls.

use crate::protocol::input::MouseButton;
use crate::protocol::keys::KeyInfo;
use serde::Serialize;

pub mod dispatcher;
pub mod windows;

pub use dispatcher::{ClientId, DispatchOutcome, InputDispatcher, InputPermissions, InputStats};

#[derive(Debug, thiserror::Error)]
pub enum InputError {
    #[error("input backend unavailable: {0}")]
    Unavailable(String),
    #[error("input injection failed: {0}")]
    Injection(String),
}

pub type Result<T> = std::result::Result<T, InputError>;

/// Host desktop rectangle that absolute coordinates map onto (the captured
/// display).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct TargetRect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

impl TargetRect {
    /// Map normalized 0..=65535 coordinates to desktop pixels.
    pub fn map(&self, nx: u16, ny: u16) -> (i32, i32) {
        let px = self.x + ((nx as u64 * (self.width.max(1) as u64 - 1)) / 65535) as i32;
        let py = self.y + ((ny as u64 * (self.height.max(1) as u64 - 1)) / 65535) as i32;
        (px, py)
    }
}

/// OS-specific injection of validated keyboard/mouse events.
pub trait InputBackend: Send {
    fn name(&self) -> &'static str;
    /// Whether the client's autorepeat events should be injected (true when
    /// the OS does not autorepeat injected keys itself).
    fn wants_client_repeat(&self) -> bool {
        false
    }
    fn key(&mut self, key: &'static KeyInfo, down: bool) -> Result<()>;
    fn mouse_move_relative(&mut self, dx: i32, dy: i32) -> Result<()>;
    fn mouse_move_absolute(&mut self, x: u16, y: u16) -> Result<()>;
    fn mouse_button(&mut self, button: MouseButton, down: bool) -> Result<()>;
    /// Units of 1/120 notch.
    fn scroll(&mut self, dx: i32, dy: i32) -> Result<()>;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MockInputEvent {
    Key(u16, bool),
    MoveRel(i32, i32),
    MoveAbs(u16, u16),
    Button(MouseButton, bool),
    Scroll(i32, i32),
}

/// Safe test backend: records events, injects nothing.
#[derive(Clone, Default)]
pub struct MockInput {
    pub log: std::sync::Arc<crate::controller::recorder::Recorder<MockInputEvent>>,
}

impl InputBackend for MockInput {
    fn name(&self) -> &'static str {
        "mock"
    }
    fn key(&mut self, key: &'static KeyInfo, down: bool) -> Result<()> {
        self.log.push(MockInputEvent::Key(key.hid, down));
        Ok(())
    }
    fn mouse_move_relative(&mut self, dx: i32, dy: i32) -> Result<()> {
        self.log.push(MockInputEvent::MoveRel(dx, dy));
        Ok(())
    }
    fn mouse_move_absolute(&mut self, x: u16, y: u16) -> Result<()> {
        self.log.push(MockInputEvent::MoveAbs(x, y));
        Ok(())
    }
    fn mouse_button(&mut self, button: MouseButton, down: bool) -> Result<()> {
        self.log.push(MockInputEvent::Button(button, down));
        Ok(())
    }
    fn scroll(&mut self, dx: i32, dy: i32) -> Result<()> {
        self.log.push(MockInputEvent::Scroll(dx, dy));
        Ok(())
    }
}

/// Which keyboard/mouse backend to use.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InputBackendKind {
    /// Pick the best available for this session.
    Auto,
    Mock,
    Win32SendInput,
}

/// Create a keyboard/mouse backend (`Auto` = SendInput).
pub fn create_backend(kind: InputBackendKind, target: TargetRect) -> Result<Box<dyn InputBackend>> {
    match kind {
        InputBackendKind::Mock => Ok(Box::new(MockInput::default())),
        InputBackendKind::Win32SendInput | InputBackendKind::Auto => {
            Ok(Box::new(windows::SendInputBackend::new(target)))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn target_rect_maps_edges() {
        let r = TargetRect { x: 1920, y: 0, width: 2560, height: 1440 };
        assert_eq!(r.map(0, 0), (1920, 0));
        assert_eq!(r.map(65535, 65535), (1920 + 2559, 1439));
        assert_eq!(r.map(32768, 32768).0, 1920 + 1279);
    }
}
