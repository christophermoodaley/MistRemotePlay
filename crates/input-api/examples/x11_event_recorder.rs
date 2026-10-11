//! Test tool: covers most of the X screen with a window and prints received
//! input events as JSON lines (used by tests/e2e to verify remote input).
//! The top-left corner (400 x 200 px) stays uncovered so moving content
//! there (e.g. a running clock) keeps the captured screen changing; every
//! point the tests click or move to lies inside the window.
#[cfg(target_os = "linux")]
fn main() {
    use x11rb::connection::Connection;
    use x11rb::protocol::xproto::{ConnectionExt as _, CreateWindowAux, EventMask, InputFocus, WindowClass};
    use x11rb::protocol::Event;
    const LEFT: u16 = 400;
    const TOP: u16 = 200;
    let (conn, screen_num) = x11rb::connect(None).expect("X display");
    let screen = &conn.setup().roots[screen_num];
    let win = conn.generate_id().unwrap();
    conn.create_window(
        screen.root_depth,
        win,
        screen.root,
        LEFT as i16,
        TOP as i16,
        screen.width_in_pixels.saturating_sub(LEFT),
        screen.height_in_pixels.saturating_sub(TOP),
        0,
        WindowClass::INPUT_OUTPUT,
        screen.root_visual,
        &CreateWindowAux::new().override_redirect(1).background_pixel(screen.white_pixel).event_mask(
            EventMask::KEY_PRESS
                | EventMask::KEY_RELEASE
                | EventMask::BUTTON_PRESS
                | EventMask::BUTTON_RELEASE
                | EventMask::POINTER_MOTION,
        ),
    )
    .unwrap();
    conn.map_window(win).unwrap();
    conn.flush().unwrap();
    std::thread::sleep(std::time::Duration::from_millis(200));
    conn.set_input_focus(InputFocus::POINTER_ROOT, win, x11rb::CURRENT_TIME).unwrap();
    conn.flush().unwrap();
    println!("{{\"event\":\"ready\"}}");
    loop {
        let ev = conn.wait_for_event().expect("X connection");
        let line = match ev {
            Event::KeyPress(e) => format!("{{\"event\":\"key\",\"down\":true,\"keycode\":{}}}", e.detail),
            Event::KeyRelease(e) => format!("{{\"event\":\"key\",\"down\":false,\"keycode\":{}}}", e.detail),
            Event::ButtonPress(e) => format!("{{\"event\":\"button\",\"down\":true,\"button\":{}}}", e.detail),
            Event::ButtonRelease(e) => format!("{{\"event\":\"button\",\"down\":false,\"button\":{}}}", e.detail),
            Event::MotionNotify(e) => format!("{{\"event\":\"motion\",\"x\":{},\"y\":{}}}", e.root_x, e.root_y),
            _ => continue,
        };
        println!("{line}");
    }
}

#[cfg(not(target_os = "linux"))]
fn main() {}
