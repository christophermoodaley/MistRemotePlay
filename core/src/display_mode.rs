//! Match the host's display to the player's screen.
//!
//! When the player's monitor has another shape than the host's (16:10 vs
//! 16:9, ultrawide, a laptop), the stream would be letterboxed. Switching
//! the host display to the player's resolution, or the closest mode with
//! the same aspect ratio, makes games and the desktop render for the
//! player's screen, as if it were plugged into the host.
//!
//! The change is temporary (`CDS_FULLSCREEN`: never written to the
//! registry, so signing out or restarting also undoes it) and is undone
//! when the [`DisplayModeGuard`] is dropped.

/// A display mode (pixels and refresh rate).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Mode {
    pub width: u32,
    pub height: u32,
    pub refresh_hz: u32,
}

/// Same shape within 1 % (1366x768 counts as 16:9).
fn same_aspect(a: (u32, u32), b: (u32, u32)) -> bool {
    let lhs = a.0 as f64 * b.1 as f64;
    let rhs = a.1 as f64 * b.0 as f64;
    (lhs - rhs).abs() <= 0.01 * lhs.max(rhs)
}

/// The mode to switch to for a client screen of `client` pixels, or `None`
/// to keep `current`: the client's exact size if offered, else the
/// largest same-shaped mode no bigger than the client's screen, else the
/// smallest same-shaped mode bigger than it. Keeps the current refresh rate
/// when the chosen size offers it, otherwise the highest one.
pub fn choose(modes: &[Mode], current: Mode, client: (u32, u32)) -> Option<Mode> {
    if same_aspect((current.width, current.height), client) {
        // Same shape already: scaling fills the client's screen exactly.
        return None;
    }
    let sizes: Vec<(u32, u32)> = {
        let mut v: Vec<(u32, u32)> = modes
            .iter()
            .map(|m| (m.width, m.height))
            .filter(|&s| same_aspect(s, client) && s.0 >= 640 && s.1 >= 480)
            .collect();
        v.sort_unstable();
        v.dedup();
        v
    };
    let area = |s: (u32, u32)| s.0 as u64 * s.1 as u64;
    let fits = |s: &(u32, u32)| s.0 <= client.0 && s.1 <= client.1;
    let size = sizes
        .iter()
        .copied()
        .find(|&s| s == client)
        .or_else(|| sizes.iter().copied().filter(fits).max_by_key(|&s| area(s)))
        .or_else(|| sizes.iter().copied().filter(|s| !fits(s)).min_by_key(|&s| area(s)))?;
    let rates = modes.iter().filter(|m| (m.width, m.height) == size).map(|m| m.refresh_hz);
    let refresh_hz = if modes.iter().any(|m| (m.width, m.height) == size && m.refresh_hz == current.refresh_hz) {
        current.refresh_hz
    } else {
        rates.max()?
    };
    Some(Mode { width: size.0, height: size.1, refresh_hz })
}

/// A stream size with the shape of `screen` for a quality setting of `size`:
/// the same height (wider or narrower to match the shape), never larger
/// than the screen, even numbers.
pub fn fit_shape(size: (u32, u32), screen: (u32, u32)) -> (u32, u32) {
    if screen.0 == 0 || screen.1 == 0 {
        return size;
    }
    let aspect = screen.0 as f64 / screen.1 as f64;
    let mut h = size.1.min(screen.1) as f64;
    let mut w = h * aspect;
    if w > screen.0 as f64 {
        w = screen.0 as f64;
        h = w / aspect;
    }
    let even = |v: f64, lo: u32, hi: u32| ((v.round() as u32).clamp(lo, hi)) & !1;
    (even(w, 320, 7680), even(h, 240, 4320))
}

/// Puts the display back as it was when dropped.
pub struct DisplayModeGuard {
    #[cfg(windows)]
    device: Vec<u16>,
    pub mode: Mode,
}

#[cfg(windows)]
mod imp {
    use super::{choose, DisplayModeGuard, Mode};
    use windows::core::PCWSTR;
    use windows::Win32::Graphics::Gdi::{
        ChangeDisplaySettingsExW, EnumDisplaySettingsW, CDS_FULLSCREEN, CDS_TYPE, DEVMODEW, DISP_CHANGE_SUCCESSFUL,
        DM_BITSPERPEL, DM_DISPLAYFREQUENCY, DM_PELSHEIGHT, DM_PELSWIDTH, ENUM_CURRENT_SETTINGS,
        ENUM_DISPLAY_SETTINGS_MODE,
    };

    fn devmode() -> DEVMODEW {
        DEVMODEW { dmSize: std::mem::size_of::<DEVMODEW>() as u16, ..Default::default() }
    }

    fn mode_of(d: &DEVMODEW) -> Mode {
        Mode { width: d.dmPelsWidth, height: d.dmPelsHeight, refresh_hz: d.dmDisplayFrequency }
    }

    /// The current mode, its colour depth and every mode of that depth, for
    /// a landscape display (portrait displays are left alone).
    pub fn modes(device: &[u16]) -> Option<(Mode, u32, Vec<Mode>)> {
        let dev = PCWSTR(device.as_ptr());
        let mut cur = devmode();
        // SAFETY: `cur` is a properly sized DEVMODEW; `device` is NUL-terminated.
        if !unsafe { EnumDisplaySettingsW(dev, ENUM_CURRENT_SETTINGS, &mut cur) }.as_bool() {
            tracing::debug!("cannot read the current display mode");
            return None;
        }
        // SAFETY: display devices fill the display variant of the union.
        let orientation = unsafe { cur.Anonymous1.Anonymous2.dmDisplayOrientation };
        if orientation.0 % 2 == 1 {
            return None;
        }
        let bits = cur.dmBitsPerPel;
        let mut modes = Vec::new();
        for i in 0u32.. {
            let mut m = devmode();
            // SAFETY: as above.
            if !unsafe { EnumDisplaySettingsW(dev, ENUM_DISPLAY_SETTINGS_MODE(i), &mut m) }.as_bool() {
                break;
            }
            if m.dmBitsPerPel == bits {
                modes.push(mode_of(&m));
            }
        }
        Some((mode_of(&cur), bits, modes))
    }

    /// Switch `device` (e.g. `\\.\DISPLAY1`) for a client screen of `client`.
    pub fn match_client(device: &str, client: (u32, u32)) -> Option<DisplayModeGuard> {
        let name: Vec<u16> = device.encode_utf16().chain(std::iter::once(0)).collect();
        let dev = PCWSTR(name.as_ptr());
        let (current, bits, modes) = modes(&name)?;
        let target = choose(&modes, current, client)?;
        if target == current {
            return None;
        }
        let mut want = devmode();
        want.dmPelsWidth = target.width;
        want.dmPelsHeight = target.height;
        want.dmDisplayFrequency = target.refresh_hz;
        want.dmBitsPerPel = bits;
        want.dmFields = DM_PELSWIDTH | DM_PELSHEIGHT | DM_DISPLAYFREQUENCY | DM_BITSPERPEL;
        // SAFETY: `want` is a valid DEVMODEW for this device.
        let r = unsafe { ChangeDisplaySettingsExW(dev, Some(&want), None, CDS_FULLSCREEN, None) };
        if r != DISP_CHANGE_SUCCESSFUL {
            tracing::warn!(device, ?target, code = r.0, "could not switch the display to the player's screen size");
            return None;
        }
        tracing::info!(device, from = ?current, to = ?target, "display switched to match the player's screen");
        Some(DisplayModeGuard { device: name, mode: target })
    }

    impl Drop for DisplayModeGuard {
        fn drop(&mut self) {
            // No mode = back to the registry (the user's own) settings.
            // SAFETY: `device` is NUL-terminated.
            let r = unsafe { ChangeDisplaySettingsExW(PCWSTR(self.device.as_ptr()), None, None, CDS_TYPE(0), None) };
            if r == DISP_CHANGE_SUCCESSFUL {
                tracing::info!("display mode restored");
            } else {
                tracing::warn!(code = r.0, "could not restore the display mode");
            }
        }
    }
}

#[cfg(windows)]
pub use imp::match_client;

#[cfg(all(test, windows))]
mod system {
    /// Reads this PC's display modes and shows what would be chosen for a
    /// few client screens. Read-only: nothing is switched.
    #[test]
    #[ignore]
    fn lists_modes_and_choices() {
        let displays = crate::capture::detect_backends()[0].displays().unwrap();
        for d in displays {
            let name: Vec<u16> = d.name.encode_utf16().chain(std::iter::once(0)).collect();
            let (current, bits, modes) = super::imp::modes(&name).expect("modes");
            println!("{} current {current:?} ({bits} bpp), {} modes", d.name, modes.len());
            for client in [(1920, 1080), (1920, 1200), (2560, 1080), (3440, 1440), (1366, 768), (2880, 1800)] {
                println!("  client {client:?} -> {:?}", super::choose(&modes, current, client));
            }
        }
    }
}

#[cfg(not(windows))]
pub fn match_client(_device: &str, _client: (u32, u32)) -> Option<DisplayModeGuard> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(width: u32, height: u32, refresh_hz: u32) -> Mode {
        Mode { width, height, refresh_hz }
    }

    fn modes() -> Vec<Mode> {
        vec![
            m(1280, 720, 60),
            m(1680, 1050, 60),
            m(1920, 1080, 60),
            m(1920, 1080, 144),
            m(1920, 1200, 60),
            m(2560, 1080, 60),
            m(2560, 1440, 144),
            m(2560, 1600, 60),
        ]
    }

    #[test]
    fn stream_size_takes_the_screens_shape() {
        assert_eq!(fit_shape((1920, 1080), (1920, 1080)), (1920, 1080));
        assert_eq!(fit_shape((1920, 1080), (2560, 1440)), (1920, 1080));
        assert_eq!(fit_shape((1920, 1080), (1920, 1200)), (1728, 1080));
        assert_eq!(fit_shape((1920, 1080), (3440, 1440)), (2580, 1080));
        assert_eq!(fit_shape((1920, 1080), (1366, 768)), (1366, 768));
        assert_eq!(fit_shape((1280, 720), (1280, 800)), (1152, 720));
    }

    #[test]
    fn same_shape_needs_no_switch() {
        assert_eq!(choose(&modes(), m(2560, 1440, 144), (1920, 1080)), None);
        assert_eq!(choose(&modes(), m(1920, 1080, 60), (1366, 768)), None);
    }

    #[test]
    fn exact_size_wins_and_keeps_refresh_when_possible() {
        assert_eq!(choose(&modes(), m(1920, 1080, 144), (1920, 1200)), Some(m(1920, 1200, 60)));
        assert_eq!(choose(&modes(), m(2560, 1440, 144), (2560, 1080)), Some(m(2560, 1080, 60)));
    }

    #[test]
    fn falls_back_to_largest_fitting_then_smallest_larger() {
        // 16:10 client bigger than any 16:10 mode: largest that fits.
        assert_eq!(choose(&modes(), m(1920, 1080, 60), (2880, 1800)), Some(m(2560, 1600, 60)));
        // 16:10 client smaller than every 16:10 mode: smallest bigger one.
        assert_eq!(choose(&modes(), m(1920, 1080, 60), (1440, 900)), Some(m(1680, 1050, 60)));
        // No mode of that shape: keep the display as it is.
        assert_eq!(choose(&modes(), m(1920, 1080, 60), (1024, 1024)), None);
    }
}
