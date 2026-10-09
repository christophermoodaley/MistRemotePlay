//! Windows virtual Xbox 360 controllers through the ViGEmBus driver.
//! The driver must be installed separately (see docs/windows-setup.md).
//! XInput limits triggers to 8 bits, so trigger precision is reduced here.

use std::sync::Arc;

use mist_protocol::input::{buttons, GamepadKind, GamepadState, MAX_GAMEPADS};
use vigem_client::{Client, TargetId, XButtons, XGamepad, Xbox360Wired};

use crate::{ControllerBackend, ControllerError, Result};

pub struct VigemGamepads {
    client: Arc<Client>,
    pads: Vec<Option<Xbox360Wired<Arc<Client>>>>,
}

fn err(e: vigem_client::Error) -> ControllerError {
    ControllerError::Backend(format!("ViGEm: {e:?}"))
}

impl VigemGamepads {
    pub fn connect() -> Result<Self> {
        let client = Client::connect()
            .map_err(|e| ControllerError::Unavailable(format!("ViGEmBus driver not available ({e:?}); install it to enable controllers")))?;
        Ok(VigemGamepads { client: Arc::new(client), pads: (0..MAX_GAMEPADS).map(|_| None).collect() })
    }
}

pub(crate) fn to_xgamepad(s: &GamepadState) -> XGamepad {
    const MAP: [(u32, u16); 15] = [
        (buttons::A, XButtons::A),
        (buttons::B, XButtons::B),
        (buttons::X, XButtons::X),
        (buttons::Y, XButtons::Y),
        (buttons::LEFT_SHOULDER, XButtons::LB),
        (buttons::RIGHT_SHOULDER, XButtons::RB),
        (buttons::BACK, XButtons::BACK),
        (buttons::START, XButtons::START),
        (buttons::GUIDE, XButtons::GUIDE),
        (buttons::LEFT_STICK, XButtons::LTHUMB),
        (buttons::RIGHT_STICK, XButtons::RTHUMB),
        (buttons::DPAD_UP, XButtons::UP),
        (buttons::DPAD_DOWN, XButtons::DOWN),
        (buttons::DPAD_LEFT, XButtons::LEFT),
        (buttons::DPAD_RIGHT, XButtons::RIGHT),
    ];
    let raw = MAP.iter().filter(|(b, _)| s.buttons & b != 0).fold(0u16, |acc, (_, x)| acc | x);
    XGamepad {
        buttons: XButtons { raw },
        left_trigger: (s.left_trigger >> 8) as u8,
        right_trigger: (s.right_trigger >> 8) as u8,
        // XInput is also up-positive.
        thumb_lx: s.left_x,
        thumb_ly: s.left_y,
        thumb_rx: s.right_x,
        thumb_ry: s.right_y,
    }
}

impl ControllerBackend for VigemGamepads {
    fn name(&self) -> &'static str {
        "vigem"
    }

    fn plug(&mut self, index: u8, _kind: GamepadKind) -> Result<()> {
        let slot = self.pads.get_mut(index as usize).ok_or(ControllerError::NotConnected(index))?;
        if slot.is_none() {
            let mut target = Xbox360Wired::new(self.client.clone(), TargetId::XBOX360_WIRED);
            target.plugin().map_err(err)?;
            target.wait_ready().map_err(err)?;
            *slot = Some(target);
            tracing::info!(index, "virtual gamepad created (ViGEm)");
        }
        Ok(())
    }

    fn update(&mut self, index: u8, state: &GamepadState) -> Result<()> {
        let target = self.pads.get_mut(index as usize).and_then(|s| s.as_mut()).ok_or(ControllerError::NotConnected(index))?;
        target.update(&to_xgamepad(state)).map_err(err)
    }

    fn unplug(&mut self, index: u8) -> Result<()> {
        if let Some(mut target) = self.pads.get_mut(index as usize).and_then(|s| s.take()) {
            let _ = target.update(&XGamepad::default());
            target.unplug().map_err(err)?;
        }
        Ok(())
    }
}
