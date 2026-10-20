//! Linux keyboard/mouse through /dev/uinput. Works on Wayland and X11
//! (and the console) because events enter the kernel input stack; requires
//! write access to /dev/uinput (see the README, Linux and Raspberry Pi).
//!
//! Two devices are created: a keyboard + relative mouse, and an absolute
//! pointer (0..65535 on both axes) used for absolute positioning. The
//! compositor maps the absolute device across its whole output layout, so
//! absolute positioning is exact only for single-monitor hosts.

use crate::protocol::input::MouseButton;
use crate::protocol::keys::{KeyInfo, KEYS};
use evdev::uinput::VirtualDevice;
use evdev::{
    AbsInfo, AbsoluteAxisCode, AttributeSet, BusType, EventType, InputEvent, InputId, KeyCode, RelativeAxisCode,
    UinputAbsSetup,
};

use crate::input::{InputBackend, InputError, Result, TargetRect};

pub struct UinputInput {
    kbm: VirtualDevice,
    abs: VirtualDevice,
    _target: TargetRect,
}

fn io(e: std::io::Error) -> InputError {
    match e.kind() {
        std::io::ErrorKind::PermissionDenied => {
            InputError::Unavailable("no permission to open /dev/uinput (see the README, Linux and Raspberry Pi)".into())
        }
        std::io::ErrorKind::NotFound => InputError::Unavailable("/dev/uinput does not exist (modprobe uinput)".into()),
        _ => InputError::Injection(e.to_string()),
    }
}

fn btn_code(b: MouseButton) -> KeyCode {
    match b {
        MouseButton::Left => KeyCode::BTN_LEFT,
        MouseButton::Right => KeyCode::BTN_RIGHT,
        MouseButton::Middle => KeyCode::BTN_MIDDLE,
        MouseButton::Back => KeyCode::BTN_SIDE,
        MouseButton::Forward => KeyCode::BTN_EXTRA,
    }
}

impl UinputInput {
    pub fn create(target: TargetRect) -> Result<Self> {
        let mut keys = AttributeSet::<KeyCode>::new();
        for k in KEYS {
            keys.insert(KeyCode::new(k.evdev));
        }
        for b in MouseButton::ALL {
            keys.insert(btn_code(b));
        }
        let mut rel = AttributeSet::<RelativeAxisCode>::new();
        for a in [
            RelativeAxisCode::REL_X,
            RelativeAxisCode::REL_Y,
            RelativeAxisCode::REL_WHEEL,
            RelativeAxisCode::REL_HWHEEL,
            RelativeAxisCode::REL_WHEEL_HI_RES,
            RelativeAxisCode::REL_HWHEEL_HI_RES,
        ] {
            rel.insert(a);
        }
        let kbm = VirtualDevice::builder()
            .map_err(io)?
            .name("Mist Virtual Keyboard and Mouse")
            .input_id(InputId::new(BusType::BUS_VIRTUAL, 0x4d49, 0x0001, 1))
            .with_keys(&keys)
            .map_err(io)?
            .with_relative_axes(&rel)
            .map_err(io)?
            .build()
            .map_err(io)?;

        let mut abs_keys = AttributeSet::<KeyCode>::new();
        abs_keys.insert(KeyCode::BTN_LEFT);
        let info = AbsInfo::new(0, 0, 65535, 0, 0, 0);
        let abs = VirtualDevice::builder()
            .map_err(io)?
            .name("Mist Virtual Absolute Pointer")
            .input_id(InputId::new(BusType::BUS_VIRTUAL, 0x4d49, 0x0002, 1))
            .with_keys(&abs_keys)
            .map_err(io)?
            .with_absolute_axis(&UinputAbsSetup::new(AbsoluteAxisCode::ABS_X, info))
            .map_err(io)?
            .with_absolute_axis(&UinputAbsSetup::new(AbsoluteAxisCode::ABS_Y, info))
            .map_err(io)?
            .build()
            .map_err(io)?;
        tracing::info!("uinput keyboard/mouse devices created");
        Ok(UinputInput { kbm, abs, _target: target })
    }
}

impl InputBackend for UinputInput {
    fn name(&self) -> &'static str {
        "uinput"
    }

    fn key(&mut self, key: &'static KeyInfo, down: bool) -> Result<()> {
        self.kbm.emit(&[InputEvent::new(EventType::KEY.0, key.evdev, down as i32)]).map_err(io)
    }

    fn mouse_move_relative(&mut self, dx: i32, dy: i32) -> Result<()> {
        self.kbm
            .emit(&[
                InputEvent::new(EventType::RELATIVE.0, RelativeAxisCode::REL_X.0, dx),
                InputEvent::new(EventType::RELATIVE.0, RelativeAxisCode::REL_Y.0, dy),
            ])
            .map_err(io)
    }

    fn mouse_move_absolute(&mut self, x: u16, y: u16) -> Result<()> {
        self.abs
            .emit(&[
                InputEvent::new(EventType::ABSOLUTE.0, AbsoluteAxisCode::ABS_X.0, x as i32),
                InputEvent::new(EventType::ABSOLUTE.0, AbsoluteAxisCode::ABS_Y.0, y as i32),
            ])
            .map_err(io)
    }

    fn mouse_button(&mut self, button: MouseButton, down: bool) -> Result<()> {
        self.kbm.emit(&[InputEvent::new(EventType::KEY.0, btn_code(button).0, down as i32)]).map_err(io)
    }

    fn scroll(&mut self, dx: i32, dy: i32) -> Result<()> {
        let mut ev = Vec::new();
        if dy != 0 {
            ev.push(InputEvent::new(EventType::RELATIVE.0, RelativeAxisCode::REL_WHEEL_HI_RES.0, dy));
            if dy % 120 == 0 {
                ev.push(InputEvent::new(EventType::RELATIVE.0, RelativeAxisCode::REL_WHEEL.0, dy / 120));
            }
        }
        if dx != 0 {
            ev.push(InputEvent::new(EventType::RELATIVE.0, RelativeAxisCode::REL_HWHEEL_HI_RES.0, dx));
            if dx % 120 == 0 {
                ev.push(InputEvent::new(EventType::RELATIVE.0, RelativeAxisCode::REL_HWHEEL.0, dx / 120));
            }
        }
        if ev.is_empty() {
            return Ok(());
        }
        self.kbm.emit(&ev).map_err(io)
    }
}
