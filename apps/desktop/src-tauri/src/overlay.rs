//! Host-only notice that someone is connected: a small click-through,
//! always-on-top window at the top of the primary screen. It is excluded
//! from screen capture (`content_protected`, WDA_EXCLUDEFROMCAPTURE on
//! Windows), so it never shows up in the stream. It renders the same web UI,
//! which shows the overlay view in the window labelled [`LABEL`].

use mist_app_core::host::HostSnapshot;
use mist_session_manager::{SessionRole, SessionState};
use parking_lot::Mutex;
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State, WebviewUrl, WebviewWindowBuilder};

pub const LABEL: &str = "overlay";
const WIDTH: f64 = 480.0;
const HEIGHT: f64 = 76.0;
const TOP_MARGIN: f64 = 12.0;

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Viewer {
    pub name: String,
    pub role: SessionRole,
}

/// What the overlay currently shows; the overlay page reads it on load.
#[derive(Default)]
pub struct OverlayState(Mutex<Vec<Viewer>>);

/// People connected right now (a dropped link that is reconnecting counts).
pub fn viewers(snapshot: &HostSnapshot) -> Vec<Viewer> {
    snapshot
        .sessions
        .iter()
        .filter(|s| matches!(s.state, SessionState::Connected | SessionState::Reconnecting))
        .map(|s| Viewer { name: s.client_name.clone(), role: s.role })
        .collect()
}

/// Show, update or close the overlay. Creates a window, so call it off the
/// main thread (async commands and tasks are fine).
pub fn sync(app: &AppHandle, viewers: Vec<Viewer>) {
    {
        let state = app.state::<OverlayState>();
        let mut current = state.0.lock();
        if *current == viewers {
            return;
        }
        *current = viewers.clone();
    }
    let window = app.get_webview_window(LABEL);
    if viewers.is_empty() {
        if let Some(w) = window {
            let _ = w.close();
        }
        return;
    }
    if window.is_some() {
        let _ = app.emit_to(LABEL, "overlay-viewers", &viewers);
        return;
    }
    if let Err(e) = open(app) {
        tracing::warn!("cannot show the connection overlay: {e}");
    }
}

fn open(app: &AppHandle) -> tauri::Result<()> {
    let mut builder = WebviewWindowBuilder::new(app, LABEL, WebviewUrl::default())
        .title("Mist — someone is connected")
        .inner_size(WIDTH, HEIGHT)
        .decorations(false)
        .transparent(true)
        .shadow(false)
        .resizable(false)
        .always_on_top(true)
        .skip_taskbar(true)
        .focused(false)
        .focusable(false)
        .content_protected(true);
    if let Some(m) = app.primary_monitor()? {
        let scale = m.scale_factor();
        let (pos, size) = (m.position().to_logical::<f64>(scale), m.size().to_logical::<f64>(scale));
        builder = builder.position(pos.x + (size.width - WIDTH) / 2.0, pos.y + TOP_MARGIN);
    }
    let window = builder.build()?;
    window.set_ignore_cursor_events(true)?;
    Ok(())
}

#[tauri::command]
pub fn overlay_viewers(state: State<'_, OverlayState>) -> Vec<Viewer> {
    state.0.lock().clone()
}
