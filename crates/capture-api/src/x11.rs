//! X11 capture: monitors come from RandR 1.5, pixels from `ximagesrc`
//! (XShm/XGetImage) in the media engine.

use x11rb::connection::Connection;
use x11rb::protocol::randr::ConnectionExt as _;
use x11rb::protocol::xproto::ConnectionExt as _;

use crate::{CaptureBackend, CaptureError, CaptureKind, CaptureSourceSpec, DisplayInfo, Result};

pub struct X11Capture {
    display_name: Option<String>,
}

impl X11Capture {
    /// Verifies the X server is reachable.
    pub fn connect(display_name: Option<String>) -> Result<Self> {
        x11rb::connect(display_name.as_deref()).map_err(|e| CaptureError::Unavailable(format!("cannot open X display: {e}")))?;
        Ok(X11Capture { display_name })
    }

    fn query(&self) -> std::result::Result<Vec<DisplayInfo>, Box<dyn std::error::Error>> {
        let (conn, screen_num) = x11rb::connect(self.display_name.as_deref())?;
        let screen = &conn.setup().roots[screen_num];
        let mut out = Vec::new();
        // RandR 1.5 monitors; fall back to the whole root window.
        let monitors = conn.randr_get_monitors(screen.root, true).ok().and_then(|c| c.reply().ok());
        if let Some(reply) = monitors {
            for m in reply.monitors {
                let name = conn
                    .get_atom_name(m.name)
                    .ok()
                    .and_then(|c| c.reply().ok())
                    .map(|r| String::from_utf8_lossy(&r.name).into_owned())
                    .unwrap_or_else(|| format!("monitor-{}", out.len()));
                out.push(DisplayInfo {
                    id: format!("x11:{name}"),
                    name,
                    x: m.x as i32,
                    y: m.y as i32,
                    width: m.width as u32,
                    height: m.height as u32,
                    primary: m.primary,
                    backend: CaptureKind::X11,
                });
            }
        }
        if out.is_empty() {
            out.push(DisplayInfo {
                id: "x11:screen".into(),
                name: format!("Screen {screen_num}"),
                x: 0,
                y: 0,
                width: screen.width_in_pixels as u32,
                height: screen.height_in_pixels as u32,
                primary: true,
                backend: CaptureKind::X11,
            });
        }
        Ok(out)
    }
}

impl CaptureBackend for X11Capture {
    fn kind(&self) -> CaptureKind {
        CaptureKind::X11
    }

    fn displays(&self) -> Result<Vec<DisplayInfo>> {
        self.query().map_err(|e| CaptureError::Backend(e.to_string()))
    }

    fn open(&self, display_id: &str) -> Result<CaptureSourceSpec> {
        let displays = self.displays()?;
        let d = if display_id.is_empty() || display_id == "primary" {
            displays.iter().find(|d| d.primary).or(displays.first())
        } else {
            displays.iter().find(|d| d.id == display_id)
        }
        .ok_or_else(|| CaptureError::UnknownDisplay(display_id.into()))?;
        Ok(CaptureSourceSpec::X11 {
            display_name: self.display_name.clone().or_else(|| std::env::var("DISPLAY").ok()),
            x: d.x,
            y: d.y,
            width: d.width,
            height: d.height,
            show_cursor: true,
        })
    }
}
