//! Controller normalization and user mapping.
//!
//! Physical controllers report axes as floats (-1..1) and triggers 0..1.
//! They are converted to the wire representation (i16 sticks, u16
//! triggers) after a radial deadzone, optional inversion and button remap.

use std::collections::BTreeMap;

use mist_protocol::input::{buttons, GamepadState};
use serde::{Deserialize, Serialize};

/// Raw, unmapped reading in the standard layout.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct RawGamepad {
    pub buttons: u32,
    pub left: (f32, f32),
    pub right: (f32, f32),
    pub left_trigger: f32,
    pub right_trigger: f32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ControllerMapping {
    /// Radial deadzone for each stick, 0.0..0.5.
    pub left_deadzone: f32,
    pub right_deadzone: f32,
    pub trigger_deadzone: f32,
    pub invert_left_y: bool,
    pub invert_right_y: bool,
    /// Source button name -> target button name (see `buttons::NAMES`).
    pub remap: BTreeMap<String, String>,
}

impl Default for ControllerMapping {
    fn default() -> Self {
        ControllerMapping {
            left_deadzone: 0.08,
            right_deadzone: 0.08,
            trigger_deadzone: 0.02,
            invert_left_y: false,
            invert_right_y: false,
            remap: BTreeMap::new(),
        }
    }
}

pub fn button_by_name(name: &str) -> Option<u32> {
    buttons::NAMES.iter().find(|(_, n)| *n == name).map(|(b, _)| *b)
}

impl ControllerMapping {
    pub fn validate(&self) -> Result<(), String> {
        for dz in [self.left_deadzone, self.right_deadzone, self.trigger_deadzone] {
            if !(0.0..=0.5).contains(&dz) {
                return Err("deadzones must be between 0.0 and 0.5".into());
            }
        }
        for (from, to) in &self.remap {
            if button_by_name(from).is_none() || button_by_name(to).is_none() {
                return Err(format!("unknown button in remap {from} -> {to}"));
            }
        }
        Ok(())
    }

    fn remap_buttons(&self, pressed: u32) -> u32 {
        if self.remap.is_empty() {
            return pressed;
        }
        let mut out = 0;
        for (bit, name) in buttons::NAMES {
            if pressed & bit == 0 {
                continue;
            }
            let target = self.remap.get(name).and_then(|t| button_by_name(t)).unwrap_or(bit);
            out |= target;
        }
        out
    }

    pub fn apply(&self, raw: &RawGamepad) -> GamepadState {
        let (lx, ly) = radial_deadzone(raw.left, self.left_deadzone);
        let (rx, ry) = radial_deadzone(raw.right, self.right_deadzone);
        GamepadState {
            buttons: self.remap_buttons(raw.buttons) & buttons::ALL,
            left_x: axis_to_i16(lx),
            left_y: axis_to_i16(if self.invert_left_y { -ly } else { ly }),
            right_x: axis_to_i16(rx),
            right_y: axis_to_i16(if self.invert_right_y { -ry } else { ry }),
            left_trigger: trigger_to_u16(raw.left_trigger, self.trigger_deadzone),
            right_trigger: trigger_to_u16(raw.right_trigger, self.trigger_deadzone),
        }
    }
}

/// Scaled radial deadzone: input inside the deadzone maps to 0, the rest is
/// rescaled so full deflection still reaches 1.0 (no lost range).
pub fn radial_deadzone((x, y): (f32, f32), deadzone: f32) -> (f32, f32) {
    let x = if x.is_finite() { x.clamp(-1.0, 1.0) } else { 0.0 };
    let y = if y.is_finite() { y.clamp(-1.0, 1.0) } else { 0.0 };
    let mag = (x * x + y * y).sqrt();
    if mag <= deadzone || mag == 0.0 {
        return (0.0, 0.0);
    }
    let scaled = ((mag - deadzone) / (1.0 - deadzone)).min(1.0);
    (x / mag * scaled, y / mag * scaled)
}

pub fn axis_to_i16(v: f32) -> i16 {
    let v = if v.is_finite() { v.clamp(-1.0, 1.0) } else { 0.0 };
    if v >= 0.0 {
        (v * i16::MAX as f32).round() as i16
    } else {
        (v * -(i16::MIN as f32)).round() as i16
    }
}

pub fn trigger_to_u16(v: f32, deadzone: f32) -> u16 {
    let v = if v.is_finite() { v.clamp(0.0, 1.0) } else { 0.0 };
    if v <= deadzone {
        return 0;
    }
    (((v - deadzone) / (1.0 - deadzone)) * u16::MAX as f32).round() as u16
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn axis_extremes_preserved() {
        assert_eq!(axis_to_i16(1.0), i16::MAX);
        assert_eq!(axis_to_i16(-1.0), i16::MIN);
        assert_eq!(axis_to_i16(0.0), 0);
        assert_eq!(axis_to_i16(f32::NAN), 0);
        assert_eq!(axis_to_i16(7.0), i16::MAX);
        // Precision: small deflections survive (no 8-bit quantization).
        assert_eq!(axis_to_i16(0.001), 33);
    }

    #[test]
    fn deadzone_is_radial_and_rescaled() {
        assert_eq!(radial_deadzone((0.05, 0.05), 0.1), (0.0, 0.0));
        let (x, y) = radial_deadzone((1.0, 0.0), 0.1);
        assert!((x - 1.0).abs() < 1e-6 && y == 0.0);
        let (x, _) = radial_deadzone((0.55, 0.0), 0.1);
        assert!((x - 0.5).abs() < 1e-6);
        // Diagonal keeps direction.
        let (x, y) = radial_deadzone((0.5, 0.5), 0.1);
        assert!((x - y).abs() < 1e-6);
    }

    #[test]
    fn triggers() {
        assert_eq!(trigger_to_u16(0.0, 0.02), 0);
        assert_eq!(trigger_to_u16(0.01, 0.02), 0);
        assert_eq!(trigger_to_u16(1.0, 0.02), u16::MAX);
        assert_eq!(trigger_to_u16(-3.0, 0.0), 0);
    }

    #[test]
    fn remap_and_invert() {
        let mut m = ControllerMapping::default();
        m.remap.insert("a".into(), "b".into());
        m.remap.insert("b".into(), "a".into());
        m.invert_left_y = true;
        m.validate().unwrap();
        let raw = RawGamepad { buttons: buttons::A | buttons::START, left: (0.0, 1.0), ..Default::default() };
        let s = m.apply(&raw);
        assert_eq!(s.buttons, buttons::B | buttons::START);
        assert_eq!(s.left_y, i16::MIN);
        m.remap.insert("a".into(), "jump".into());
        assert!(m.validate().is_err());
    }
}
