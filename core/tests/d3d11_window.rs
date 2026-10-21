//! The Direct3D 11 video sink draws into a winit window, as the streaming
//! window uses it. Opens a window for a moment; run with `--ignored`.

#![cfg(windows)]

use std::time::{Duration, Instant};

use mist_core::media::WindowTarget;
use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::platform::pump_events::EventLoopExtPumpEvents;
use winit::platform::windows::EventLoopBuilderExtWindows;
use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
use winit::window::{Window, WindowId};

#[derive(Default)]
struct App {
    window: Option<Window>,
}

impl ApplicationHandler for App {
    fn resumed(&mut self, el: &ActiveEventLoop) {
        if self.window.is_none() {
            let attrs = Window::default_attributes().with_title("Mist D3D11 sink test");
            self.window = Some(el.create_window(attrs).expect("window"));
        }
    }
    fn window_event(&mut self, _: &ActiveEventLoop, _: WindowId, _: WindowEvent) {}
}

#[test]
#[ignore]
fn d3d11_sink_draws_into_a_winit_window() {
    let mut el = EventLoop::builder().with_any_thread(true).build().unwrap();
    let mut app = App::default();
    let start = Instant::now();
    while app.window.is_none() && start.elapsed() < Duration::from_secs(5) {
        el.pump_app_events(Some(Duration::from_millis(10)), &mut app);
    }
    let window = app.window.as_ref().expect("window opened");
    let RawWindowHandle::Win32(h) = window.window_handle().unwrap().as_raw() else { panic!("not a Win32 window") };
    let target = WindowTarget::Win32 { hwnd: h.hwnd.get() as u64 };
    // The sink sets up its child window through the window's thread, so
    // keep pumping messages while it renders.
    let probe = std::thread::spawn(move || mist_core::media::window_sink_works(target));
    while !probe.is_finished() {
        el.pump_app_events(Some(Duration::from_millis(5)), &mut app);
    }
    let ms = probe.join().unwrap().expect("the D3D11 sink rendered into the window");
    println!("D3D11 sink: {ms:.2} ms per 1080p frame");
}
