//! X11 input through the XTEST extension. Works without special
//! privileges for the X session the host runs in. Keycodes follow the
//! evdev/libinput convention used by Xorg and Xvfb (X keycode = evdev + 8).

use mist_protocol::input::MouseButton;
use mist_protocol::keys::KeyInfo;
use x11rb::connection::{Connection, RequestConnection};
use x11rb::protocol::xproto::{self, Window};
use x11rb::protocol::xtest::ConnectionExt as _;
use x11rb::rust_connection::RustConnection;

use crate::{InputBackend, InputError, Result, TargetRect};

pub struct XTestInput {
    conn: RustConnection,
    root: Window,
    target: TargetRect,
    scroll_acc: (i32, i32),
}

fn err(e: impl std::fmt::Display) -> InputError {
    InputError::Injection(e.to_string())
}

impl XTestInput {
    pub fn connect(display: Option<&str>, target: TargetRect) -> Result<Self> {
        let (conn, screen) = x11rb::connect(display).map_err(|e| InputError::Unavailable(format!("X display: {e}")))?;
        let ext = conn
            .extension_information(x11rb::protocol::xtest::X11_EXTENSION_NAME)
            .map_err(|e| InputError::Unavailable(e.to_string()))?;
        if ext.is_none() {
            return Err(InputError::Unavailable("X server lacks the XTEST extension".into()));
        }
        let root = conn.setup().roots[screen].root;
        Ok(XTestInput { conn, root, target, scroll_acc: (0, 0) })
    }

    fn fake(&self, ty: u8, detail: u8, x: i16, y: i16) -> Result<()> {
        self.conn.xtest_fake_input(ty, detail, x11rb::CURRENT_TIME, self.root, x, y, 0).map_err(err)?;
        Ok(())
    }

    fn flush(&self) -> Result<()> {
        self.conn.flush().map_err(err)
    }

    fn click(&self, button: u8, times: i32) -> Result<()> {
        for _ in 0..times {
            self.fake(xproto::BUTTON_PRESS_EVENT, button, 0, 0)?;
            self.fake(xproto::BUTTON_RELEASE_EVENT, button, 0, 0)?;
        }
        Ok(())
    }
}

fn x_button(b: MouseButton) -> u8 {
    match b {
        MouseButton::Left => 1,
        MouseButton::Middle => 2,
        MouseButton::Right => 3,
        MouseButton::Back => 8,
        MouseButton::Forward => 9,
    }
}

impl InputBackend for XTestInput {
    fn name(&self) -> &'static str {
        "x11-xtest"
    }

    fn key(&mut self, key: &'static KeyInfo, down: bool) -> Result<()> {
        let keycode = u8::try_from(key.evdev + 8).map_err(|_| err("keycode out of X11 range"))?;
        let ty = if down { xproto::KEY_PRESS_EVENT } else { xproto::KEY_RELEASE_EVENT };
        self.fake(ty, keycode, 0, 0)?;
        self.flush()
    }

    fn mouse_move_relative(&mut self, dx: i32, dy: i32) -> Result<()> {
        // detail = 1 marks the motion as relative.
        self.fake(xproto::MOTION_NOTIFY_EVENT, 1, dx.clamp(-32768, 32767) as i16, dy.clamp(-32768, 32767) as i16)?;
        self.flush()
    }

    fn mouse_move_absolute(&mut self, x: u16, y: u16) -> Result<()> {
        let (px, py) = self.target.map(x, y);
        self.fake(xproto::MOTION_NOTIFY_EVENT, 0, px.clamp(0, 32767) as i16, py.clamp(0, 32767) as i16)?;
        self.flush()
    }

    fn mouse_button(&mut self, button: MouseButton, down: bool) -> Result<()> {
        let ty = if down { xproto::BUTTON_PRESS_EVENT } else { xproto::BUTTON_RELEASE_EVENT };
        self.fake(ty, x_button(button), 0, 0)?;
        self.flush()
    }

    fn scroll(&mut self, dx: i32, dy: i32) -> Result<()> {
        // X11 scrolls in whole notches (buttons 4-7); keep the remainder.
        self.scroll_acc.0 += dx;
        self.scroll_acc.1 += dy;
        let (nx, ny) = (self.scroll_acc.0 / 120, self.scroll_acc.1 / 120);
        self.scroll_acc.0 -= nx * 120;
        self.scroll_acc.1 -= ny * 120;
        if ny > 0 {
            self.click(4, ny)?;
        } else if ny < 0 {
            self.click(5, -ny)?;
        }
        if nx > 0 {
            self.click(7, nx)?;
        } else if nx < 0 {
            self.click(6, -nx)?;
        }
        self.flush()
    }
}
