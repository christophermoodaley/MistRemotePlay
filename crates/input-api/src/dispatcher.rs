//! Network -> OS input path with validation, permission checks, rate
//! limits, held-state tracking and guaranteed release.

use std::collections::HashSet;

use mist_controller_api::ControllerBackend;
use mist_protocol::input::{GamepadKind, GamepadState, InputClass, InputMessage, MouseButton, MAX_GAMEPADS};
use mist_protocol::keys;
use mist_protocol::ratelimit::TokenBucket;
use mist_protocol::{decode, MAX_DATA_CHANNEL_MESSAGE_BYTES};
use serde::Serialize;

use crate::InputBackend;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DispatchOutcome {
    Applied,
    /// Valid but intentionally not applied (e.g. stale gamepad packet,
    /// client autorepeat on a host that repeats itself).
    Ignored,
    Denied,
    RateLimited,
    Invalid,
    BackendError,
}

#[derive(Debug, Default, Clone, Serialize)]
pub struct InputStats {
    pub applied: u64,
    pub denied: u64,
    pub rate_limited: u64,
    pub invalid: u64,
    pub backend_errors: u64,
    /// Rumble commands forwarded from games to the client.
    pub rumble: u64,
}

#[derive(Debug, Default, Clone, Copy)]
struct PadSlot {
    connected: bool,
    last_seq: Option<u32>,
    state: GamepadState,
}

pub struct InputDispatcher {
    kbm: Box<dyn InputBackend>,
    pads: Box<dyn ControllerBackend>,
    allowed: bool,
    held_keys: HashSet<u16>,
    held_buttons: HashSet<MouseButton>,
    slots: [PadSlot; MAX_GAMEPADS as usize],
    kb_limit: TokenBucket,
    mouse_limit: TokenBucket,
    pad_limit: TokenBucket,
    stats: InputStats,
    pad_error_reported: bool,
}

/// `a` is newer than `b` in wrapping sequence space.
fn seq_newer(a: u32, b: u32) -> bool {
    a != b && a.wrapping_sub(b) < u32::MAX / 2
}

impl InputDispatcher {
    pub fn new(kbm: Box<dyn InputBackend>, pads: Box<dyn ControllerBackend>) -> Self {
        InputDispatcher {
            kbm,
            pads,
            allowed: false,
            held_keys: HashSet::new(),
            held_buttons: HashSet::new(),
            slots: Default::default(),
            // Humans type < 20 keys/s; mice report up to 1000 Hz; four pads
            // at 250 Hz plus periodic resends.
            kb_limit: TokenBucket::new(60.0, 120.0),
            mouse_limit: TokenBucket::new(2000.0, 2000.0),
            pad_limit: TokenBucket::new(1200.0, 1200.0),
            stats: InputStats::default(),
            pad_error_reported: false,
        }
    }

    pub fn backend_name(&self) -> &'static str {
        self.kbm.name()
    }

    pub fn controller_backend_name(&self) -> &'static str {
        self.pads.name()
    }

    pub fn stats(&self) -> InputStats {
        self.stats.clone()
    }

    pub fn allowed(&self) -> bool {
        self.allowed
    }

    /// Grant or revoke input. Revoking releases everything immediately.
    pub fn set_allowed(&mut self, allowed: bool) {
        if !allowed {
            self.release_all();
        }
        self.allowed = allowed;
    }

    /// Decode and apply a raw data-channel message.
    pub fn handle_text(&mut self, text: &str) -> DispatchOutcome {
        match decode::<InputMessage>(text.as_bytes(), MAX_DATA_CHANNEL_MESSAGE_BYTES) {
            Ok(m) => self.handle(m),
            Err(e) => {
                self.stats.invalid += 1;
                tracing::debug!("rejected input message: {e}");
                DispatchOutcome::Invalid
            }
        }
    }

    pub fn handle(&mut self, msg: InputMessage) -> DispatchOutcome {
        if let Err(e) = mist_protocol::Validate::validate(&msg) {
            self.stats.invalid += 1;
            tracing::debug!("rejected input message: {e}");
            return DispatchOutcome::Invalid;
        }
        // Releases are always honoured, even without permission.
        if let InputMessage::ReleaseAll {} = msg {
            self.release_all();
            return DispatchOutcome::Applied;
        }
        if !self.allowed {
            self.stats.denied += 1;
            return DispatchOutcome::Denied;
        }
        let bucket = match msg.class() {
            InputClass::Keyboard => &mut self.kb_limit,
            InputClass::Mouse => &mut self.mouse_limit,
            InputClass::Gamepad => &mut self.pad_limit,
        };
        if !bucket.try_take() {
            self.stats.rate_limited += 1;
            return DispatchOutcome::RateLimited;
        }
        let result = match msg {
            InputMessage::Key { usage, down, repeat } => {
                if repeat && !self.kbm.wants_client_repeat() {
                    return DispatchOutcome::Ignored;
                }
                let info = keys::by_hid(usage).expect("validated");
                let r = self.kbm.key(info, down);
                if down {
                    self.held_keys.insert(usage);
                } else {
                    self.held_keys.remove(&usage);
                }
                r.map_err(|e| e.to_string())
            }
            InputMessage::MouseMove { dx, dy } => self.kbm.mouse_move_relative(dx, dy).map_err(|e| e.to_string()),
            InputMessage::MouseMoveAbs { x, y } => self.kbm.mouse_move_absolute(x, y).map_err(|e| e.to_string()),
            InputMessage::MouseButton { button, down } => {
                if down {
                    self.held_buttons.insert(button);
                } else {
                    self.held_buttons.remove(&button);
                }
                self.kbm.mouse_button(button, down).map_err(|e| e.to_string())
            }
            InputMessage::Scroll { dx, dy } => self.kbm.scroll(dx, dy).map_err(|e| e.to_string()),
            InputMessage::GamepadConnected { index, kind } => self.plug(index, kind),
            InputMessage::GamepadDisconnected { index } => self.unplug(index),
            InputMessage::Gamepad { index, seq, state } => {
                let slot = self.slots[index as usize];
                if let Some(last) = slot.last_seq {
                    if !seq_newer(seq, last) {
                        return DispatchOutcome::Ignored;
                    }
                }
                // State can overtake the (reliable) connect message.
                if !slot.connected {
                    if let Err(e) = self.plug(index, GamepadKind::Xbox) {
                        return self.pad_error(e);
                    }
                }
                let s = &mut self.slots[index as usize];
                s.last_seq = Some(seq);
                s.state = state;
                self.pads.update(index, &state).map_err(|e| e.to_string())
            }
            InputMessage::ReleaseAll {} => unreachable!("handled above"),
        };
        match result {
            Ok(()) => {
                self.stats.applied += 1;
                DispatchOutcome::Applied
            }
            Err(e) => self.pad_error(e),
        }
    }

    fn pad_error(&mut self, e: String) -> DispatchOutcome {
        self.stats.backend_errors += 1;
        if !self.pad_error_reported {
            tracing::warn!("input backend error: {e}");
            self.pad_error_reported = true;
        }
        DispatchOutcome::BackendError
    }

    fn plug(&mut self, index: u8, kind: GamepadKind) -> Result<(), String> {
        let slot = &mut self.slots[index as usize];
        if slot.connected {
            return Ok(());
        }
        self.pads.plug(index, kind).map_err(|e| e.to_string())?;
        *slot = PadSlot { connected: true, last_seq: None, state: GamepadState::default() };
        Ok(())
    }

    fn unplug(&mut self, index: u8) -> Result<(), String> {
        let slot = &mut self.slots[index as usize];
        if !slot.connected {
            return Ok(());
        }
        let _ = self.pads.update(index, &GamepadState::default());
        *slot = PadSlot::default();
        self.pads.unplug(index).map_err(|e| e.to_string())
    }

    /// Release every held key, mouse button and gamepad input. Called on
    /// client request, focus loss, permission revocation and disconnect.
    pub fn release_all(&mut self) {
        let keys: Vec<u16> = self.held_keys.drain().collect();
        for usage in keys {
            if let Some(info) = keys::by_hid(usage) {
                if let Err(e) = self.kbm.key(info, false) {
                    tracing::warn!("releasing key {usage:#x}: {e}");
                }
            }
        }
        let buttons: Vec<MouseButton> = self.held_buttons.drain().collect();
        for b in buttons {
            if let Err(e) = self.kbm.mouse_button(b, false) {
                tracing::warn!("releasing mouse button {b:?}: {e}");
            }
        }
        for index in 0..MAX_GAMEPADS {
            let slot = &mut self.slots[index as usize];
            if slot.connected && !slot.state.is_neutral() {
                slot.state = GamepadState::default();
                if let Err(e) = self.pads.update(index, &GamepadState::default()) {
                    tracing::warn!("neutralizing gamepad {index}: {e}");
                }
            }
        }
    }

    /// Rumble requests from games for connected pads, to forward to the
    /// client. Must be called regularly: on Linux, games block in their
    /// force-feedback upload until the backend answers.
    pub fn poll_feedback(&mut self) -> Vec<mist_controller_api::Rumble> {
        let slots = self.slots;
        let out: Vec<_> = self
            .pads
            .poll_feedback()
            .into_iter()
            .filter(|r| slots.get(r.index as usize).map(|s| s.connected).unwrap_or(false))
            .collect();
        self.stats.rumble += out.len() as u64;
        out
    }

    /// Session ended: release everything and remove virtual controllers.
    pub fn disconnect(&mut self) {
        self.release_all();
        for index in 0..MAX_GAMEPADS {
            let _ = self.unplug(index);
        }
        self.allowed = false;
    }

    pub fn held_count(&self) -> usize {
        self.held_keys.len() + self.held_buttons.len()
    }
}

impl Drop for InputDispatcher {
    fn drop(&mut self) {
        self.disconnect();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{MockInput, MockInputEvent};
    use mist_controller_api::{MockController, MockPadEvent};
    use mist_protocol::input::buttons;

    #[test]
    fn rumble_from_connected_pads_is_forwarded() {
        let (mut d, _, _) = setup();
        d.handle(InputMessage::GamepadConnected { index: 1, kind: GamepadKind::Xbox });
        let state = GamepadState { left_trigger: 1000, right_trigger: 2000, ..Default::default() };
        d.handle(InputMessage::Gamepad { index: 1, seq: 1, state });
        let expected = mist_controller_api::Rumble { index: 1, strong: 1000, weak: 2000, duration_ms: 0 };
        assert_eq!(d.poll_feedback(), vec![expected]);
        assert!(d.poll_feedback().is_empty(), "feedback is drained");
        assert_eq!(d.stats().rumble, 1);
        // Feedback for a pad that is gone is not forwarded.
        d.handle(InputMessage::GamepadDisconnected { index: 1 });
        assert!(d.poll_feedback().is_empty());
    }

    fn setup() -> (InputDispatcher, MockInput, MockController) {
        let kbm = MockInput::default();
        let pads = MockController::default();
        let mut d = InputDispatcher::new(Box::new(kbm.clone()), Box::new(pads.clone()));
        d.set_allowed(true);
        (d, kbm, pads)
    }

    #[test]
    fn denied_without_permission() {
        let (mut d, kbm, _) = setup();
        d.set_allowed(false);
        assert_eq!(d.handle(InputMessage::Key { usage: 4, down: true, repeat: false }), DispatchOutcome::Denied);
        assert!(kbm.log.snapshot().is_empty());
    }

    #[test]
    fn revoking_permission_releases_held_input() {
        let (mut d, kbm, pads) = setup();
        d.handle(InputMessage::Key { usage: 0xE1, down: true, repeat: false });
        d.handle(InputMessage::Key { usage: 0x1A, down: true, repeat: false });
        d.handle(InputMessage::MouseButton { button: MouseButton::Left, down: true });
        let held = GamepadState { buttons: buttons::A, right_trigger: 9000, ..Default::default() };
        d.handle(InputMessage::Gamepad { index: 0, seq: 1, state: held });
        kbm.log.clear();
        pads.log.clear();
        d.set_allowed(false);
        let log = kbm.log.snapshot();
        assert!(log.contains(&MockInputEvent::Key(0xE1, false)));
        assert!(log.contains(&MockInputEvent::Key(0x1A, false)));
        assert!(log.contains(&MockInputEvent::Button(MouseButton::Left, false)));
        assert_eq!(pads.log.snapshot(), vec![MockPadEvent::Update(0, GamepadState::default())]);
        assert_eq!(d.held_count(), 0);
    }

    #[test]
    fn disconnect_releases_and_unplugs() {
        let (mut d, kbm, pads) = setup();
        d.handle(InputMessage::GamepadConnected { index: 1, kind: GamepadKind::PlayStation });
        d.handle(InputMessage::Key { usage: 0x2C, down: true, repeat: false });
        kbm.log.clear();
        d.disconnect();
        assert_eq!(kbm.log.snapshot(), vec![MockInputEvent::Key(0x2C, false)]);
        assert!(pads.log.snapshot().contains(&MockPadEvent::Unplug(1)));
        assert!(!d.allowed());
        assert_eq!(d.handle(InputMessage::Key { usage: 4, down: true, repeat: false }), DispatchOutcome::Denied);
    }

    #[test]
    fn malformed_and_unknown_messages_rejected() {
        let (mut d, kbm, _) = setup();
        assert_eq!(d.handle_text("{\"type\":\"key\",\"usage\":1,\"down\":true}"), DispatchOutcome::Invalid);
        assert_eq!(d.handle_text("{\"type\":\"run\",\"cmd\":\"calc.exe\"}"), DispatchOutcome::Invalid);
        assert_eq!(d.handle_text("not json"), DispatchOutcome::Invalid);
        assert_eq!(d.handle_text(&"x".repeat(5000)), DispatchOutcome::Invalid);
        assert!(kbm.log.snapshot().is_empty());
        assert_eq!(d.stats().invalid, 4);
    }

    #[test]
    fn keyboard_flood_is_rate_limited() {
        let (mut d, _, _) = setup();
        let mut limited = 0;
        for i in 0..500 {
            if d.handle(InputMessage::Key { usage: 4, down: i % 2 == 0, repeat: false }) == DispatchOutcome::RateLimited
            {
                limited += 1;
            }
        }
        assert!(limited > 300, "only {limited} limited");
    }

    #[test]
    fn stale_gamepad_packets_ignored_and_autoconnect() {
        let (mut d, _, pads) = setup();
        let s = |b| GamepadState { buttons: b, ..Default::default() };
        assert_eq!(
            d.handle(InputMessage::Gamepad { index: 2, seq: 10, state: s(buttons::A) }),
            DispatchOutcome::Applied
        );
        assert_eq!(
            d.handle(InputMessage::Gamepad { index: 2, seq: 9, state: s(buttons::B) }),
            DispatchOutcome::Ignored
        );
        assert_eq!(
            d.handle(InputMessage::Gamepad { index: 2, seq: 10, state: s(buttons::B) }),
            DispatchOutcome::Ignored
        );
        assert_eq!(
            d.handle(InputMessage::Gamepad { index: 2, seq: 11, state: s(buttons::B) }),
            DispatchOutcome::Applied
        );
        let log = pads.log.snapshot();
        assert_eq!(log[0], MockPadEvent::Plug(2, GamepadKind::Xbox));
        assert_eq!(log.len(), 3);
        // Wraparound.
        assert!(seq_newer(0, u32::MAX));
        assert!(!seq_newer(u32::MAX, 0));
    }

    #[test]
    fn client_autorepeat_ignored_when_host_repeats() {
        let (mut d, kbm, _) = setup();
        assert_eq!(d.handle(InputMessage::Key { usage: 4, down: true, repeat: true }), DispatchOutcome::Ignored);
        assert!(kbm.log.snapshot().is_empty());
    }

    #[test]
    fn release_all_honoured_without_permission() {
        let (mut d, kbm, _) = setup();
        d.handle(InputMessage::Key { usage: 4, down: true, repeat: false });
        d.allowed = false; // simulate: permission flag flipped without release
        kbm.log.clear();
        assert_eq!(d.handle(InputMessage::ReleaseAll {}), DispatchOutcome::Applied);
        assert_eq!(kbm.log.snapshot(), vec![MockInputEvent::Key(4, false)]);
    }
}
