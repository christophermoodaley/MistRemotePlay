//! Client-side physical controller discovery and polling (gilrs).
//!
//! gilrs applies the SDL GameControllerDB mappings, so different physical
//! controllers arrive in one standard layout; we then apply the user's
//! [`ControllerMapping`] and assign stable slots (a reconnecting controller
//! gets its previous slot back when it is still free).

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

use gilrs::{Axis, Button, EventType, GamepadId, Gilrs};
use mist_protocol::input::{buttons, GamepadKind, GamepadState, MAX_GAMEPADS};
use serde::Serialize;

use crate::normalize::{ControllerMapping, RawGamepad};
use crate::{ControllerError, Result};

#[derive(Debug, Clone, PartialEq)]
pub enum ControllerEvent {
    Connected { slot: u8, name: String, kind: GamepadKind },
    State { slot: u8, state: GamepadState },
    Disconnected { slot: u8 },
}

#[derive(Debug, Clone, Serialize)]
pub struct ConnectedController {
    pub slot: u8,
    pub name: String,
    pub kind: GamepadKind,
}

pub fn kind_from_name(name: &str) -> GamepadKind {
    let n = name.to_ascii_lowercase();
    if n.contains("xbox") || n.contains("x-box") || n.contains("xinput") {
        GamepadKind::Xbox
    } else if n.contains("playstation") || n.contains("dualshock") || n.contains("dualsense") || n.contains("ps4") || n.contains("ps5") || n == "wireless controller" {
        GamepadKind::PlayStation
    } else if n.contains("nintendo") || n.contains("pro controller") || n.contains("joy-con") {
        GamepadKind::Nintendo
    } else {
        GamepadKind::Generic
    }
}

/// Slot allocation with reconnect affinity.
#[derive(Debug, Default)]
pub struct SlotTable {
    active: HashMap<usize, u8>,
    remembered: HashMap<[u8; 16], u8>,
}

impl SlotTable {
    pub fn assign(&mut self, key: usize, uuid: [u8; 16]) -> Option<u8> {
        if let Some(s) = self.active.get(&key) {
            return Some(*s);
        }
        let used: Vec<u8> = self.active.values().copied().collect();
        let preferred = self.remembered.get(&uuid).copied().filter(|s| !used.contains(s));
        let slot = preferred.or_else(|| (0..MAX_GAMEPADS).find(|s| !used.contains(s)))?;
        self.active.insert(key, slot);
        self.remembered.insert(uuid, slot);
        Some(slot)
    }

    pub fn release(&mut self, key: usize) -> Option<u8> {
        self.active.remove(&key)
    }

    pub fn get(&self, key: usize) -> Option<u8> {
        self.active.get(&key).copied()
    }
}

fn read_raw(pad: &gilrs::Gamepad<'_>) -> RawGamepad {
    const MAP: [(Button, u32); 15] = [
        (Button::South, buttons::A),
        (Button::East, buttons::B),
        (Button::West, buttons::X),
        (Button::North, buttons::Y),
        (Button::LeftTrigger, buttons::LEFT_SHOULDER),
        (Button::RightTrigger, buttons::RIGHT_SHOULDER),
        (Button::Select, buttons::BACK),
        (Button::Start, buttons::START),
        (Button::Mode, buttons::GUIDE),
        (Button::LeftThumb, buttons::LEFT_STICK),
        (Button::RightThumb, buttons::RIGHT_STICK),
        (Button::DPadUp, buttons::DPAD_UP),
        (Button::DPadDown, buttons::DPAD_DOWN),
        (Button::DPadLeft, buttons::DPAD_LEFT),
        (Button::DPadRight, buttons::DPAD_RIGHT),
    ];
    let mut b = 0;
    for (btn, bit) in MAP {
        if pad.is_pressed(btn) {
            b |= bit;
        }
    }
    let trig = |btn| pad.button_data(btn).map(|d| d.value()).unwrap_or(0.0);
    RawGamepad {
        buttons: b,
        left: (pad.value(Axis::LeftStickX), pad.value(Axis::LeftStickY)),
        right: (pad.value(Axis::RightStickX), pad.value(Axis::RightStickY)),
        left_trigger: trig(Button::LeftTrigger2),
        right_trigger: trig(Button::RightTrigger2),
    }
}

pub struct ControllerSource {
    stop: Arc<AtomicBool>,
    connected: Arc<Mutex<Vec<ConnectedController>>>,
    mapping: Arc<Mutex<ControllerMapping>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

/// State is re-sent at this interval even when unchanged, because the
/// gamepad data channel is unreliable.
pub const RESEND_INTERVAL: Duration = Duration::from_millis(100);

impl ControllerSource {
    pub fn start(mapping: ControllerMapping, tx: mpsc::Sender<ControllerEvent>) -> Result<Self> {
        mapping.validate().map_err(ControllerError::Backend)?;
        let stop = Arc::new(AtomicBool::new(false));
        let connected = Arc::new(Mutex::new(Vec::new()));
        let mapping = Arc::new(Mutex::new(mapping));
        let (ready_tx, ready_rx) = mpsc::channel();
        let (s, c, m) = (stop.clone(), connected.clone(), mapping.clone());
        let thread = std::thread::Builder::new()
            .name("controllers".into())
            .spawn(move || {
                let gilrs = match Gilrs::new() {
                    Ok(g) => {
                        let _ = ready_tx.send(Ok(()));
                        g
                    }
                    Err(e) => {
                        let _ = ready_tx.send(Err(e.to_string()));
                        return;
                    }
                };
                run(gilrs, s, c, m, tx);
            })
            .map_err(|e| ControllerError::Backend(e.to_string()))?;
        ready_rx
            .recv()
            .map_err(|_| ControllerError::Backend("controller thread exited".into()))?
            .map_err(|e| ControllerError::Unavailable(format!("controller input unavailable: {e}")))?;
        Ok(ControllerSource { stop, connected, mapping, thread: Some(thread) })
    }

    pub fn controllers(&self) -> Vec<ConnectedController> {
        self.connected.lock().map(|c| c.clone()).unwrap_or_default()
    }

    pub fn set_mapping(&self, mapping: ControllerMapping) -> Result<()> {
        mapping.validate().map_err(ControllerError::Backend)?;
        if let Ok(mut m) = self.mapping.lock() {
            *m = mapping;
        }
        Ok(())
    }
}

impl Drop for ControllerSource {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn run(
    mut gilrs: Gilrs,
    stop: Arc<AtomicBool>,
    connected: Arc<Mutex<Vec<ConnectedController>>>,
    mapping: Arc<Mutex<ControllerMapping>>,
    tx: mpsc::Sender<ControllerEvent>,
) {
    let mut slots = SlotTable::default();
    let mut last: HashMap<u8, GamepadState> = HashMap::new();
    let mut last_send = Instant::now();

    let publish = |slots: &SlotTable, gilrs: &Gilrs, connected: &Mutex<Vec<ConnectedController>>| {
        let list = gilrs
            .gamepads()
            .filter_map(|(id, pad)| {
                slots.get(usize::from(id)).map(|slot| ConnectedController {
                    slot,
                    name: pad.name().to_string(),
                    kind: kind_from_name(pad.name()),
                })
            })
            .collect();
        if let Ok(mut c) = connected.lock() {
            *c = list;
        }
    };

    let connect = |id: GamepadId, gilrs: &Gilrs, slots: &mut SlotTable| -> Option<u8> {
        let pad = gilrs.gamepad(id);
        let slot = slots.assign(usize::from(id), pad.uuid())?;
        let name = pad.name().to_string();
        tracing::info!(slot, %name, "controller connected");
        let _ = tx.send(ControllerEvent::Connected { slot, name: name.clone(), kind: kind_from_name(&name) });
        Some(slot)
    };

    // Controllers already present at start.
    let initial: Vec<GamepadId> = gilrs.gamepads().map(|(id, _)| id).collect();
    for id in initial {
        connect(id, &gilrs, &mut slots);
    }
    publish(&slots, &gilrs, &connected);

    while !stop.load(Ordering::SeqCst) {
        let mut dirty = Vec::new();
        if let Some(ev) = gilrs.next_event_blocking(Some(Duration::from_millis(4))) {
            match ev.event {
                EventType::Connected => {
                    if connect(ev.id, &gilrs, &mut slots).is_none() {
                        tracing::warn!("more than {MAX_GAMEPADS} controllers; ignoring the new one");
                    }
                    publish(&slots, &gilrs, &connected);
                }
                EventType::Disconnected => {
                    if let Some(slot) = slots.release(usize::from(ev.id)) {
                        tracing::info!(slot, "controller disconnected");
                        last.remove(&slot);
                        let _ = tx.send(ControllerEvent::Disconnected { slot });
                    }
                    publish(&slots, &gilrs, &connected);
                }
                _ => dirty.push(ev.id),
            }
            // Drain anything else queued so state is read once per batch.
            while let Some(ev) = gilrs.next_event() {
                if matches!(ev.event, EventType::Connected | EventType::Disconnected) {
                    // Handle on the next loop iteration via a re-scan.
                    continue;
                }
                dirty.push(ev.id);
            }
        }
        let map = mapping.lock().map(|m| m.clone()).unwrap_or_default();
        let resend = last_send.elapsed() >= RESEND_INTERVAL;
        let ids: Vec<GamepadId> = gilrs.gamepads().map(|(id, _)| id).collect();
        for id in ids {
            let Some(slot) = slots.get(usize::from(id)) else { continue };
            if !resend && !dirty.contains(&id) {
                continue;
            }
            let state = map.apply(&read_raw(&gilrs.gamepad(id)));
            if resend || last.get(&slot) != Some(&state) {
                last.insert(slot, state);
                let _ = tx.send(ControllerEvent::State { slot, state });
            }
        }
        if resend {
            last_send = Instant::now();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slots_fill_lowest_and_remember_on_reconnect() {
        let mut t = SlotTable::default();
        let (u1, u2, u3) = ([1; 16], [2; 16], [3; 16]);
        assert_eq!(t.assign(10, u1), Some(0));
        assert_eq!(t.assign(11, u2), Some(1));
        assert_eq!(t.release(10), Some(0));
        // A different controller takes the free slot 0...
        assert_eq!(t.assign(12, u3), Some(0));
        // ...so the reconnecting first controller gets the next free one.
        assert_eq!(t.assign(13, u1), Some(2));
        t.release(13);
        assert_eq!(t.assign(14, u1), Some(2));
        assert_eq!(t.assign(15, [4; 16]), Some(3));
        assert_eq!(t.assign(16, [5; 16]), None);
    }

    #[test]
    fn kinds() {
        assert_eq!(kind_from_name("Xbox Wireless Controller"), GamepadKind::Xbox);
        assert_eq!(kind_from_name("DualSense Wireless Controller"), GamepadKind::PlayStation);
        assert_eq!(kind_from_name("Pro Controller"), GamepadKind::Nintendo);
        assert_eq!(kind_from_name("8BitDo SN30"), GamepadKind::Generic);
    }
}
