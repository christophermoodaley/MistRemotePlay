//! Windows keyboard/mouse injection with SendInput.
//!
//! Keys are sent as scan codes (layout independent). Windows' UIPI blocks
//! injection into windows of a higher integrity level (e.g. elevated apps,
//! UAC prompts) unless the host itself runs elevated; see
//! docs/windows-setup.md. Injected keys are not autorepeated by Windows, so
//! client autorepeat is forwarded.

use mist_protocol::input::MouseButton;
use mist_protocol::keys::KeyInfo;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBDINPUT, KEYBD_EVENT_FLAGS, KEYEVENTF_EXTENDEDKEY,
    KEYEVENTF_KEYUP, KEYEVENTF_SCANCODE, MOUSEEVENTF_ABSOLUTE, MOUSEEVENTF_HWHEEL, MOUSEEVENTF_LEFTDOWN,
    MOUSEEVENTF_LEFTUP, MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP, MOUSEEVENTF_MOVE, MOUSEEVENTF_RIGHTDOWN,
    MOUSEEVENTF_RIGHTUP, MOUSEEVENTF_VIRTUALDESK, MOUSEEVENTF_WHEEL, MOUSEEVENTF_XDOWN, MOUSEEVENTF_XUP, MOUSEINPUT,
    MOUSE_EVENT_FLAGS, VIRTUAL_KEY,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetSystemMetrics, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN,
};

use crate::{InputBackend, InputError, Result, TargetRect};

const XBUTTON1: u32 = 1;
const XBUTTON2: u32 = 2;

pub struct SendInputBackend {
    target: TargetRect,
}

impl SendInputBackend {
    pub fn new(target: TargetRect) -> Self {
        SendInputBackend { target }
    }
}

fn send(inputs: &[INPUT]) -> Result<()> {
    // SAFETY: `inputs` is a valid slice of initialized INPUT structures.
    let sent = unsafe { SendInput(inputs, std::mem::size_of::<INPUT>() as i32) };
    if sent as usize != inputs.len() {
        return Err(InputError::Injection(
            "SendInput was blocked (UIPI: target window may be elevated, or the desktop is locked)".into(),
        ));
    }
    Ok(())
}

fn mouse(dx: i32, dy: i32, data: u32, flags: MOUSE_EVENT_FLAGS) -> INPUT {
    INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 { mi: MOUSEINPUT { dx, dy, mouseData: data, dwFlags: flags, time: 0, dwExtraInfo: 0 } },
    }
}

impl InputBackend for SendInputBackend {
    fn name(&self) -> &'static str {
        "win32-sendinput"
    }

    fn wants_client_repeat(&self) -> bool {
        true
    }

    fn key(&mut self, key: &'static KeyInfo, down: bool) -> Result<()> {
        let mut flags = KEYBD_EVENT_FLAGS(0);
        let (vk, scan) = if key.win_scan == 0 {
            (key.win_vk, 0)
        } else {
            flags |= KEYEVENTF_SCANCODE;
            if key.win_extended {
                flags |= KEYEVENTF_EXTENDEDKEY;
            }
            (0, key.win_scan)
        };
        if !down {
            flags |= KEYEVENTF_KEYUP;
        }
        let input = INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT { wVk: VIRTUAL_KEY(vk), wScan: scan, dwFlags: flags, time: 0, dwExtraInfo: 0 },
            },
        };
        send(&[input])
    }

    fn mouse_move_relative(&mut self, dx: i32, dy: i32) -> Result<()> {
        send(&[mouse(dx, dy, 0, MOUSEEVENTF_MOVE)])
    }

    fn mouse_move_absolute(&mut self, x: u16, y: u16) -> Result<()> {
        let (px, py) = self.target.map(x, y);
        // SAFETY: GetSystemMetrics has no preconditions.
        let (vx, vy, vw, vh) = unsafe {
            (
                GetSystemMetrics(SM_XVIRTUALSCREEN),
                GetSystemMetrics(SM_YVIRTUALSCREEN),
                GetSystemMetrics(SM_CXVIRTUALSCREEN).max(1),
                GetSystemMetrics(SM_CYVIRTUALSCREEN).max(1),
            )
        };
        let nx = (((px - vx) as i64 * 65535) / (vw - 1).max(1) as i64) as i32;
        let ny = (((py - vy) as i64 * 65535) / (vh - 1).max(1) as i64) as i32;
        send(&[mouse(nx, ny, 0, MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK)])
    }

    fn mouse_button(&mut self, button: MouseButton, down: bool) -> Result<()> {
        let (flags, data) = match (button, down) {
            (MouseButton::Left, true) => (MOUSEEVENTF_LEFTDOWN, 0),
            (MouseButton::Left, false) => (MOUSEEVENTF_LEFTUP, 0),
            (MouseButton::Right, true) => (MOUSEEVENTF_RIGHTDOWN, 0),
            (MouseButton::Right, false) => (MOUSEEVENTF_RIGHTUP, 0),
            (MouseButton::Middle, true) => (MOUSEEVENTF_MIDDLEDOWN, 0),
            (MouseButton::Middle, false) => (MOUSEEVENTF_MIDDLEUP, 0),
            (MouseButton::Back, true) => (MOUSEEVENTF_XDOWN, XBUTTON1),
            (MouseButton::Back, false) => (MOUSEEVENTF_XUP, XBUTTON1),
            (MouseButton::Forward, true) => (MOUSEEVENTF_XDOWN, XBUTTON2),
            (MouseButton::Forward, false) => (MOUSEEVENTF_XUP, XBUTTON2),
        };
        send(&[mouse(0, 0, data, flags)])
    }

    fn scroll(&mut self, dx: i32, dy: i32) -> Result<()> {
        let mut v = Vec::new();
        if dy != 0 {
            v.push(mouse(0, 0, dy as u32, MOUSEEVENTF_WHEEL));
        }
        if dx != 0 {
            v.push(mouse(0, 0, dx as u32, MOUSEEVENTF_HWHEEL));
        }
        if v.is_empty() {
            return Ok(());
        }
        send(&v)
    }
}
