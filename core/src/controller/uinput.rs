//! Linux virtual gamepads through /dev/uinput.
//!
//! Each slot is a separate uinput device using the Linux gamepad event
//! conventions (BTN_SOUTH.., ABS_X/Y/RX/RY sticks, ABS_Z/RZ triggers,
//! ABS_HAT0X/Y d-pad). The device reports the USB vendor/product IDs of a
//! common wired Xbox-style controller so SDL and games apply their standard
//! mapping; the device *name* is our own. Requires write access to
//! /dev/uinput (see the README, Linux and Raspberry Pi).

use std::collections::HashMap;
use std::os::fd::AsRawFd;

use crate::protocol::input::{buttons, GamepadKind, GamepadState, MAX_GAMEPADS};
use evdev::uinput::VirtualDevice;
use evdev::{
    AbsInfo, AbsoluteAxisCode, AttributeSet, BusType, EventSummary, EventType, FFEffectCode, FFEffectKind, InputEvent,
    InputId, KeyCode, UInputCode, UinputAbsSetup,
};

use crate::controller::{ControllerBackend, ControllerError, Result, Rumble};

/// Effects a game may upload per virtual pad.
const MAX_FF_EFFECTS: u16 = 16;

const VENDOR: u16 = 0x045e;
const PRODUCT: u16 = 0x028e;

const BUTTON_CODES: [(u32, KeyCode); 11] = [
    (buttons::A, KeyCode::BTN_SOUTH),
    (buttons::B, KeyCode::BTN_EAST),
    (buttons::X, KeyCode::BTN_NORTH),
    (buttons::Y, KeyCode::BTN_WEST),
    (buttons::LEFT_SHOULDER, KeyCode::BTN_TL),
    (buttons::RIGHT_SHOULDER, KeyCode::BTN_TR),
    (buttons::BACK, KeyCode::BTN_SELECT),
    (buttons::START, KeyCode::BTN_START),
    (buttons::GUIDE, KeyCode::BTN_MODE),
    (buttons::LEFT_STICK, KeyCode::BTN_THUMBL),
    (buttons::RIGHT_STICK, KeyCode::BTN_THUMBR),
];

struct Pad {
    dev: VirtualDevice,
    state: GamepadState,
    /// Uploaded rumble effects: id -> (strong, weak, length ms).
    effects: HashMap<i16, (u16, u16, u16)>,
    /// FF_GAIN set by the game, 0..=0xffff.
    gain: u32,
}

#[derive(Default)]
pub struct UinputGamepads {
    pads: [Option<Pad>; MAX_GAMEPADS as usize],
}

fn io(e: std::io::Error) -> ControllerError {
    if e.kind() == std::io::ErrorKind::PermissionDenied {
        ControllerError::Unavailable(
            "no permission to open /dev/uinput (see the README, Linux and Raspberry Pi)".into(),
        )
    } else {
        ControllerError::Backend(e.to_string())
    }
}

impl UinputGamepads {
    /// Checks that /dev/uinput exists and is writable without creating a device.
    pub fn probe() -> Result<()> {
        std::fs::OpenOptions::new().write(true).open("/dev/uinput").map(|_| ()).map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => {
                ControllerError::Unavailable("/dev/uinput does not exist (load the uinput kernel module)".into())
            }
            _ => io(e),
        })
    }

    fn create(index: u8) -> Result<VirtualDevice> {
        let mut keys = AttributeSet::<KeyCode>::new();
        for (_, code) in BUTTON_CODES {
            keys.insert(code);
        }
        let stick = AbsInfo::new(0, i16::MIN as i32, i16::MAX as i32, 16, 128, 0);
        let trigger = AbsInfo::new(0, 0, u16::MAX as i32, 0, 0, 0);
        let hat = AbsInfo::new(0, -1, 1, 0, 0, 0);
        let name = format!("Mist Virtual Gamepad {}", index + 1);
        let mut b = VirtualDevice::builder()
            .map_err(io)?
            .name(&name)
            .input_id(InputId::new(BusType::BUS_USB, VENDOR, PRODUCT, 0x0114))
            .with_keys(&keys)
            .map_err(io)?;
        for (axis, info) in [
            (AbsoluteAxisCode::ABS_X, stick),
            (AbsoluteAxisCode::ABS_Y, stick),
            (AbsoluteAxisCode::ABS_RX, stick),
            (AbsoluteAxisCode::ABS_RY, stick),
            (AbsoluteAxisCode::ABS_Z, trigger),
            (AbsoluteAxisCode::ABS_RZ, trigger),
            (AbsoluteAxisCode::ABS_HAT0X, hat),
            (AbsoluteAxisCode::ABS_HAT0Y, hat),
        ] {
            b = b.with_absolute_axis(&UinputAbsSetup::new(axis, info)).map_err(io)?;
        }
        // Rumble: games upload FF_RUMBLE effects and play/stop them; the
        // requests are answered in `poll_feedback`.
        b = b
            .with_ff(&AttributeSet::from_iter([FFEffectCode::FF_RUMBLE, FFEffectCode::FF_GAIN]))
            .map_err(io)?
            .with_ff_effects_max(MAX_FF_EFFECTS as u32);
        let dev = b.build().map_err(io)?;
        // Never block the input thread waiting for force-feedback requests.
        let fd = dev.as_raw_fd();
        // SAFETY: fcntl on a file descriptor we own.
        let ok = unsafe {
            let flags = libc::fcntl(fd, libc::F_GETFL);
            flags >= 0 && libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) >= 0
        };
        if !ok {
            return Err(io(std::io::Error::last_os_error()));
        }
        Ok(dev)
    }
}

impl Pad {
    /// Answer pending uploads/erases (games block until we do) and turn
    /// play/stop events into rumble commands.
    fn poll(&mut self, index: u8, out: &mut Vec<Rumble>) {
        loop {
            let events: Vec<InputEvent> = match self.dev.fetch_events() {
                Ok(ev) => ev.collect(),
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => return,
                Err(e) => {
                    tracing::debug!(index, "force feedback read failed: {e}");
                    return;
                }
            };
            if events.is_empty() {
                return;
            }
            for event in events {
                match event.destructure() {
                    EventSummary::UInput(ev, UInputCode::UI_FF_UPLOAD, ..) => {
                        let Ok(mut up) = self.dev.process_ff_upload(ev) else { continue };
                        let mut id = up.effect_id();
                        if id < 0 {
                            id = (0..MAX_FF_EFFECTS as i16).find(|i| !self.effects.contains_key(i)).unwrap_or(-1);
                        }
                        let effect = up.effect();
                        match (id, effect.kind) {
                            (0.., FFEffectKind::Rumble { strong_magnitude, weak_magnitude }) => {
                                self.effects.insert(id, (strong_magnitude, weak_magnitude, effect.replay.length));
                                up.set_effect_id(id);
                                up.set_retval(0);
                            }
                            // Out of slots, or an effect type we do not offer.
                            _ => up.set_retval(-libc::EINVAL),
                        }
                        // Dropping `up` completes the upload.
                    }
                    EventSummary::UInput(ev, UInputCode::UI_FF_ERASE, ..) => {
                        if let Ok(erase) = self.dev.process_ff_erase(ev) {
                            self.effects.remove(&(erase.effect_id() as i16));
                        }
                    }
                    EventSummary::ForceFeedback(_, code, value) if code == FFEffectCode::FF_GAIN => {
                        self.gain = value.clamp(0, 0xffff) as u32;
                    }
                    EventSummary::ForceFeedback(_, code, value) => {
                        let Some(&(strong, weak, length)) = self.effects.get(&(code.0 as i16)) else { continue };
                        let scale = |m: u16| (m as u32 * self.gain / 0xffff) as u16;
                        out.push(if value > 0 {
                            Rumble { index, strong: scale(strong), weak: scale(weak), duration_ms: length }
                        } else {
                            Rumble { index, strong: 0, weak: 0, duration_ms: 0 }
                        });
                    }
                    _ => {}
                }
            }
        }
    }
}

/// Events needed to move a pad from `prev` to `next` (changed values only).
fn diff_events(prev: &GamepadState, next: &GamepadState) -> Vec<InputEvent> {
    let mut ev = Vec::new();
    for (bit, code) in BUTTON_CODES {
        if (prev.buttons ^ next.buttons) & bit != 0 {
            ev.push(InputEvent::new(EventType::KEY.0, code.0, (next.buttons & bit != 0) as i32));
        }
    }
    let hat =
        |s: &GamepadState, neg: u32, pos: u32| -> i32 { (s.buttons & pos != 0) as i32 - (s.buttons & neg != 0) as i32 };
    // Linux convention: Y axes grow downward; the wire format is up-positive.
    let inv = |v: i16| -> i32 { -(v as i32).clamp(-(i16::MAX as i32), i16::MAX as i32) };
    let abs = [
        (AbsoluteAxisCode::ABS_X, prev.left_x as i32, next.left_x as i32),
        (AbsoluteAxisCode::ABS_Y, inv(prev.left_y), inv(next.left_y)),
        (AbsoluteAxisCode::ABS_RX, prev.right_x as i32, next.right_x as i32),
        (AbsoluteAxisCode::ABS_RY, inv(prev.right_y), inv(next.right_y)),
        (AbsoluteAxisCode::ABS_Z, prev.left_trigger as i32, next.left_trigger as i32),
        (AbsoluteAxisCode::ABS_RZ, prev.right_trigger as i32, next.right_trigger as i32),
        (
            AbsoluteAxisCode::ABS_HAT0X,
            hat(prev, buttons::DPAD_LEFT, buttons::DPAD_RIGHT),
            hat(next, buttons::DPAD_LEFT, buttons::DPAD_RIGHT),
        ),
        (
            AbsoluteAxisCode::ABS_HAT0Y,
            hat(prev, buttons::DPAD_UP, buttons::DPAD_DOWN),
            hat(next, buttons::DPAD_UP, buttons::DPAD_DOWN),
        ),
    ];
    for (axis, a, b) in abs {
        if a != b {
            ev.push(InputEvent::new(EventType::ABSOLUTE.0, axis.0, b));
        }
    }
    ev
}

impl ControllerBackend for UinputGamepads {
    fn name(&self) -> &'static str {
        "uinput"
    }

    fn plug(&mut self, index: u8, _kind: GamepadKind) -> Result<()> {
        let slot = self.pads.get_mut(index as usize).ok_or(ControllerError::NotConnected(index))?;
        if slot.is_none() {
            *slot = Some(Pad {
                dev: Self::create(index)?,
                state: GamepadState::default(),
                effects: HashMap::new(),
                gain: 0xffff,
            });
            tracing::info!(index, "virtual gamepad created (uinput)");
        }
        Ok(())
    }

    fn update(&mut self, index: u8, state: &GamepadState) -> Result<()> {
        let pad =
            self.pads.get_mut(index as usize).and_then(|s| s.as_mut()).ok_or(ControllerError::NotConnected(index))?;
        let events = diff_events(&pad.state, state);
        if !events.is_empty() {
            pad.dev.emit(&events).map_err(io)?;
        }
        pad.state = *state;
        Ok(())
    }

    fn unplug(&mut self, index: u8) -> Result<()> {
        if let Some(slot) = self.pads.get_mut(index as usize) {
            if let Some(mut pad) = slot.take() {
                // Release everything before the device disappears.
                let events = diff_events(&pad.state, &GamepadState::default());
                if !events.is_empty() {
                    let _ = pad.dev.emit(&events);
                }
                tracing::info!(index, "virtual gamepad removed");
            }
        }
        Ok(())
    }

    fn poll_feedback(&mut self) -> Vec<Rumble> {
        let mut out = Vec::new();
        for (index, pad) in self.pads.iter_mut().enumerate() {
            if let Some(pad) = pad {
                pad.poll(index as u8, &mut out);
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diff_only_changed_and_inverts_y() {
        let a = GamepadState::default();
        let b = GamepadState { buttons: buttons::A | buttons::DPAD_UP, left_y: i16::MAX, ..Default::default() };
        let ev = diff_events(&a, &b);
        let find = |t: EventType, c: u16| ev.iter().find(|e| e.event_type() == t && e.code() == c).map(|e| e.value());
        assert_eq!(find(EventType::KEY, KeyCode::BTN_SOUTH.0), Some(1));
        assert_eq!(find(EventType::ABSOLUTE, AbsoluteAxisCode::ABS_Y.0), Some(-(i16::MAX as i32)));
        assert_eq!(find(EventType::ABSOLUTE, AbsoluteAxisCode::ABS_HAT0Y.0), Some(-1));
        assert_eq!(find(EventType::ABSOLUTE, AbsoluteAxisCode::ABS_X.0), None);
        assert!(diff_events(&b, &b).is_empty());
        // Back to neutral releases everything that was set.
        let back = diff_events(&b, &GamepadState::default());
        assert_eq!(back.len(), 3);
    }
}
