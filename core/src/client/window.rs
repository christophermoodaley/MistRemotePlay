//! Streaming window: presents decoded frames with softbuffer (CPU blit;
//! frames are scaled to the window by the GStreamer pipeline) and captures
//! keyboard, mouse and focus events.
//!
//! Hotkeys (Ctrl+Alt+Shift + key, not forwarded to the host):
//!   Q disconnect · F fullscreen · M mouse capture · S stats overlay · A mute

use std::num::NonZeroU32;
use std::rc::Rc;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::protocol::input::{InputMessage, MouseButton};
use crate::protocol::keys;
use winit::application::ApplicationHandler;
use winit::dpi::{PhysicalPosition, PhysicalSize};
use winit::event::{DeviceEvent, DeviceId, ElementState, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::keyboard::{KeyCode, ModifiersState, PhysicalKey};
use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
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
    /// Video has been shown; until then the connecting screen is drawn.
    showing_video: bool,
    started: Instant,
    next_splash: Instant,
}

/// Connecting-screen animation rate.
const SPLASH_FRAME: Duration = Duration::from_millis(33);

pub fn proxy_waker(proxy: EventLoopProxy<UiEvent>) -> impl Fn() + Send + Sync + 'static {
    move || {
        let _ = proxy.send_event(UiEvent::Frame);
    }
}

/// Create the window's event loop. On a Wayland desktop with XWayland
/// (Raspberry Pi OS, most current distributions) the window is opened
/// through XWayland unless `renderer` is software: the GPU video renderer
/// draws into X11 windows only, and the CPU renderer is too slow for 1080p
/// on small ARM boards.
pub fn build_event_loop(renderer: crate::config::Renderer) -> anyhow::Result<EventLoop<UiEvent>> {
    #[allow(unused_mut)]
    let mut builder = EventLoop::<UiEvent>::with_user_event();
    #[cfg(target_os = "linux")]
    if renderer != crate::config::Renderer::Software
        && std::env::var_os("WAYLAND_DISPLAY").is_some()
        && std::env::var_os("DISPLAY").is_some()
    {
        use winit::platform::x11::EventLoopBuilderExtX11;
        tracing::info!("Wayland session: opening the streaming window through XWayland for the GPU renderer");
        builder.with_x11();
    }
    #[cfg(not(target_os = "linux"))]
    let _ = renderer;
    Ok(builder.build()?)
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
        showing_video: false,
        started: Instant::now(),
        next_splash: Instant::now(),
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

    /// Draw the connecting screen (until the first frame is shown).
    fn draw_splash(&mut self) {
        let (Some(surface), Some(window)) = (self.surface.as_mut(), self.window.as_ref()) else { return };
        let (ww, wh) = (self.size.width.max(1), self.size.height.max(1));
        if surface.resize(NonZeroU32::new(ww).expect("nonzero"), NonZeroU32::new(wh).expect("nonzero")).is_err() {
            return;
        }
        let Ok(mut buf) = surface.buffer_mut() else { return };
        let status = self.shared.status.lock().clone();
        super::splash::draw(&mut buf, ww as usize, wh as usize, self.started.elapsed().as_secs_f32(), &status);
        window.pre_present_notify();
        let _ = buf.present();
    }

    fn redraw(&mut self) {
        if !self.showing_video && self.shared.frames_presented.load(Ordering::Relaxed) > 0 {
            self.showing_video = true;
        }
        if self.shared.gpu_presenting.load(Ordering::Relaxed) {
            // The GPU video sink draws into this window itself.
            if !self.showing_video {
                self.draw_splash();
            }
            return;
        }
        let Some(frame) = self.frames.take() else {
            if !self.showing_video {
                self.draw_splash();
            }
            return;
        };
        self.showing_video = true;
        let (Some(surface), Some(window)) = (self.surface.as_mut(), self.window.as_ref()) else { return };
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

/// The native window the GPU video sink can draw into, if this platform's
/// GPU renderer supports it (Win32 with Direct3D 11, X11 with OpenGL).
// `c_ulong` is 64-bit on Linux but 32-bit on Windows targets.
#[allow(clippy::unnecessary_cast)]
fn video_target(window: &Window) -> Option<crate::media::WindowTarget> {
    match window.window_handle().ok()?.as_raw() {
        RawWindowHandle::Xlib(h) => Some(crate::media::WindowTarget::X11 { window: h.window as u64 }),
        RawWindowHandle::Xcb(h) => Some(crate::media::WindowTarget::X11 { window: h.window.get() as u64 }),
        RawWindowHandle::Win32(h) => Some(crate::media::WindowTarget::Win32 { hwnd: h.hwnd.get() as u64 }),

        _ => None,
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
        *self.shared.video_target.lock() = video_target(&window);
        // The session asks the host to match this monitor's size.
        if let Some(m) = window.current_monitor().or_else(|| el.primary_monitor()) {
            let s = m.size();
            *self.shared.display_size.lock() = Some((s.width, s.height));
        }
        // Paint the connecting screen right away instead of a blank window.
        window.request_redraw();
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
                self.shared.expose();
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

    fn about_to_wait(&mut self, el: &ActiveEventLoop) {
        if self.showing_video {
            el.set_control_flow(ControlFlow::Wait);
            return;
        }
        // Animate the connecting screen.
        let now = Instant::now();
        if now >= self.next_splash {
            if let Some(w) = &self.window {
                w.request_redraw();
            }
            self.next_splash = now + SPLASH_FRAME;
        }
        el.set_control_flow(ControlFlow::WaitUntil(self.next_splash));
    }

    fn exiting(&mut self, _el: &ActiveEventLoop) {
        self.shared.send_input(InputMessage::ReleaseAll {});
    }
}
