//! Injects keyboard/mouse events with XTEST into a real X server and reads
//! them back from a test window. Skipped when no X display is available.
#![cfg(target_os = "linux")]

use std::time::{Duration, Instant};

use mist_input_api::x11::XTestInput;
use mist_input_api::{InputBackend, TargetRect};
use mist_protocol::input::MouseButton;
use mist_protocol::keys;
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{ConnectionExt as _, CreateWindowAux, EventMask, InputFocus, WindowClass};
use x11rb::protocol::Event;

#[test]
fn xtest_events_reach_x_clients() {
    let Ok(display) = std::env::var("DISPLAY") else {
        eprintln!("skipping: DISPLAY not set");
        return;
    };
    let (conn, screen_num) = x11rb::connect(Some(&display)).expect("connect to X");
    let screen = &conn.setup().roots[screen_num];
    let (w, h) = (screen.width_in_pixels, screen.height_in_pixels);
    let win = conn.generate_id().unwrap();
    conn.create_window(
        screen.root_depth,
        win,
        screen.root,
        0,
        0,
        w,
        h,
        0,
        WindowClass::INPUT_OUTPUT,
        screen.root_visual,
        &CreateWindowAux::new().override_redirect(1).event_mask(
            EventMask::KEY_PRESS | EventMask::KEY_RELEASE | EventMask::BUTTON_PRESS | EventMask::BUTTON_RELEASE | EventMask::POINTER_MOTION,
        ),
    )
    .unwrap();
    conn.map_window(win).unwrap();
    conn.flush().unwrap();
    std::thread::sleep(Duration::from_millis(200));
    conn.set_input_focus(InputFocus::POINTER_ROOT, win, x11rb::CURRENT_TIME).unwrap();
    conn.flush().unwrap();

    let target = TargetRect { x: 0, y: 0, width: w as u32, height: h as u32 };
    let mut input = XTestInput::connect(Some(&display), target).expect("xtest");
    let a = keys::by_code("KeyA").unwrap();
    input.mouse_move_absolute(32768, 16384).unwrap();
    input.key(a, true).unwrap();
    input.key(a, false).unwrap();
    input.mouse_button(MouseButton::Left, true).unwrap();
    input.mouse_button(MouseButton::Left, false).unwrap();
    input.mouse_move_relative(10, 5).unwrap();
    input.scroll(0, 240).unwrap();

    let mut seen = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline {
        match conn.poll_for_event().unwrap() {
            Some(Event::KeyPress(e)) => seen.push(format!("kp{}", e.detail)),
            Some(Event::KeyRelease(e)) => seen.push(format!("kr{}", e.detail)),
            Some(Event::ButtonPress(e)) => seen.push(format!("bp{}", e.detail)),
            Some(Event::ButtonRelease(e)) => seen.push(format!("br{}", e.detail)),
            Some(Event::MotionNotify(e)) => seen.push(format!("m{},{}", e.root_x, e.root_y)),
            Some(_) => {}
            None => std::thread::sleep(Duration::from_millis(10)),
        }
    }
    eprintln!("events: {seen:?}");
    let (cx, cy) = target.map(32768, 16384);
    assert!(seen.contains(&format!("m{cx},{cy}")), "absolute move missing");
    // 'A' is evdev 30 -> X keycode 38.
    assert!(seen.contains(&"kp38".to_string()) && seen.contains(&"kr38".to_string()), "key events missing");
    assert!(seen.contains(&"bp1".to_string()) && seen.contains(&"br1".to_string()), "button events missing");
    assert!(seen.contains(&format!("m{},{}", cx + 10, cy + 5)), "relative move missing");
    assert_eq!(seen.iter().filter(|e| *e == "bp4").count(), 2, "two scroll-up notches expected");
    conn.destroy_window(win).unwrap();
    conn.flush().unwrap();
}
