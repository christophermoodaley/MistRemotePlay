//! System capability report (`mist diagnose`, Settings -> Diagnostics).
//! Every entry is detected at runtime.

use crate::audio::AudioDeviceInfo;
use crate::capture::{CaptureKind, DisplayInfo};
use crate::media::CodecInfo;
use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct CaptureReport {
    pub backend: CaptureKind,
    pub displays: Vec<DisplayInfo>,
    pub error: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct SystemReport {
    pub app_version: String,
    pub os: String,
    pub arch: String,
    pub session_type: Option<String>,
    pub gstreamer: String,
    pub encoders: Vec<CodecInfo>,
    pub decoders: Vec<CodecInfo>,
    pub capture: Vec<CaptureReport>,
    pub audio_devices: Result<Vec<AudioDeviceInfo>, String>,
    pub keyboard_mouse_injection: Vec<(String, Result<(), String>)>,
    pub virtual_controllers: Result<(), String>,
    pub device_id: String,
    pub config_dir: String,
}

// Each platform's section is compiled out on the others.
#[allow(clippy::vec_init_then_push)]
pub fn system_report(ctx: &crate::AppContext) -> SystemReport {
    use crate::input::{create_backend, InputBackendKind, TargetRect};
    let target = TargetRect { x: 0, y: 0, width: 1, height: 1 };
    let mut injection = Vec::new();
    #[cfg(target_os = "linux")]
    {
        // Only probe XTest here; creating uinput devices has side effects,
        // so report /dev/uinput access instead.
        if std::env::var_os("DISPLAY").is_some() {
            injection.push((
                "x11_xtest".to_string(),
                create_backend(InputBackendKind::X11Xtest, target).map(|_| ()).map_err(|e| e.to_string()),
            ));
        }
        let uinput = std::fs::OpenOptions::new()
            .write(true)
            .open("/dev/uinput")
            .map(|_| ())
            .map_err(|e| format!("/dev/uinput: {e} (see the README, Linux and Raspberry Pi)"));
        injection.push(("uinput".to_string(), uinput));
    }
    #[cfg(windows)]
    injection.push((
        "win32_sendinput".to_string(),
        create_backend(InputBackendKind::Win32SendInput, target).map(|_| ()).map_err(|e| e.to_string()),
    ));
    let _ = (&target, InputBackendKind::Auto);

    #[cfg(target_os = "linux")]
    let controllers = crate::controller::uinput::UinputGamepads::probe().map_err(|e| e.to_string());
    #[cfg(windows)]
    let controllers = crate::controller::vigem::VigemGamepads::connect().map(|_| ()).map_err(|e| e.to_string());

    let mut capture: Vec<CaptureReport> = crate::host::available_capture_backends()
        .into_iter()
        .map(|(backend, displays, error)| CaptureReport { backend, displays, error })
        .collect();
    if capture.is_empty() {
        capture.push(CaptureReport {
            backend: CaptureKind::TestPattern,
            displays: Vec::new(),
            error: Some(if cfg!(windows) {
                "no screen capture available".into()
            } else {
                "no real capture backend detected (no DISPLAY / WAYLAND_DISPLAY?)".into()
            }),
        });
    }

    SystemReport {
        app_version: env!("CARGO_PKG_VERSION").into(),
        os: std::env::consts::OS.into(),
        arch: std::env::consts::ARCH.into(),
        session_type: std::env::var("XDG_SESSION_TYPE").ok(),
        gstreamer: crate::media::gstreamer_version(),
        encoders: crate::media::probe_encoders().to_vec(),
        decoders: crate::media::probe_decoders().to_vec(),
        capture,
        audio_devices: crate::media::audio_devices::default_backend().devices().map_err(|e| e.to_string()),
        keyboard_mouse_injection: injection,
        virtual_controllers: controllers,
        device_id: ctx.identity.device_id().to_string(),
        config_dir: ctx.dir.display().to_string(),
    }
}
