//! Streaming window: presents decoded frames with softbuffer (CPU blit;
//! frames are scaled to the window by the GStreamer pipeline) and captures
//! keyboard, mouse and focus events.
//!
//! Hotkeys (Ctrl+Alt+Shift + key, not forwarded to the host):
//!   Q disconnect · F fullscreen · M mouse capture · S stats overlay · A mute

use std::num::NonZeroU32;
use std::rc::Rc;
use std::sync::Arc;

use crate::protocol::input::{InputMessage, MouseButton};
use crate::protocol::keys;
use winit::application::ApplicationHandler;
use winit::dpi::{PhysicalPosition, PhysicalSize};
use winit::event::{DeviceEvent, DeviceId, ElementState, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::keyboard::{KeyCode, ModifiersState, PhysicalKey};
use winit::window::{CursorGrabMode, Fullscreen, Window, WindowId};

use super::shared::{ClientCmd, ClientShared};
use super::sinks::LatestFrameSink;

#[derive(Debug, Clone)]
pub enum UiEvent {
    Frame,
    Title(String),
    ToggleFullscreen,
    Exit,
}

pub struct WindowOptions {
    pub title: String,
    pub fullscreen: bool,
    pub capture_mouse: bool,
}

struct App {
    opts: WindowOptions,
    shared: Arc<ClientShared>,
    frames: Arc<LatestFrameSink>,
    window: Option<Rc<Window>>,
    surface: Option<softbuffer::Surface<Rc<Window>, Rc<Window>>>,
    modifiers: ModifiersState,
    captured: bool,
    focused: bool,
    motion_acc: (f64, f64),
    size: PhysicalSize<u32>,
}

pub fn proxy_waker(proxy: EventLoopProxy<UiEvent>) -> impl Fn() + Send + Sync + 'static {
    move || {
        let _ = proxy.send_event(UiEvent::Frame);
    }
}

pub fn build_event_loop() -> anyhow::Result<EventLoop<UiEvent>> {
    Ok(EventLoop::<UiEvent>::with_user_event().build()?)
}

/// Run the window on the current (main) thread until the session ends.
pub fn run(
    event_loop: EventLoop<UiEvent>,
    opts: WindowOptions,
    shared: Arc<ClientShared>,
    frames: Arc<LatestFrameSink>,
) -> anyhow::Result<()> {
    event_loop.set_control_flow(ControlFlow::Wait);
    let mut app = App {
        captured: opts.capture_mouse,
        opts,
        shared,
        frames,
        window: None,
        surface: None,
        modifiers: ModifiersState::empty(),
        focused: true,
        motion_acc: (0.0, 0.0),
        size: PhysicalSize::new(1280, 720),
    };
    event_loop.run_app(&mut app)?;
    Ok(())
}

fn mouse_button(b: winit::event::MouseButton) -> Option<MouseButton> {
    Some(match b {
        winit::event::MouseButton::Left => MouseButton::Left,
        winit::event::MouseButton::Right => MouseButton::Right,
        winit::event::MouseButton::Middle => MouseButton::Middle,
        winit::event::MouseButton::Back => MouseButton::Back,
        winit::event::MouseButton::Forward => MouseButton::Forward,
        winit::event::MouseButton::Other(_) => return None,
    })
}

impl App {
    fn hotkey(&self) -> bool {
        self.modifiers.control_key() && self.modifiers.alt_key() && self.modifiers.shift_key()
    }

    fn set_capture(&mut self, on: bool) {
        let Some(w) = &self.window else { return };
        if on {
            let grabbed =
                w.set_cursor_grab(CursorGrabMode::Locked).or_else(|_| w.set_cursor_grab(CursorGrabMode::Confined));
            if let Err(e) = grabbed {
                tracing::warn!("pointer capture unavailable: {e}");
                return;
            }
            w.set_cursor_visible(false);
        } else {
            let _ = w.set_cursor_grab(CursorGrabMode::None);
            w.set_cursor_visible(true);
        }
        self.captured = on;
    }

    fn toggle_fullscreen(&self) {
        let Some(w) = &self.window else { return };
        // Borderless on the monitor the window is on.
        let fs = w.fullscreen().is_some();
        w.set_fullscreen((!fs).then(|| Fullscreen::Borderless(w.current_monitor())));
    }

    /// Area inside the window where the (letterboxed) video is drawn.
    fn video_rect(&self) -> (f64, f64, f64, f64) {
        let (ww, wh) = (self.size.width as f64, self.size.height as f64);
        let (sw, sh) = self.shared.stream_size.lock().map(|(w, h)| (w as f64, h as f64)).unwrap_or((ww, wh));
        let scale = f64::min(ww / sw.max(1.0), wh / sh.max(1.0));
        let (vw, vh) = (sw * scale, sh * scale);
        ((ww - vw) / 2.0, (wh - vh) / 2.0, vw, vh)
    }

    fn redraw(&mut self) {
        let (Some(surface), Some(window)) = (self.surface.as_mut(), self.window.as_ref()) else { return };
        let Some(frame) = self.frames.take() else { return };
        let (ww, wh) = (self.size.width.max(1), self.size.height.max(1));
        if surface.resize(NonZeroU32::new(ww).expect("nonzero"), NonZeroU32::new(wh).expect("nonzero")).is_err() {
            return;
        }
        let Ok(mut buf) = surface.buffer_mut() else { return };
        let (fw, fh, stride) = (frame.width as usize, frame.height as usize, frame.stride);
        let (ww, wh) = (ww as usize, wh as usize);
        // Normally the pipeline already scaled to the window size; during a
        // resize the frame may differ, so copy the overlapping region.
        let (cw, ch) = (fw.min(ww), fh.min(wh));
        let (ox, oy) = ((ww - cw) / 2, (wh - ch) / 2);
        if cw != ww || ch != wh {
            buf.fill(0);
        }
        frame.with_data(|data| {
            for y in 0..ch {
                let src = &data[y * stride..y * stride + cw * 4];
                let dst = &mut buf[(y + oy) * ww + ox..(y + oy) * ww + ox + cw];
                for (d, s) in dst.iter_mut().zip(src.as_chunks::<4>().0) {
                    // BGRx little-endian -> 0RGB
                    *d = u32::from_le_bytes([s[0], s[1], s[2], 0]);
                }
            }
        });
        window.pre_present_notify();
        if buf.present().is_ok() {
            self.shared.on_presented(&frame.times);
        }
    }
}

impl ApplicationHandler<UiEvent> for App {
    fn resumed(&mut self, el: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let attrs = Window::default_attributes()
            .with_title(&self.opts.title)
            .with_inner_size(PhysicalSize::new(1280u32, 720u32))
            .with_fullscreen(self.opts.fullscreen.then_some(Fullscreen::Borderless(None)));
        let window = match el.create_window(attrs) {
            Ok(w) => Rc::new(w),
            Err(e) => {
                tracing::error!("cannot create window: {e}");
                let _ = self.shared.cmd.send(ClientCmd::Disconnect);
                el.exit();
                return;
            }
        };
        let context = softbuffer::Context::new(window.clone()).expect("softbuffer context");
        self.surface = Some(softbuffer::Surface::new(&context, window.clone()).expect("softbuffer surface"));
        self.size = window.inner_size();
        self.shared.set_output_size(Some((self.size.width, self.size.height)));
        self.window = Some(window);
        if self.captured {
            self.set_capture(true);
        }
    }

    fn user_event(&mut self, el: &ActiveEventLoop, ev: UiEvent) {
        match ev {
            UiEvent::Frame => {
                // The media pipeline may have been (re)created since the
                // last resize; keep it scaling to our size.
                self.shared.set_output_size(Some((self.size.width, self.size.height)));
                if let Some(w) = &self.window {
                    w.request_redraw();
                }
            }
            UiEvent::Title(t) => {
                if let Some(w) = &self.window {
                    w.set_title(&t);
                }
            }
            UiEvent::ToggleFullscreen => {
                // Requested from the desktop app, which has the focus: bring
                // the stream to the front, or it goes fullscreen behind it.
                self.toggle_fullscreen();
                if let Some(w) = &self.window {
                    w.focus_window();
                }
            }
            UiEvent::Exit => el.exit(),
        }
    }

    fn window_event(&mut self, el: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => {
                self.shared.send_input(InputMessage::ReleaseAll {});
                let _ = self.shared.cmd.send(ClientCmd::Disconnect);
                el.exit();
            }
            WindowEvent::Resized(size) => {
                self.size = size;
                self.shared.set_output_size(Some((size.width.max(2), size.height.max(2))));
            }
            WindowEvent::RedrawRequested => self.redraw(),
            WindowEvent::Focused(f) => {
                self.focused = f;
                if !f {
                    // Never leave keys stuck on the host when we lose focus.
                    self.shared.send_input(InputMessage::ReleaseAll {});
                    if self.captured {
                        self.set_capture(false);
                        self.captured = self.opts.capture_mouse;
                    }
                } else if self.opts.capture_mouse {
                    self.set_capture(true);
                }
            }
            WindowEvent::ModifiersChanged(m) => self.modifiers = m.state(),
            WindowEvent::KeyboardInput { event, .. } => {
                let PhysicalKey::Code(code) = event.physical_key else { return };
                let down = event.state == ElementState::Pressed;
                if down && self.hotkey() {
                    let handled = match code {
                        KeyCode::KeyQ => {
                            self.shared.send_input(InputMessage::ReleaseAll {});
                            let _ = self.shared.cmd.send(ClientCmd::Disconnect);
                            true
                        }
                        KeyCode::KeyF => {
                            self.toggle_fullscreen();
                            true
                        }
                        KeyCode::KeyM => {
                            let on = !self.captured;
                            self.set_capture(on);
                            self.opts.capture_mouse = on;
                            true
                        }
                        KeyCode::KeyS => {
                            let _ = self.shared.cmd.send(ClientCmd::ToggleOverlay);
                            true
                        }
                        KeyCode::KeyA => {
                            let _ = self.shared.cmd.send(ClientCmd::ToggleMute);
                            true
                        }
                        _ => false,
                    };
                    if handled {
                        return;
                    }
                }
                let name = format!("{code:?}");
                if let Some(k) = keys::by_code(&name) {
                    self.shared.send_input(InputMessage::Key { usage: k.hid, down, repeat: event.repeat });
                }
            }
            WindowEvent::CursorMoved { position: PhysicalPosition { x, y }, .. } => {
                if self.captured {
                    return;
                }
                let (vx, vy, vw, vh) = self.video_rect();
                if vw <= 0.0 || vh <= 0.0 {
                    return;
                }
                let nx = ((x - vx) / vw).clamp(0.0, 1.0);
                let ny = ((y - vy) / vh).clamp(0.0, 1.0);
                self.shared
                    .send_input(InputMessage::MouseMoveAbs { x: (nx * 65535.0) as u16, y: (ny * 65535.0) as u16 });
            }
            WindowEvent::MouseInput { state, button, .. } => {
                if let Some(b) = mouse_button(button) {
                    self.shared
                        .send_input(InputMessage::MouseButton { button: b, down: state == ElementState::Pressed });
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let (dx, dy) = match delta {
                    MouseScrollDelta::LineDelta(x, y) => ((x * 120.0) as i32, (y * 120.0) as i32),
                    MouseScrollDelta::PixelDelta(p) => ((p.x * 2.4) as i32, (p.y * 2.4) as i32),
                };
                if dx != 0 || dy != 0 {
                    self.shared
                        .send_input(InputMessage::Scroll { dx: dx.clamp(-6000, 6000), dy: dy.clamp(-6000, 6000) });
                }
            }
            _ => {}
        }
    }

    fn device_event(&mut self, _el: &ActiveEventLoop, _id: DeviceId, event: DeviceEvent) {
        // Raw relative motion is used only while the pointer is captured.
        if let DeviceEvent::MouseMotion { delta } = event {
            if !self.captured || !self.focused {
                return;
            }
            self.motion_acc.0 += delta.0;
            self.motion_acc.1 += delta.1;
            let (dx, dy) = (self.motion_acc.0.trunc(), self.motion_acc.1.trunc());
            if dx != 0.0 || dy != 0.0 {
                self.motion_acc.0 -= dx;
                self.motion_acc.1 -= dy;
                self.shared.send_input(InputMessage::MouseMove { dx: dx as i32, dy: dy as i32 });
            }
        }
    }

    fn exiting(&mut self, _el: &ActiveEventLoop) {
        self.shared.send_input(InputMessage::ReleaseAll {});
    }
}
