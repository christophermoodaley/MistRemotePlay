//! Stream quality settings negotiated between client and host.

use serde::{Deserialize, Serialize};

use crate::protocol::{invalid, Result, Validate};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VideoCodec {
    H264,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum QualityPreset {
    LowBandwidth,
    /// Fastest encoder settings at 1080p60 with a higher bitrate cap to
    /// compensate for their lower compression efficiency.
    #[default]
    LowLatency,
    Balanced,
    HighQuality,
    Custom,
}

/// Encoder speed/quality trade-off, mapped to each encoder's own knobs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum EncoderQuality {
    Fastest,
    #[default]
    Balanced,
    Quality,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StreamSettings {
    pub preset: QualityPreset,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    /// Upper bound for the adaptive bitrate controller.
    pub max_bitrate_kbps: u32,
    /// Lower bound for the adaptive bitrate controller.
    pub min_bitrate_kbps: u32,
    pub adaptive_bitrate: bool,
    pub codec: VideoCodec,
    pub encoder_quality: EncoderQuality,
    /// Encoder element preference; `None` = automatic (hardware first).
    #[serde(default)]
    pub encoder: Option<String>,
    pub audio: AudioSettings,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AudioSettings {
    pub enabled: bool,
    pub bitrate_kbps: u32,
    pub channels: u8,
}

impl Default for AudioSettings {
    fn default() -> Self {
        AudioSettings { enabled: true, bitrate_kbps: 128, channels: 2 }
    }
}

impl StreamSettings {
    pub fn preset(preset: QualityPreset) -> Self {
        let (w, h, fps, max, min) = match preset {
            QualityPreset::LowBandwidth => (1280, 720, 30, 4_000, 1_000),
            QualityPreset::LowLatency => (1920, 1080, 60, 20_000, 3_000),
            QualityPreset::Balanced | QualityPreset::Custom => (1920, 1080, 60, 15_000, 2_000),
            QualityPreset::HighQuality => (1920, 1080, 60, 40_000, 5_000),
        };
        StreamSettings {
            preset,
            width: w,
            height: h,
            fps,
            max_bitrate_kbps: max,
            min_bitrate_kbps: min,
            adaptive_bitrate: true,
            codec: VideoCodec::H264,
            encoder_quality: match preset {
                QualityPreset::LowBandwidth | QualityPreset::LowLatency => EncoderQuality::Fastest,
                QualityPreset::HighQuality => EncoderQuality::Quality,
                _ => EncoderQuality::Balanced,
            },
            encoder: None,
            audio: AudioSettings {
                bitrate_kbps: if preset == QualityPreset::LowBandwidth { 64 } else { 128 },
                ..AudioSettings::default()
            },
        }
    }

    /// Initial bitrate before the adaptive controller has any feedback.
    pub fn start_bitrate_kbps(&self) -> u32 {
        if self.adaptive_bitrate {
            (self.max_bitrate_kbps * 2 / 3).max(self.min_bitrate_kbps)
        } else {
            self.max_bitrate_kbps
        }
    }

    /// Clamp a host's capabilities onto the request (host never upscales).
    pub fn clamp_to(&self, max_width: u32, max_height: u32, max_fps: u32) -> StreamSettings {
        let mut s = self.clone();
        if s.width > max_width || s.height > max_height {
            let scale = f64::min(max_width as f64 / s.width as f64, max_height as f64 / s.height as f64);
            // Keep dimensions even for 4:2:0 subsampling.
            s.width = ((s.width as f64 * scale) as u32) & !1;
            s.height = ((s.height as f64 * scale) as u32) & !1;
        }
        s.fps = s.fps.min(max_fps);
        s
    }
}

impl Default for StreamSettings {
    fn default() -> Self {
        Self::preset(QualityPreset::default())
    }
}

impl Validate for StreamSettings {
    fn validate(&self) -> Result<()> {
        if !(320..=7680).contains(&self.width) || !self.width.is_multiple_of(2) {
            return Err(invalid("width", "must be an even number in 320..=7680"));
        }
        if !(240..=4320).contains(&self.height) || !self.height.is_multiple_of(2) {
            return Err(invalid("height", "must be an even number in 240..=4320"));
        }
        if !(10..=240).contains(&self.fps) {
            return Err(invalid("fps", "must be in 10..=240"));
        }
        if !(250..=500_000).contains(&self.max_bitrate_kbps) {
            return Err(invalid("max_bitrate_kbps", "must be in 250..=500000"));
        }
        if self.min_bitrate_kbps < 100 || self.min_bitrate_kbps > self.max_bitrate_kbps {
            return Err(invalid("min_bitrate_kbps", "must be >= 100 and <= max_bitrate_kbps"));
        }
        if let Some(enc) = &self.encoder {
            if enc.is_empty() || enc.len() > 64 || !enc.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') {
                return Err(invalid("encoder", "must be an element name"));
            }
        }
        if !(16..=510).contains(&self.audio.bitrate_kbps) {
            return Err(invalid("audio.bitrate_kbps", "must be in 16..=510"));
        }
        if !(1..=2).contains(&self.audio.channels) {
            return Err(invalid("audio.channels", "must be 1 or 2"));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presets_are_valid() {
        for p in [
            QualityPreset::LowBandwidth,
            QualityPreset::LowLatency,
            QualityPreset::Balanced,
            QualityPreset::HighQuality,
            QualityPreset::Custom,
        ] {
            StreamSettings::preset(p).validate().unwrap();
        }
    }

    #[test]
    fn start_bitrate_respects_bounds() {
        let mut s = StreamSettings::preset(QualityPreset::Balanced);
        assert_eq!(s.start_bitrate_kbps(), 10_000);
        s.adaptive_bitrate = false;
        assert_eq!(s.start_bitrate_kbps(), s.max_bitrate_kbps);
        s.adaptive_bitrate = true;
        s.min_bitrate_kbps = s.max_bitrate_kbps;
        assert_eq!(s.start_bitrate_kbps(), s.max_bitrate_kbps);
    }

    #[test]
    fn invalid_settings_rejected() {
        let s = StreamSettings { width: 1921, ..Default::default() };
        assert!(s.validate().is_err());
        let mut s = StreamSettings::default();
        s.min_bitrate_kbps = s.max_bitrate_kbps + 1;
        assert!(s.validate().is_err());
        let s =
            StreamSettings { encoder: Some("x264enc ! filesink location=/etc/passwd".into()), ..Default::default() };
        assert!(s.validate().is_err());
    }

    #[test]
    fn clamp_keeps_aspect_and_even_dimensions() {
        let s = StreamSettings::preset(QualityPreset::HighQuality).clamp_to(1366, 768, 30);
        assert_eq!((s.width, s.height, s.fps), (1364, 768, 30));
    }
}
