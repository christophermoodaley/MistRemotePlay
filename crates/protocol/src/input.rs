//! Client -> host input messages carried on the WebRTC data channels.
//!
//! Keyboard, mouse and gamepad are separate message kinds. Gamepads use a
//! standardized Xbox-style layout so the wire format is independent of the
//! physical controller model.

use serde::{Deserialize, Serialize};

use crate::{invalid, keys, Result, Validate};

pub const MAX_GAMEPADS: u8 = 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MouseButton {
    Left,
    Right,
    Middle,
    Back,
    Forward,
}

impl MouseButton {
    pub const ALL: [MouseButton; 5] =
        [MouseButton::Left, MouseButton::Right, MouseButton::Middle, MouseButton::Back, MouseButton::Forward];
}

/// Standard gamepad buttons as bit flags in [`GamepadState::buttons`].
pub mod buttons {
    pub const A: u32 = 1 << 0;
    pub const B: u32 = 1 << 1;
    pub const X: u32 = 1 << 2;
    pub const Y: u32 = 1 << 3;
    pub const LEFT_SHOULDER: u32 = 1 << 4;
    pub const RIGHT_SHOULDER: u32 = 1 << 5;
    pub const BACK: u32 = 1 << 6;
    pub const START: u32 = 1 << 7;
    pub const GUIDE: u32 = 1 << 8;
    pub const LEFT_STICK: u32 = 1 << 9;
    pub const RIGHT_STICK: u32 = 1 << 10;
    pub const DPAD_UP: u32 = 1 << 11;
    pub const DPAD_DOWN: u32 = 1 << 12;
    pub const DPAD_LEFT: u32 = 1 << 13;
    pub const DPAD_RIGHT: u32 = 1 << 14;
    pub const ALL: u32 = (1 << 15) - 1;

    pub const NAMES: [(u32, &str); 15] = [
        (A, "a"),
        (B, "b"),
        (X, "x"),
        (Y, "y"),
        (LEFT_SHOULDER, "left_shoulder"),
        (RIGHT_SHOULDER, "right_shoulder"),
        (BACK, "back"),
        (START, "start"),
        (GUIDE, "guide"),
        (LEFT_STICK, "left_stick"),
        (RIGHT_STICK, "right_stick"),
        (DPAD_UP, "dpad_up"),
        (DPAD_DOWN, "dpad_down"),
        (DPAD_LEFT, "dpad_left"),
        (DPAD_RIGHT, "dpad_right"),
    ];
}

/// Full controller state. Sticks are signed 16-bit (up/right positive),
/// triggers unsigned 16-bit, preserving analog precision.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GamepadState {
    pub buttons: u32,
    pub left_x: i16,
    pub left_y: i16,
    pub right_x: i16,
    pub right_y: i16,
    pub left_trigger: u16,
    pub right_trigger: u16,
}

impl GamepadState {
    pub fn is_neutral(&self) -> bool {
        *self == GamepadState::default()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GamepadKind {
    Xbox,
    PlayStation,
    Nintendo,
    Generic,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum InputMessage {
    /// `usage` is a USB HID keyboard usage, see [`crate::keys`].
    Key { usage: u16, down: bool },
    /// Relative motion in host pixels (pointer-locked / game mode).
    MouseMove { dx: i32, dy: i32 },
    /// Absolute position normalized to 0..=65535 over the streamed area.
    MouseMoveAbs { x: u16, y: u16 },
    MouseButton { button: MouseButton, down: bool },
    /// Scroll in 1/120ths of a notch (Windows WHEEL_DELTA convention).
    Scroll { dx: i32, dy: i32 },
    GamepadConnected { index: u8, kind: GamepadKind },
    GamepadDisconnected { index: u8 },
    /// Full state; `seq` lets the host discard reordered packets on the
    /// unreliable gamepad channel.
    Gamepad { index: u8, seq: u32, state: GamepadState },
    /// Release every held key/button (e.g. client window lost focus).
    ReleaseAll {},
}

impl InputMessage {
    pub fn class(&self) -> InputClass {
        match self {
            InputMessage::Key { .. } | InputMessage::ReleaseAll {} => InputClass::Keyboard,
            InputMessage::MouseMove { .. }
            | InputMessage::MouseMoveAbs { .. }
            | InputMessage::MouseButton { .. }
            | InputMessage::Scroll { .. } => InputClass::Mouse,
            InputMessage::GamepadConnected { .. }
            | InputMessage::GamepadDisconnected { .. }
            | InputMessage::Gamepad { .. } => InputClass::Gamepad,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum InputClass {
    Keyboard,
    Mouse,
    Gamepad,
}

impl Validate for InputMessage {
    fn validate(&self) -> Result<()> {
        match self {
            InputMessage::Key { usage, .. } => {
                if keys::by_hid(*usage).is_none() {
                    return Err(invalid("usage", format!("unknown HID usage {usage:#x}")));
                }
            }
            InputMessage::MouseMove { dx, dy } => {
                if dx.unsigned_abs() > 8192 || dy.unsigned_abs() > 8192 {
                    return Err(invalid("mouse_move", "delta out of range"));
                }
            }
            InputMessage::Scroll { dx, dy } => {
                if dx.unsigned_abs() > 120 * 50 || dy.unsigned_abs() > 120 * 50 {
                    return Err(invalid("scroll", "delta out of range"));
                }
            }
            InputMessage::GamepadConnected { index, .. }
            | InputMessage::GamepadDisconnected { index }
            | InputMessage::Gamepad { index, .. } => {
                if *index >= MAX_GAMEPADS {
                    return Err(invalid("index", "gamepad index out of range"));
                }
                if let InputMessage::Gamepad { state, .. } = self {
                    if state.buttons & !buttons::ALL != 0 {
                        return Err(invalid("buttons", "unknown button bits"));
                    }
                }
            }
            InputMessage::MouseMoveAbs { .. } | InputMessage::MouseButton { .. } | InputMessage::ReleaseAll {} => {}
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{decode, encode, MAX_DATA_CHANNEL_MESSAGE_BYTES};

    fn rt(m: InputMessage) -> crate::Result<InputMessage> {
        decode(encode(&m).as_bytes(), MAX_DATA_CHANNEL_MESSAGE_BYTES)
    }

    #[test]
    fn roundtrip_valid_messages() {
        let msgs = [
            InputMessage::Key { usage: 0x04, down: true },
            InputMessage::MouseMove { dx: -5, dy: 7 },
            InputMessage::MouseMoveAbs { x: 65535, y: 0 },
            InputMessage::MouseButton { button: MouseButton::Left, down: true },
            InputMessage::Scroll { dx: 0, dy: -120 },
            InputMessage::Gamepad {
                index: 1,
                seq: 9,
                state: GamepadState { buttons: buttons::A | buttons::DPAD_UP, left_x: i16::MIN, right_trigger: u16::MAX, ..Default::default() },
            },
            InputMessage::ReleaseAll {},
        ];
        for m in msgs {
            assert_eq!(rt(m.clone()).unwrap(), m);
        }
    }

    #[test]
    fn malformed_input_rejected() {
        assert!(rt(InputMessage::Key { usage: 0x01, down: true }).is_err());
        assert!(rt(InputMessage::MouseMove { dx: 100_000, dy: 0 }).is_err());
        assert!(rt(InputMessage::GamepadDisconnected { index: 4 }).is_err());
        let bad = InputMessage::Gamepad { index: 0, seq: 0, state: GamepadState { buttons: 1 << 20, ..Default::default() } };
        assert!(rt(bad).is_err());
        assert!(decode::<InputMessage>(br#"{"type":"key","usage":4,"down":true,"cmd":"x"}"#, 2048).is_err());
        assert!(decode::<InputMessage>(br#"{"type":"mouse_move","dx":1e9,"dy":0}"#, 2048).is_err());
        assert!(decode::<InputMessage>(br#"{"type":"shell","cmd":"x"}"#, 2048).is_err());
    }
}
