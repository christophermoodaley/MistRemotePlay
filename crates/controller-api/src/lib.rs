//! Controller subsystem.
//!
//! * Client side: [`source::ControllerSource`] discovers physical controllers
//!   (gilrs: XInput/Windows.Gaming.Input on Windows, evdev on Linux), assigns
//!   them to slots 0..4 and normalizes their state.
//! * Host side: a [`ControllerBackend`] creates virtual controllers and
//!   applies state updates (uinput on Linux, ViGEmBus on Windows).

use mist_protocol::input::{GamepadKind, GamepadState};

pub mod normalize;
#[cfg(feature = "gilrs")]
pub mod source;
#[cfg(target_os = "linux")]
pub mod uinput;
#[cfg(windows)]
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

/// Host-side virtual controller backend.
pub trait ControllerBackend: Send {
    fn name(&self) -> &'static str;
    fn plug(&mut self, index: u8, kind: GamepadKind) -> Result<()>;
    fn update(&mut self, index: u8, state: &GamepadState) -> Result<()>;
    fn unplug(&mut self, index: u8) -> Result<()>;
}

/// Records calls; used in tests and the safe `--mock-input` mode.
#[derive(Debug, Default, Clone)]
pub struct MockController {
    pub log: std::sync::Arc<recorder::Recorder<MockPadEvent>>,
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
        self.log.push(MockPadEvent::Plug(index, kind));
        Ok(())
    }
    fn update(&mut self, index: u8, state: &GamepadState) -> Result<()> {
        self.log.push(MockPadEvent::Update(index, *state));
        Ok(())
    }
    fn unplug(&mut self, index: u8) -> Result<()> {
        self.log.push(MockPadEvent::Unplug(index));
        Ok(())
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
    #[cfg(target_os = "linux")]
    {
        match uinput::UinputGamepads::probe() {
            Ok(()) => return Box::new(uinput::UinputGamepads::default()),
            Err(e) => return Box::new(UnavailableController(e.to_string())),
        }
    }
    #[cfg(windows)]
    {
        match vigem::VigemGamepads::connect() {
            Ok(b) => return Box::new(b),
            Err(e) => return Box::new(UnavailableController(e.to_string())),
        }
    }
    #[allow(unreachable_code)]
    Box::new(UnavailableController("no virtual controller backend for this platform".into()))
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
