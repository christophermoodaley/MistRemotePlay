//! Controller subsystem.
//!
//! * Client side: [`source::ControllerSource`] discovers physical controllers
//!   (gilrs: XInput/Windows.Gaming.Input), assigns them to slots 0..4 and
//!   normalizes their state.
//! * Host side: a [`ControllerBackend`] creates virtual controllers and
//!   applies state updates (ViGEmBus).

use crate::protocol::input::{GamepadKind, GamepadState};

pub mod normalize;
pub mod source;
pub mod vigem;

pub use normalize::{ControllerMapping, RawGamepad};

#[derive(Debug, thiserror::Error)]
pub enum ControllerError {
    #[error("virtual controllers unavailable: {0}")]
    Unavailable(String),
    #[error("controller backend error: {0}")]
    Backend(String),
    #[error("controller slot {0} is not connected")]
    NotConnected(u8),
}

pub type Result<T> = std::result::Result<T, ControllerError>;

/// Force feedback a game sent to a virtual controller.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rumble {
    pub index: u8,
    /// Heavy (low-frequency) motor, 0..=65535.
    pub strong: u16,
    /// Light (high-frequency) motor, 0..=65535.
    pub weak: u16,
    /// 0 = until the next rumble for this pad.
    pub duration_ms: u16,
}

/// Host-side virtual controller backend.
pub trait ControllerBackend: Send {
    fn name(&self) -> &'static str;
    fn plug(&mut self, index: u8, kind: GamepadKind) -> Result<()>;
    fn update(&mut self, index: u8, state: &GamepadState) -> Result<()>;
    fn unplug(&mut self, index: u8) -> Result<()>;
    /// Rumble requests from games since the last call. Called every few
    /// milliseconds; backends must not block.
    fn poll_feedback(&mut self) -> Vec<Rumble> {
        Vec::new()
    }
}

/// Records calls; used in tests and the safe `--controllers mock` mode.
///
/// To exercise the force-feedback path end to end without hardware, the
/// mock echoes trigger positions back as rumble (left trigger -> strong
/// motor, right trigger -> weak motor) whenever they change.
#[derive(Debug, Default, Clone)]
pub struct MockController {
    pub log: std::sync::Arc<recorder::Recorder<MockPadEvent>>,
    last: [Option<GamepadState>; crate::protocol::input::MAX_GAMEPADS as usize],
    feedback: Vec<Rumble>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MockPadEvent {
    Plug(u8, GamepadKind),
    Update(u8, GamepadState),
    Unplug(u8),
}

impl ControllerBackend for MockController {
    fn name(&self) -> &'static str {
        "mock"
    }
    fn plug(&mut self, index: u8, kind: GamepadKind) -> Result<()> {
        tracing::info!(index, ?kind, "mock gamepad plugged");
        self.log.push(MockPadEvent::Plug(index, kind));
        if let Some(slot) = self.last.get_mut(index as usize) {
            *slot = Some(GamepadState::default());
        }
        Ok(())
    }
    fn update(&mut self, index: u8, state: &GamepadState) -> Result<()> {
        self.log.push(MockPadEvent::Update(index, *state));
        let Some(Some(prev)) = self.last.get_mut(index as usize) else {
            return Err(ControllerError::NotConnected(index));
        };
        if (prev.left_trigger, prev.right_trigger) != (state.left_trigger, state.right_trigger) {
            self.feedback.push(Rumble { index, strong: state.left_trigger, weak: state.right_trigger, duration_ms: 0 });
        }
        *prev = *state;
        Ok(())
    }
    fn unplug(&mut self, index: u8) -> Result<()> {
        let last = self.last.get_mut(index as usize).and_then(|s| s.take());
        tracing::info!(index, neutral = last == Some(GamepadState::default()), "mock gamepad unplugged");
        self.log.push(MockPadEvent::Unplug(index));
        Ok(())
    }
    fn poll_feedback(&mut self) -> Vec<Rumble> {
        std::mem::take(&mut self.feedback)
    }
}

/// Used when no virtual controller support exists; reports why.
pub struct UnavailableController(pub String);

impl ControllerBackend for UnavailableController {
    fn name(&self) -> &'static str {
        "unavailable"
    }
    fn plug(&mut self, _: u8, _: GamepadKind) -> Result<()> {
        Err(ControllerError::Unavailable(self.0.clone()))
    }
    fn update(&mut self, index: u8, _: &GamepadState) -> Result<()> {
        Err(ControllerError::NotConnected(index))
    }
    fn unplug(&mut self, _: u8) -> Result<()> {
        Ok(())
    }
}

/// Best available virtual-controller backend on this host.
pub fn platform_backend() -> Box<dyn ControllerBackend> {
    match vigem::VigemGamepads::connect() {
        Ok(b) => Box::new(b),
        Err(e) => Box::new(UnavailableController(e.to_string())),
    }
}

/// Thread-safe append-only event recorder for mocks.
pub mod recorder {
    use std::sync::Mutex;

    #[derive(Debug)]
    pub struct Recorder<T>(Mutex<Vec<T>>);

    impl<T> Default for Recorder<T> {
        fn default() -> Self {
            Recorder(Mutex::new(Vec::new()))
        }
    }

    impl<T: Clone> Recorder<T> {
        pub fn push(&self, v: T) {
            self.0.lock().unwrap_or_else(|e| e.into_inner()).push(v);
        }
        pub fn snapshot(&self) -> Vec<T> {
            self.0.lock().unwrap_or_else(|e| e.into_inner()).clone()
        }
        pub fn clear(&self) {
            self.0.lock().unwrap_or_else(|e| e.into_inner()).clear();
        }
    }
}
