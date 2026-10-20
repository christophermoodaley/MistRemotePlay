//! Windows capture: monitors are enumerated with Win32; pixels come from
//! GStreamer's `d3d11screencapturesrc` (DXGI Desktop Duplication) selected
//! by HMONITOR.

use windows::core::BOOL;
use windows::Win32::Foundation::{LPARAM, RECT};
use windows::Win32::Graphics::Gdi::{EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFOEXW};

use crate::capture::{CaptureBackend, CaptureError, CaptureKind, CaptureSourceSpec, DisplayInfo, Result};

pub struct DxgiCapture;

const MONITORINFOF_PRIMARY: u32 = 1;

unsafe extern "system" fn collect(monitor: HMONITOR, _hdc: HDC, _rect: *mut RECT, data: LPARAM) -> BOOL {
    // SAFETY: `data` is the &mut Vec passed by `monitors()` below and outlives the enumeration.
    let out = unsafe { &mut *(data.0 as *mut Vec<DisplayInfo>) };
    let mut info = MONITORINFOEXW::default();
    info.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
    // SAFETY: info is a properly sized MONITORINFOEXW.
    if unsafe { GetMonitorInfoW(monitor, &mut info.monitorInfo as *mut _ as *mut _) }.as_bool() {
        let r = info.monitorInfo.rcMonitor;
        let len = info.szDevice.iter().position(|c| *c == 0).unwrap_or(info.szDevice.len());
        let name = String::from_utf16_lossy(&info.szDevice[..len]);
        out.push(DisplayInfo {
            id: format!("hmonitor:{}", monitor.0 as usize),
            name,
            x: r.left,
            y: r.top,
            width: (r.right - r.left) as u32,
            height: (r.bottom - r.top) as u32,
            primary: info.monitorInfo.dwFlags & MONITORINFOF_PRIMARY != 0,
            backend: CaptureKind::WindowsDxgi,
        });
    }
    BOOL(1)
}

fn monitors() -> Result<Vec<DisplayInfo>> {
    let mut out: Vec<DisplayInfo> = Vec::new();
    // SAFETY: the callback only touches `out` for the duration of this call.
    let ok = unsafe { EnumDisplayMonitors(None, None, Some(collect), LPARAM(&mut out as *mut _ as isize)) };
    if !ok.as_bool() {
        return Err(CaptureError::Backend("EnumDisplayMonitors failed".into()));
    }
    Ok(out)
}

impl CaptureBackend for DxgiCapture {
    fn kind(&self) -> CaptureKind {
        CaptureKind::WindowsDxgi
    }

    fn displays(&self) -> Result<Vec<DisplayInfo>> {
        monitors()
    }

    fn open(&self, display_id: &str) -> Result<CaptureSourceSpec> {
        let displays = monitors()?;
        let d = if display_id.is_empty() || display_id == "primary" {
            displays.iter().find(|d| d.primary).or(displays.first())
        } else {
            displays.iter().find(|d| d.id == display_id)
        }
        .ok_or_else(|| CaptureError::UnknownDisplay(display_id.into()))?;
        let handle =
            d.id.strip_prefix("hmonitor:")
                .and_then(|h| h.parse::<u64>().ok())
                .ok_or_else(|| CaptureError::UnknownDisplay(display_id.into()))?;
        Ok(CaptureSourceSpec::D3D11 { monitor_handle: handle, width: d.width, height: d.height, show_cursor: true })
    }
}
