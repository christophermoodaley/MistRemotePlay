//! Platform-neutral key codes.
//!
//! Keys travel as USB HID usage IDs (Keyboard/Keypad page 0x07), which
//! describe physical key positions independent of layout. Each entry maps to
//! the W3C `KeyboardEvent.code` name (also used by winit's `KeyCode`), the
//! Linux evdev code, and the Windows set-1 scancode.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KeyInfo {
    pub hid: u16,
    pub code: &'static str,
    pub evdev: u16,
    /// Windows scan code; 0 means "use `win_vk` instead".
    pub win_scan: u16,
    pub win_extended: bool,
    pub win_vk: u16,
}

const fn k(hid: u16, code: &'static str, evdev: u16, win_scan: u16, win_extended: bool) -> KeyInfo {
    KeyInfo { hid, code, evdev, win_scan, win_extended, win_vk: 0 }
}

pub static KEYS: &[KeyInfo] = &[
    k(0x04, "KeyA", 30, 0x1E, false),
    k(0x05, "KeyB", 48, 0x30, false),
    k(0x06, "KeyC", 46, 0x2E, false),
    k(0x07, "KeyD", 32, 0x20, false),
    k(0x08, "KeyE", 18, 0x12, false),
    k(0x09, "KeyF", 33, 0x21, false),
    k(0x0A, "KeyG", 34, 0x22, false),
    k(0x0B, "KeyH", 35, 0x23, false),
    k(0x0C, "KeyI", 23, 0x17, false),
    k(0x0D, "KeyJ", 36, 0x24, false),
    k(0x0E, "KeyK", 37, 0x25, false),
    k(0x0F, "KeyL", 38, 0x26, false),
    k(0x10, "KeyM", 50, 0x32, false),
    k(0x11, "KeyN", 49, 0x31, false),
    k(0x12, "KeyO", 24, 0x18, false),
    k(0x13, "KeyP", 25, 0x19, false),
    k(0x14, "KeyQ", 16, 0x10, false),
    k(0x15, "KeyR", 19, 0x13, false),
    k(0x16, "KeyS", 31, 0x1F, false),
    k(0x17, "KeyT", 20, 0x14, false),
    k(0x18, "KeyU", 22, 0x16, false),
    k(0x19, "KeyV", 47, 0x2F, false),
    k(0x1A, "KeyW", 17, 0x11, false),
    k(0x1B, "KeyX", 45, 0x2D, false),
    k(0x1C, "KeyY", 21, 0x15, false),
    k(0x1D, "KeyZ", 44, 0x2C, false),
    k(0x1E, "Digit1", 2, 0x02, false),
    k(0x1F, "Digit2", 3, 0x03, false),
    k(0x20, "Digit3", 4, 0x04, false),
    k(0x21, "Digit4", 5, 0x05, false),
    k(0x22, "Digit5", 6, 0x06, false),
    k(0x23, "Digit6", 7, 0x07, false),
    k(0x24, "Digit7", 8, 0x08, false),
    k(0x25, "Digit8", 9, 0x09, false),
    k(0x26, "Digit9", 10, 0x0A, false),
    k(0x27, "Digit0", 11, 0x0B, false),
    k(0x28, "Enter", 28, 0x1C, false),
    k(0x29, "Escape", 1, 0x01, false),
    k(0x2A, "Backspace", 14, 0x0E, false),
    k(0x2B, "Tab", 15, 0x0F, false),
    k(0x2C, "Space", 57, 0x39, false),
    k(0x2D, "Minus", 12, 0x0C, false),
    k(0x2E, "Equal", 13, 0x0D, false),
    k(0x2F, "BracketLeft", 26, 0x1A, false),
    k(0x30, "BracketRight", 27, 0x1B, false),
    k(0x31, "Backslash", 43, 0x2B, false),
    k(0x33, "Semicolon", 39, 0x27, false),
    k(0x34, "Quote", 40, 0x28, false),
    k(0x35, "Backquote", 41, 0x29, false),
    k(0x36, "Comma", 51, 0x33, false),
    k(0x37, "Period", 52, 0x34, false),
    k(0x38, "Slash", 53, 0x35, false),
    k(0x39, "CapsLock", 58, 0x3A, false),
    k(0x3A, "F1", 59, 0x3B, false),
    k(0x3B, "F2", 60, 0x3C, false),
    k(0x3C, "F3", 61, 0x3D, false),
    k(0x3D, "F4", 62, 0x3E, false),
    k(0x3E, "F5", 63, 0x3F, false),
    k(0x3F, "F6", 64, 0x40, false),
    k(0x40, "F7", 65, 0x41, false),
    k(0x41, "F8", 66, 0x42, false),
    k(0x42, "F9", 67, 0x43, false),
    k(0x43, "F10", 68, 0x44, false),
    k(0x44, "F11", 87, 0x57, false),
    k(0x45, "F12", 88, 0x58, false),
    k(0x46, "PrintScreen", 99, 0x37, true),
    k(0x47, "ScrollLock", 70, 0x46, false),
    KeyInfo { hid: 0x48, code: "Pause", evdev: 119, win_scan: 0, win_extended: false, win_vk: 0x13 },
    k(0x49, "Insert", 110, 0x52, true),
    k(0x4A, "Home", 102, 0x47, true),
    k(0x4B, "PageUp", 104, 0x49, true),
    k(0x4C, "Delete", 111, 0x53, true),
    k(0x4D, "End", 107, 0x4F, true),
    k(0x4E, "PageDown", 109, 0x51, true),
    k(0x4F, "ArrowRight", 106, 0x4D, true),
    k(0x50, "ArrowLeft", 105, 0x4B, true),
    k(0x51, "ArrowDown", 108, 0x50, true),
    k(0x52, "ArrowUp", 103, 0x48, true),
    KeyInfo { hid: 0x53, code: "NumLock", evdev: 69, win_scan: 0, win_extended: false, win_vk: 0x90 },
    k(0x54, "NumpadDivide", 98, 0x35, true),
    k(0x55, "NumpadMultiply", 55, 0x37, false),
    k(0x56, "NumpadSubtract", 74, 0x4A, false),
    k(0x57, "NumpadAdd", 78, 0x4E, false),
    k(0x58, "NumpadEnter", 96, 0x1C, true),
    k(0x59, "Numpad1", 79, 0x4F, false),
    k(0x5A, "Numpad2", 80, 0x50, false),
    k(0x5B, "Numpad3", 81, 0x51, false),
    k(0x5C, "Numpad4", 75, 0x4B, false),
    k(0x5D, "Numpad5", 76, 0x4C, false),
    k(0x5E, "Numpad6", 77, 0x4D, false),
    k(0x5F, "Numpad7", 71, 0x47, false),
    k(0x60, "Numpad8", 72, 0x48, false),
    k(0x61, "Numpad9", 73, 0x49, false),
    k(0x62, "Numpad0", 82, 0x52, false),
    k(0x63, "NumpadDecimal", 83, 0x53, false),
    k(0x64, "IntlBackslash", 86, 0x56, false),
    k(0x65, "ContextMenu", 127, 0x5D, true),
    k(0x67, "NumpadEqual", 117, 0x59, false),
    k(0x68, "F13", 183, 0x64, false),
    k(0x69, "F14", 184, 0x65, false),
    k(0x6A, "F15", 185, 0x66, false),
    k(0x6B, "F16", 186, 0x67, false),
    k(0x6C, "F17", 187, 0x68, false),
    k(0x6D, "F18", 188, 0x69, false),
    k(0x6E, "F19", 189, 0x6A, false),
    k(0x6F, "F20", 190, 0x6B, false),
    k(0x70, "F21", 191, 0x6C, false),
    k(0x71, "F22", 192, 0x6D, false),
    k(0x72, "F23", 193, 0x6E, false),
    k(0x73, "F24", 194, 0x76, false),
    k(0xE0, "ControlLeft", 29, 0x1D, false),
    k(0xE1, "ShiftLeft", 42, 0x2A, false),
    k(0xE2, "AltLeft", 56, 0x38, false),
    k(0xE3, "MetaLeft", 125, 0x5B, true),
    k(0xE4, "ControlRight", 97, 0x1D, true),
    k(0xE5, "ShiftRight", 54, 0x36, false),
    k(0xE6, "AltRight", 100, 0x38, true),
    k(0xE7, "MetaRight", 126, 0x5C, true),
];

pub fn by_hid(hid: u16) -> Option<&'static KeyInfo> {
    KEYS.iter().find(|k| k.hid == hid)
}

/// Looks up a W3C `KeyboardEvent.code` / winit `KeyCode` name. winit names
/// the meta keys `SuperLeft`/`SuperRight`; both spellings are accepted.
pub fn by_code(code: &str) -> Option<&'static KeyInfo> {
    let code = match code {
        "SuperLeft" | "OSLeft" => "MetaLeft",
        "SuperRight" | "OSRight" => "MetaRight",
        c => c,
    };
    KEYS.iter().find(|k| k.code == code)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn table_has_no_duplicates() {
        let hids: HashSet<_> = KEYS.iter().map(|k| k.hid).collect();
        let codes: HashSet<_> = KEYS.iter().map(|k| k.code).collect();
        let evdev: HashSet<_> = KEYS.iter().map(|k| k.evdev).collect();
        assert_eq!(hids.len(), KEYS.len());
        assert_eq!(codes.len(), KEYS.len());
        assert_eq!(evdev.len(), KEYS.len());
    }

    #[test]
    fn windows_mapping_is_unique_per_key() {
        let win: HashSet<_> = KEYS.iter().map(|k| (k.win_scan, k.win_extended, k.win_vk)).collect();
        assert_eq!(win.len(), KEYS.len());
    }

    #[test]
    fn lookups() {
        assert_eq!(by_code("KeyW").unwrap().evdev, 17);
        assert_eq!(by_code("SuperLeft").unwrap().hid, 0xE3);
        assert_eq!(by_hid(0x29).unwrap().code, "Escape");
        assert!(by_hid(0x01).is_none());
    }
}
