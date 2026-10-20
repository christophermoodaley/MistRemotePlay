//! Control-channel messages (reliable WebRTC data channel, both directions):
//! clock sync, statistics, permissions and keyframe requests.

use serde::{Deserialize, Serialize};

use crate::protocol::{check_printable, invalid, Result, Validate};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ControlMessage {
    /// Clock-sync probe; `client_time_us` is the client's monotonic clock.
    Ping {
        id: u32,
        client_time_us: u64,
    },
    /// `host_time_us` is the host's monotonic clock when the ping arrived.
    Pong {
        id: u32,
        client_time_us: u64,
        host_time_us: u64,
    },
    /// Host tells the client whether its input is currently accepted.
    InputPermission {
        allowed: bool,
    },
    /// Periodic host-side measurements.
    HostStats(HostStats),
    /// Periodic client-side measurements (used for adaptive bitrate).
    ClientStats(ClientStats),
    RequestKeyframe {},
    /// Host -> client: drive the rumble motors of the client's controller
    /// in slot `index` (forwarded from the game via the virtual pad).
    /// Magnitudes are 0..=65535; `duration_ms` 0 = until the next Rumble.
    Rumble {
        index: u8,
        strong: u16,
        weak: u16,
        duration_ms: u16,
    },
    /// Host is ending the session (sent before the peer connection closes).
    Goodbye {
        reason: String,
    },
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostStats {
    pub encoder: String,
    pub width: u32,
    pub height: u32,
    pub target_bitrate_kbps: u32,
    pub capture_fps: f32,
    pub encode_ms_avg: f32,
    pub frames_dropped_before_encode: u64,
    /// Screen grab: frame sampled by the capture source -> entered the
    /// pipeline (None when the source does not timestamp at sampling time).
    #[serde(default)]
    pub capture_ms_avg: Option<f32>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClientStats {
    pub rendered_fps: f32,
    pub frames_decoded: u64,
    pub frames_dropped: u64,
    pub packets_lost: i64,
    pub packets_received: u64,
    pub jitter_ms: f32,
    pub decode_ms_avg: f32,
    pub latency_ms: Option<f32>,
}

fn finite(field: &'static str, v: f32) -> Result<()> {
    if !v.is_finite() || !(0.0..=1.0e6).contains(&v) {
        return Err(invalid(field, "must be a finite non-negative number"));
    }
    Ok(())
}

impl Validate for ControlMessage {
    fn validate(&self) -> Result<()> {
        match self {
            ControlMessage::HostStats(s) => {
                check_printable("encoder", &s.encoder, 64)?;
                finite("capture_fps", s.capture_fps)?;
                finite("encode_ms_avg", s.encode_ms_avg)?;
                if let Some(c) = s.capture_ms_avg {
                    finite("capture_ms_avg", c)?;
                }
                Ok(())
            }
            ControlMessage::ClientStats(s) => {
                finite("rendered_fps", s.rendered_fps)?;
                finite("jitter_ms", s.jitter_ms)?;
                finite("decode_ms_avg", s.decode_ms_avg)?;
                if let Some(l) = s.latency_ms {
                    if !l.is_finite() {
                        return Err(invalid("latency_ms", "must be finite"));
                    }
                }
                Ok(())
            }
            ControlMessage::Goodbye { reason } => check_printable("reason", reason, 256),
            ControlMessage::Rumble { index, .. } => {
                if *index >= crate::protocol::input::MAX_GAMEPADS {
                    return Err(invalid("index", "gamepad index out of range"));
                }
                Ok(())
            }
            ControlMessage::Ping { .. }
            | ControlMessage::Pong { .. }
            | ControlMessage::InputPermission { .. }
            | ControlMessage::RequestKeyframe {} => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{decode, encode};

    #[test]
    fn roundtrip_and_reject_nan() {
        let m = ControlMessage::Ping { id: 1, client_time_us: 2 };
        assert_eq!(decode::<ControlMessage>(encode(&m).as_bytes(), 2048).unwrap(), m);
        let bad = ControlMessage::ClientStats(ClientStats { rendered_fps: f32::INFINITY, ..Default::default() });
        assert!(bad.validate().is_err());
        let r = ControlMessage::Rumble { index: 3, strong: 65535, weak: 0, duration_ms: 200 };
        assert_eq!(decode::<ControlMessage>(encode(&r).as_bytes(), 2048).unwrap(), r);
        assert!(ControlMessage::Rumble { index: 4, strong: 1, weak: 1, duration_ms: 0 }.validate().is_err());
    }
}
