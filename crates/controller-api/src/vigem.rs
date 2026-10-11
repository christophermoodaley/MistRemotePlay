//! Windows virtual Xbox 360 controllers through the ViGEmBus driver.
//! The driver must be installed separately (see docs/windows-setup.md).
//! XInput limits triggers to 8 bits, so trigger precision is reduced here.

use std::sync::{mpsc, Arc};
use std::thread::JoinHandle;

use mist_protocol::input::{buttons, GamepadKind, GamepadState, MAX_GAMEPADS};
use vigem_client::{Client, TargetId, XButtons, XGamepad, Xbox360Wired};

use crate::{ControllerBackend, ControllerError, Result, Rumble};

struct Pad {
    target: Xbox360Wired<Arc<Client>>,
    /// Receives XInput rumble notifications; exits when the target is unplugged.
    notifications: Option<JoinHandle<()>>,
}

pub struct VigemGamepads {
    client: Arc<Client>,
    pads: Vec<Option<Pad>>,
    rumble_tx: mpsc::Sender<Rumble>,
    rumble_rx: mpsc::Receiver<Rumble>,
}

fn err(e: vigem_client::Error) -> ControllerError {
    ControllerError::Backend(format!("ViGEm: {e:?}"))
}

impl VigemGamepads {
    pub fn connect() -> Result<Self> {
        let client = Client::connect().map_err(|e| {
            ControllerError::Unavailable(format!(
                "ViGEmBus driver not available ({e:?}); install it to enable controllers"
            ))
        })?;
        let (rumble_tx, rumble_rx) = mpsc::channel();
        Ok(VigemGamepads {
            client: Arc::new(client),
            pads: (0..MAX_GAMEPADS).map(|_| None).collect(),
            rumble_tx,
            rumble_rx,
        })
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
            // XInput motor levels (0..=255) persist until the game changes them.
            let tx = self.rumble_tx.clone();
            let notifications = match target.request_notification() {
                Ok(req) => Some(req.spawn_thread(move |_, n| {
                    let _ = tx.send(Rumble {
                        index,
                        strong: n.large_motor as u16 * 257,
                        weak: n.small_motor as u16 * 257,
                        duration_ms: 0,
                    });
                })),
                Err(e) => {
                    tracing::warn!(index, "rumble unavailable for this virtual pad: {e:?}");
                    None
                }
            };
            *slot = Some(Pad { target, notifications });
            tracing::info!(index, "virtual gamepad created (ViGEm)");
        }
        Ok(())
    }

    fn update(&mut self, index: u8, state: &GamepadState) -> Result<()> {
        let pad =
            self.pads.get_mut(index as usize).and_then(|s| s.as_mut()).ok_or(ControllerError::NotConnected(index))?;
        pad.target.update(&to_xgamepad(state)).map_err(err)
    }

    fn unplug(&mut self, index: u8) -> Result<()> {
        if let Some(Pad { mut target, notifications }) = self.pads.get_mut(index as usize).and_then(|s| s.take()) {
            let _ = target.update(&XGamepad::default());
            let result = target.unplug().map_err(err);
            // Unplugging aborts the pending notification request.
            drop(target);
            if let Some(t) = notifications {
                let _ = t.join();
            }
            result?;
        }
        Ok(())
    }

    fn poll_feedback(&mut self) -> Vec<Rumble> {
        self.rumble_rx.try_iter().collect()
    }
}
