//! Network -> OS input path with validation, permission checks, rate
//! limits, held-state tracking and guaranteed release.
//!
//! Several people can be connected at once. Each is a *client* with its
//! own permissions (keyboard/mouse, gamepads), held keys and buttons, and
//! rate-limit budget. Every client numbers its own gamepads from 0; the
//! dispatcher maps them onto the host's [`MAX_GAMEPADS`] virtual controller
//! slots, which all clients share, and routes force feedback back to the
//! owner's own pad number.

use std::collections::{HashMap, HashSet};

use mist_controller_api::{ControllerBackend, Rumble};
use mist_protocol::input::{GamepadKind, GamepadState, InputClass, InputMessage, MouseButton, MAX_GAMEPADS};
use mist_protocol::keys;
use mist_protocol::ratelimit::TokenBucket;
use mist_protocol::{decode, MAX_DATA_CHANNEL_MESSAGE_BYTES};
use serde::Serialize;

use crate::InputBackend;

/// Identifies one connected client (session) to the dispatcher.
pub type ClientId = u64;

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
    /// Every host controller slot is taken by other pads.
    NoFreeController,
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
    /// Gamepads refused because all host controller slots were in use.
    pub pads_refused: u64,
}

impl InputStats {
    fn add(&mut self, o: &InputStats) {
        self.applied += o.applied;
        self.denied += o.denied;
        self.rate_limited += o.rate_limited;
        self.invalid += o.invalid;
        self.backend_errors += o.backend_errors;
        self.rumble += o.rumble;
        self.pads_refused += o.pads_refused;
    }
}

/// What one client may control.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct InputPermissions {
    pub keyboard_mouse: bool,
    pub gamepads: bool,
}

impl InputPermissions {
    pub const ALL: InputPermissions = InputPermissions { keyboard_mouse: true, gamepads: true };
    pub const NONE: InputPermissions = InputPermissions { keyboard_mouse: false, gamepads: false };
}

#[derive(Debug, Default, Clone, Copy)]
struct HostSlot {
    /// (client, the client's own pad index) using this virtual controller.
    owner: Option<(ClientId, u8)>,
    last_seq: Option<u32>,
    state: GamepadState,
}

struct ClientState {
    perms: InputPermissions,
    held_keys: HashSet<u16>,
    held_buttons: HashSet<MouseButton>,
    kb_limit: TokenBucket,
    mouse_limit: TokenBucket,
    pad_limit: TokenBucket,
    stats: InputStats,
    pad_full_reported: bool,
}

impl ClientState {
    fn new(perms: InputPermissions) -> Self {
        ClientState {
            perms,
            held_keys: HashSet::new(),
            held_buttons: HashSet::new(),
            // Humans type < 20 keys/s; mice report up to 1000 Hz; four pads
            // at 250 Hz plus periodic resends.
            kb_limit: TokenBucket::new(60.0, 120.0),
            mouse_limit: TokenBucket::new(2000.0, 2000.0),
            pad_limit: TokenBucket::new(1200.0, 1200.0),
            stats: InputStats::default(),
            pad_full_reported: false,
        }
    }
}

pub struct InputDispatcher {
    kbm: Box<dyn InputBackend>,
    pads: Box<dyn ControllerBackend>,
    /// Host-wide switch; per-client permissions apply on top of it.
    allowed: bool,
    clients: HashMap<ClientId, ClientState>,
    slots: [HostSlot; MAX_GAMEPADS as usize],
    /// Counters of clients that have left (stats are totals since start).
    departed: InputStats,
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
            clients: HashMap::new(),
            slots: Default::default(),
            departed: InputStats::default(),
            pad_error_reported: false,
        }
    }

    pub fn backend_name(&self) -> &'static str {
        self.kbm.name()
    }

    pub fn controller_backend_name(&self) -> &'static str {
        self.pads.name()
    }

    /// Totals over every client, including ones that have left.
    pub fn stats(&self) -> InputStats {
        let mut s = self.departed.clone();
        for c in self.clients.values() {
            s.add(&c.stats);
        }
        s
    }

    pub fn client_stats(&self, client: ClientId) -> Option<InputStats> {
        self.clients.get(&client).map(|c| c.stats.clone())
    }

    /// Host controller slots (0-based) the client's pads currently occupy.
    pub fn client_pads(&self, client: ClientId) -> Vec<u8> {
        (0..MAX_GAMEPADS).filter(|&i| matches!(self.slots[i as usize].owner, Some((c, _)) if c == client)).collect()
    }

    pub fn allowed(&self) -> bool {
        self.allowed
    }

    /// Grant or revoke input for everyone. Revoking releases everything
    /// immediately.
    pub fn set_allowed(&mut self, allowed: bool) {
        if !allowed {
            self.release_all();
        }
        self.allowed = allowed;
    }

    /// Start accepting input from a client (no-op if it is known).
    pub fn add_client(&mut self, client: ClientId, perms: InputPermissions) {
        self.clients.entry(client).or_insert_with(|| ClientState::new(perms));
    }

    /// The client left: release its input and remove its virtual pads.
    pub fn remove_client(&mut self, client: ClientId) {
        self.release_client(client);
        self.unplug_client_pads(client);
        if let Some(c) = self.clients.remove(&client) {
            self.departed.add(&c.stats);
        }
    }

    /// Change what a client may control. Whatever it loses is released at
    /// once; losing gamepads also removes its virtual pads.
    pub fn set_permissions(&mut self, client: ClientId, perms: InputPermissions) {
        let Some(c) = self.clients.get_mut(&client) else { return };
        let old = std::mem::replace(&mut c.perms, perms);
        if old.keyboard_mouse && !perms.keyboard_mouse {
            self.release_keyboard_mouse(client);
        }
        if old.gamepads && !perms.gamepads {
            self.unplug_client_pads(client);
        }
    }

    pub fn permissions(&self, client: ClientId) -> Option<InputPermissions> {
        self.clients.get(&client).map(|c| c.perms)
    }

    /// Decode and apply a raw data-channel message from `client`.
    pub fn handle_text(&mut self, client: ClientId, text: &str) -> DispatchOutcome {
        match decode::<InputMessage>(text.as_bytes(), MAX_DATA_CHANNEL_MESSAGE_BYTES) {
            Ok(m) => self.handle(client, m),
            Err(e) => {
                if let Some(c) = self.clients.get_mut(&client) {
                    c.stats.invalid += 1;
                }
                tracing::debug!("rejected input message: {e}");
                DispatchOutcome::Invalid
            }
        }
    }

    pub fn handle(&mut self, client: ClientId, msg: InputMessage) -> DispatchOutcome {
        let allowed = self.allowed;
        let Some(c) = self.clients.get_mut(&client) else {
            tracing::debug!(client, "input from an unknown client");
            return DispatchOutcome::Denied;
        };
        if let Err(e) = mist_protocol::Validate::validate(&msg) {
            c.stats.invalid += 1;
            tracing::debug!("rejected input message: {e}");
            return DispatchOutcome::Invalid;
        }
        // Releases are always honoured, even without permission.
        if let InputMessage::ReleaseAll {} = msg {
            self.release_client(client);
            return DispatchOutcome::Applied;
        }
        let class = msg.class();
        let permitted = match class {
            InputClass::Keyboard | InputClass::Mouse => c.perms.keyboard_mouse,
            InputClass::Gamepad => c.perms.gamepads,
        };
        if !allowed || !permitted {
            c.stats.denied += 1;
            return DispatchOutcome::Denied;
        }
        let bucket = match class {
            InputClass::Keyboard => &mut c.kb_limit,
            InputClass::Mouse => &mut c.mouse_limit,
            InputClass::Gamepad => &mut c.pad_limit,
        };
        if !bucket.try_take() {
            c.stats.rate_limited += 1;
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
                    c.held_keys.insert(usage);
                } else {
                    c.held_keys.remove(&usage);
                }
                r.map_err(|e| e.to_string())
            }
            InputMessage::MouseMove { dx, dy } => self.kbm.mouse_move_relative(dx, dy).map_err(|e| e.to_string()),
            InputMessage::MouseMoveAbs { x, y } => self.kbm.mouse_move_absolute(x, y).map_err(|e| e.to_string()),
            InputMessage::MouseButton { button, down } => {
                if down {
                    c.held_buttons.insert(button);
                } else {
                    c.held_buttons.remove(&button);
                }
                self.kbm.mouse_button(button, down).map_err(|e| e.to_string())
            }
            InputMessage::Scroll { dx, dy } => self.kbm.scroll(dx, dy).map_err(|e| e.to_string()),
            InputMessage::GamepadConnected { index, kind } => match self.plug(client, index, kind) {
                Ok(_) => Ok(()),
                Err(outcome) => return outcome,
            },
            InputMessage::GamepadDisconnected { index } => self.unplug(client, index),
            InputMessage::Gamepad { index, seq, state } => {
                // State can overtake the (reliable) connect message.
                let slot = match self.plug(client, index, GamepadKind::Xbox) {
                    Ok(s) => s,
                    Err(outcome) => return outcome,
                };
                let s = &mut self.slots[slot as usize];
                if let Some(last) = s.last_seq {
                    if !seq_newer(seq, last) {
                        return DispatchOutcome::Ignored;
                    }
                }
                s.last_seq = Some(seq);
                s.state = state;
                self.pads.update(slot, &state).map_err(|e| e.to_string())
            }
            InputMessage::ReleaseAll {} => unreachable!("handled above"),
        };
        match result {
            Ok(()) => {
                if let Some(c) = self.clients.get_mut(&client) {
                    c.stats.applied += 1;
                }
                DispatchOutcome::Applied
            }
            Err(e) => self.backend_error(client, e),
        }
    }

    fn backend_error(&mut self, client: ClientId, e: String) -> DispatchOutcome {
        if let Some(c) = self.clients.get_mut(&client) {
            c.stats.backend_errors += 1;
        }
        if !self.pad_error_reported {
            tracing::warn!("input backend error: {e}");
            self.pad_error_reported = true;
        }
        DispatchOutcome::BackendError
    }

    fn slot_of(&self, client: ClientId, index: u8) -> Option<u8> {
        (0..MAX_GAMEPADS).find(|&i| self.slots[i as usize].owner == Some((client, index)))
    }

    /// The host slot of the client's pad `index`, plugging a virtual
    /// controller into the lowest free slot if it has none yet.
    fn plug(&mut self, client: ClientId, index: u8, kind: GamepadKind) -> Result<u8, DispatchOutcome> {
        if let Some(slot) = self.slot_of(client, index) {
            return Ok(slot);
        }
        let Some(slot) = (0..MAX_GAMEPADS).find(|&i| self.slots[i as usize].owner.is_none()) else {
            if let Some(c) = self.clients.get_mut(&client) {
                c.stats.pads_refused += 1;
                if !c.pad_full_reported {
                    c.pad_full_reported = true;
                    tracing::warn!(client, index, "all {MAX_GAMEPADS} host controllers are in use; pad refused");
                }
            }
            return Err(DispatchOutcome::NoFreeController);
        };
        if let Err(e) = self.pads.plug(slot, kind) {
            return Err(self.backend_error(client, e.to_string()));
        }
        self.slots[slot as usize] =
            HostSlot { owner: Some((client, index)), last_seq: None, state: GamepadState::default() };
        tracing::info!(client, index, slot, "client gamepad mapped to host controller");
        Ok(slot)
    }

    fn unplug(&mut self, client: ClientId, index: u8) -> Result<(), String> {
        let Some(slot) = self.slot_of(client, index) else { return Ok(()) };
        self.unplug_slot(slot)
    }

    fn unplug_slot(&mut self, slot: u8) -> Result<(), String> {
        let _ = self.pads.update(slot, &GamepadState::default());
        self.slots[slot as usize] = HostSlot::default();
        self.pads.unplug(slot).map_err(|e| e.to_string())
    }

    fn unplug_client_pads(&mut self, client: ClientId) {
        for slot in self.client_pads(client) {
            if let Err(e) = self.unplug_slot(slot) {
                tracing::warn!("removing virtual gamepad {slot}: {e}");
            }
        }
        if let Some(c) = self.clients.get_mut(&client) {
            c.pad_full_reported = false;
        }
    }

    fn release_keyboard_mouse(&mut self, client: ClientId) {
        let Some(c) = self.clients.get_mut(&client) else { return };
        let keys: Vec<u16> = c.held_keys.drain().collect();
        let buttons: Vec<MouseButton> = c.held_buttons.drain().collect();
        for usage in keys {
            if let Some(info) = keys::by_hid(usage) {
                if let Err(e) = self.kbm.key(info, false) {
                    tracing::warn!("releasing key {usage:#x}: {e}");
                }
            }
        }
        for b in buttons {
            if let Err(e) = self.kbm.mouse_button(b, false) {
                tracing::warn!("releasing mouse button {b:?}: {e}");
            }
        }
    }

    /// Release every key, mouse button and gamepad input `client` holds.
    /// Called on client request, focus loss, interruption and departure.
    pub fn release_client(&mut self, client: ClientId) {
        self.release_keyboard_mouse(client);
        for slot in self.client_pads(client) {
            let s = &mut self.slots[slot as usize];
            if !s.state.is_neutral() {
                s.state = GamepadState::default();
                if let Err(e) = self.pads.update(slot, &GamepadState::default()) {
                    tracing::warn!("neutralizing gamepad {slot}: {e}");
                }
            }
        }
    }

    /// Release everything every client holds.
    pub fn release_all(&mut self) {
        let ids: Vec<ClientId> = self.clients.keys().copied().collect();
        for id in ids {
            self.release_client(id);
        }
    }

    /// Rumble requests from games, addressed to the owning client with its
    /// own pad index. Must be called regularly: on Linux, games block in
    /// their force-feedback upload until the backend answers.
    pub fn poll_feedback(&mut self) -> Vec<(ClientId, Rumble)> {
        let slots = self.slots;
        let mut out = Vec::new();
        for r in self.pads.poll_feedback() {
            let Some((client, index)) = slots.get(r.index as usize).and_then(|s| s.owner) else { continue };
            if let Some(c) = self.clients.get_mut(&client) {
                c.stats.rumble += 1;
                out.push((client, Rumble { index, ..r }));
            }
        }
        out
    }

    /// Hosting ended: release everything and remove virtual controllers.
    pub fn disconnect(&mut self) {
        self.release_all();
        for slot in 0..MAX_GAMEPADS {
            if self.slots[slot as usize].owner.is_some() {
                let _ = self.unplug_slot(slot);
            }
        }
        self.allowed = false;
    }

    pub fn held_count(&self) -> usize {
        self.clients.values().map(|c| c.held_keys.len() + c.held_buttons.len()).sum()
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

    const A: ClientId = 1;
    const B: ClientId = 2;

    fn setup() -> (InputDispatcher, MockInput, MockController) {
        let kbm = MockInput::default();
        let pads = MockController::default();
        let mut d = InputDispatcher::new(Box::new(kbm.clone()), Box::new(pads.clone()));
        d.set_allowed(true);
        d.add_client(A, InputPermissions::ALL);
        (d, kbm, pads)
    }

    fn pad(index: u8, seq: u32, b: u32) -> InputMessage {
        InputMessage::Gamepad { index, seq, state: GamepadState { buttons: b, ..Default::default() } }
    }

    #[test]
    fn rumble_from_connected_pads_is_forwarded() {
        let (mut d, _, _) = setup();
        d.handle(A, InputMessage::GamepadConnected { index: 1, kind: GamepadKind::Xbox });
        let state = GamepadState { left_trigger: 1000, right_trigger: 2000, ..Default::default() };
        d.handle(A, InputMessage::Gamepad { index: 1, seq: 1, state });
        // The client's pad 1 got host slot 0; rumble goes back as pad 1.
        let expected = Rumble { index: 1, strong: 1000, weak: 2000, duration_ms: 0 };
        assert_eq!(d.poll_feedback(), vec![(A, expected)]);
        assert!(d.poll_feedback().is_empty(), "feedback is drained");
        assert_eq!(d.stats().rumble, 1);
        // Feedback for a pad that is gone is not forwarded.
        d.handle(A, InputMessage::GamepadDisconnected { index: 1 });
        assert!(d.poll_feedback().is_empty());
    }

    #[test]
    fn denied_without_permission() {
        let (mut d, kbm, _) = setup();
        d.set_allowed(false);
        assert_eq!(d.handle(A, InputMessage::Key { usage: 4, down: true, repeat: false }), DispatchOutcome::Denied);
        assert!(kbm.log.snapshot().is_empty());
        // Unknown clients are never let through.
        d.set_allowed(true);
        assert_eq!(d.handle(99, InputMessage::Key { usage: 4, down: true, repeat: false }), DispatchOutcome::Denied);
        assert!(kbm.log.snapshot().is_empty());
    }

    #[test]
    fn revoking_permission_releases_held_input() {
        let (mut d, kbm, pads) = setup();
        d.handle(A, InputMessage::Key { usage: 0xE1, down: true, repeat: false });
        d.handle(A, InputMessage::Key { usage: 0x1A, down: true, repeat: false });
        d.handle(A, InputMessage::MouseButton { button: MouseButton::Left, down: true });
        d.handle(A, pad(0, 1, buttons::A));
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
        d.handle(A, InputMessage::GamepadConnected { index: 1, kind: GamepadKind::PlayStation });
        d.handle(A, InputMessage::Key { usage: 0x2C, down: true, repeat: false });
        kbm.log.clear();
        d.disconnect();
        assert_eq!(kbm.log.snapshot(), vec![MockInputEvent::Key(0x2C, false)]);
        assert!(pads.log.snapshot().contains(&MockPadEvent::Unplug(0)));
        assert!(!d.allowed());
        assert_eq!(d.handle(A, InputMessage::Key { usage: 4, down: true, repeat: false }), DispatchOutcome::Denied);
    }

    #[test]
    fn malformed_and_unknown_messages_rejected() {
        let (mut d, kbm, _) = setup();
        assert_eq!(d.handle_text(A, "{\"type\":\"key\",\"usage\":1,\"down\":true}"), DispatchOutcome::Invalid);
        assert_eq!(d.handle_text(A, "{\"type\":\"run\",\"cmd\":\"calc.exe\"}"), DispatchOutcome::Invalid);
        assert_eq!(d.handle_text(A, "not json"), DispatchOutcome::Invalid);
        assert_eq!(d.handle_text(A, &"x".repeat(5000)), DispatchOutcome::Invalid);
        assert!(kbm.log.snapshot().is_empty());
        assert_eq!(d.stats().invalid, 4);
    }

    #[test]
    fn keyboard_flood_is_rate_limited() {
        let (mut d, _, _) = setup();
        let mut limited = 0;
        for i in 0..500 {
            if d.handle(A, InputMessage::Key { usage: 4, down: i % 2 == 0, repeat: false })
                == DispatchOutcome::RateLimited
            {
                limited += 1;
            }
        }
        assert!(limited > 300, "only {limited} limited");
    }

    #[test]
    fn stale_gamepad_packets_ignored_and_autoconnect() {
        let (mut d, _, pads) = setup();
        assert_eq!(d.handle(A, pad(2, 10, buttons::A)), DispatchOutcome::Applied);
        assert_eq!(d.handle(A, pad(2, 9, buttons::B)), DispatchOutcome::Ignored);
        assert_eq!(d.handle(A, pad(2, 10, buttons::B)), DispatchOutcome::Ignored);
        assert_eq!(d.handle(A, pad(2, 11, buttons::B)), DispatchOutcome::Applied);
        let log = pads.log.snapshot();
        assert_eq!(log[0], MockPadEvent::Plug(0, GamepadKind::Xbox));
        assert_eq!(log.len(), 3);
        // Wraparound.
        assert!(seq_newer(0, u32::MAX));
        assert!(!seq_newer(u32::MAX, 0));
    }

    #[test]
    fn client_autorepeat_ignored_when_host_repeats() {
        let (mut d, kbm, _) = setup();
        assert_eq!(d.handle(A, InputMessage::Key { usage: 4, down: true, repeat: true }), DispatchOutcome::Ignored);
        assert!(kbm.log.snapshot().is_empty());
    }

    #[test]
    fn release_all_honoured_without_permission() {
        let (mut d, kbm, _) = setup();
        d.handle(A, InputMessage::Key { usage: 4, down: true, repeat: false });
        d.allowed = false; // simulate: permission flag flipped without release
        kbm.log.clear();
        assert_eq!(d.handle(A, InputMessage::ReleaseAll {}), DispatchOutcome::Applied);
        assert_eq!(kbm.log.snapshot(), vec![MockInputEvent::Key(4, false)]);
    }

    #[test]
    fn clients_share_host_controllers_without_clashing() {
        let (mut d, _, pads) = setup();
        d.add_client(B, InputPermissions { keyboard_mouse: false, gamepads: true });
        // Both number their pad 0; they get different host controllers.
        assert_eq!(d.handle(A, pad(0, 1, buttons::A)), DispatchOutcome::Applied);
        assert_eq!(d.handle(B, pad(0, 1, buttons::B)), DispatchOutcome::Applied);
        assert_eq!(d.client_pads(A), vec![0]);
        assert_eq!(d.client_pads(B), vec![1]);
        let log = pads.log.snapshot();
        assert!(log.contains(&MockPadEvent::Update(0, GamepadState { buttons: buttons::A, ..Default::default() })));
        assert!(log.contains(&MockPadEvent::Update(1, GamepadState { buttons: buttons::B, ..Default::default() })));
        // Sequence numbers are tracked per pad, not shared between clients.
        assert_eq!(d.handle(B, pad(0, 2, 0)), DispatchOutcome::Applied);
        // Rumble on host controller 1 goes to B as its pad 0.
        let state = GamepadState { left_trigger: 500, ..Default::default() };
        d.handle(B, InputMessage::Gamepad { index: 0, seq: 3, state });
        assert_eq!(d.poll_feedback(), vec![(B, Rumble { index: 0, strong: 500, weak: 0, duration_ms: 0 })]);
    }

    #[test]
    fn fifth_pad_is_refused_until_a_slot_frees_up() {
        let (mut d, _, _) = setup();
        d.add_client(B, InputPermissions::ALL);
        for i in 0..3 {
            assert_eq!(d.handle(A, pad(i, 1, 0)), DispatchOutcome::Applied);
        }
        assert_eq!(d.handle(B, pad(0, 1, 0)), DispatchOutcome::Applied);
        assert_eq!(d.handle(B, pad(1, 1, 0)), DispatchOutcome::NoFreeController);
        assert_eq!(d.client_stats(B).unwrap().pads_refused, 1);
        // A unplugs one pad; B's second pad now fits.
        d.handle(A, InputMessage::GamepadDisconnected { index: 2 });
        assert_eq!(d.handle(B, pad(1, 2, 0)), DispatchOutcome::Applied);
        assert_eq!(d.client_pads(B), vec![2, 3]);
    }

    #[test]
    fn watchers_and_gamepad_only_players_are_limited() {
        let (mut d, kbm, _) = setup();
        d.add_client(B, InputPermissions::NONE);
        assert_eq!(d.handle(B, InputMessage::Key { usage: 4, down: true, repeat: false }), DispatchOutcome::Denied);
        assert_eq!(d.handle(B, pad(0, 1, buttons::A)), DispatchOutcome::Denied);
        assert!(d.client_pads(B).is_empty());
        d.set_permissions(B, InputPermissions { keyboard_mouse: false, gamepads: true });
        assert_eq!(d.handle(B, pad(0, 2, buttons::A)), DispatchOutcome::Applied);
        assert_eq!(d.handle(B, InputMessage::MouseMove { dx: 5, dy: 0 }), DispatchOutcome::Denied);
        assert!(kbm.log.snapshot().is_empty());
    }

    #[test]
    fn one_client_leaving_or_demoted_releases_only_its_own_input() {
        let (mut d, kbm, pads) = setup();
        d.add_client(B, InputPermissions::ALL);
        d.handle(A, InputMessage::Key { usage: 0x04, down: true, repeat: false });
        d.handle(B, InputMessage::Key { usage: 0x05, down: true, repeat: false });
        d.handle(A, pad(0, 1, buttons::A));
        d.handle(B, pad(0, 1, buttons::B));
        kbm.log.clear();
        pads.log.clear();

        // B is made a watcher: its key and pad are released, A's are not.
        d.set_permissions(B, InputPermissions::NONE);
        assert_eq!(kbm.log.snapshot(), vec![MockInputEvent::Key(0x05, false)]);
        assert!(pads.log.snapshot().contains(&MockPadEvent::Unplug(1)));
        assert!(!pads.log.snapshot().contains(&MockPadEvent::Unplug(0)));
        assert_eq!(d.client_pads(A), vec![0]);

        // A leaves: its key is released and its pad removed.
        kbm.log.clear();
        d.remove_client(A);
        assert_eq!(kbm.log.snapshot(), vec![MockInputEvent::Key(0x04, false)]);
        assert!(pads.log.snapshot().contains(&MockPadEvent::Unplug(0)));
        assert_eq!(d.held_count(), 0);
        assert_eq!(d.stats().applied, 4, "totals keep the departed client's counters");
    }
}
