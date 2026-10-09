//! System-audio (loopback) device discovery via GStreamer device providers.

use gst::prelude::*;
use mist_audio_api::{AudioCaptureBackend, AudioDeviceInfo, AudioError, AudioKind, AudioSourceSpec};

/// PulseAudio / PipeWire-pulse monitor sources.
pub struct PulseLoopback;
/// WASAPI loopback of render endpoints.
pub struct WasapiLoopback;

fn monitor_devices(class: &str) -> Result<Vec<gst::Device>, AudioError> {
    crate::init().map_err(|e| AudioError::Unavailable(e.to_string()))?;
    let monitor = gst::DeviceMonitor::new();
    monitor.add_filter(Some(class), None);
    monitor.start().map_err(|e| AudioError::Unavailable(format!("device monitor: {e}")))?;
    let devices = monitor.devices().into_iter().collect();
    monitor.stop();
    Ok(devices)
}

fn prop_str(d: &gst::Device, key: &str) -> Option<String> {
    d.properties().and_then(|p| p.get::<String>(key).ok())
}

impl AudioCaptureBackend for PulseLoopback {
    fn kind(&self) -> AudioKind {
        AudioKind::PulseMonitor
    }

    fn devices(&self) -> Result<Vec<AudioDeviceInfo>, AudioError> {
        if gst::ElementFactory::find("pulsesrc").is_none() {
            return Err(AudioError::Unavailable("pulsesrc not installed (gstreamer1.0-pulseaudio)".into()));
        }
        let mut out = vec![AudioDeviceInfo {
            id: None,
            name: "Default output (system audio)".into(),
            kind: AudioKind::PulseMonitor,
            is_default: true,
        }];
        for d in monitor_devices("Audio/Source")? {
            if prop_str(&d, "device.class").as_deref() != Some("monitor") {
                continue; // skip microphones
            }
            let id = prop_str(&d, "device.name").or_else(|| prop_str(&d, "node.name"));
            out.push(AudioDeviceInfo {
                id,
                name: d.display_name().to_string(),
                kind: AudioKind::PulseMonitor,
                is_default: false,
            });
        }
        Ok(out)
    }

    fn source(&self, device: Option<&str>) -> Result<AudioSourceSpec, AudioError> {
        Ok(AudioSourceSpec::PulseMonitor { device: device.map(str::to_owned) })
    }
}

impl AudioCaptureBackend for WasapiLoopback {
    fn kind(&self) -> AudioKind {
        AudioKind::WasapiLoopback
    }

    fn devices(&self) -> Result<Vec<AudioDeviceInfo>, AudioError> {
        if gst::ElementFactory::find("wasapi2src").is_none() {
            return Err(AudioError::Unavailable("wasapi2src not installed".into()));
        }
        let mut out = vec![AudioDeviceInfo {
            id: None,
            name: "Default output (system audio)".into(),
            kind: AudioKind::WasapiLoopback,
            is_default: true,
        }];
        for d in monitor_devices("Audio/Sink")? {
            let id = prop_str(&d, "device.id");
            out.push(AudioDeviceInfo {
                id,
                name: d.display_name().to_string(),
                kind: AudioKind::WasapiLoopback,
                is_default: false,
            });
        }
        Ok(out)
    }

    fn source(&self, device: Option<&str>) -> Result<AudioSourceSpec, AudioError> {
        Ok(AudioSourceSpec::WasapiLoopback { device: device.map(str::to_owned) })
    }
}

/// The platform's system-audio backend.
pub fn default_backend() -> Box<dyn AudioCaptureBackend> {
    if cfg!(windows) {
        Box::new(WasapiLoopback)
    } else {
        Box::new(PulseLoopback)
    }
}
