//! Typed view of `webrtcbin` `get-stats` output. Only values reported by
//! GStreamer are filled in; missing values stay `None`.

use std::collections::HashMap;

use gst_webrtc::WebRTCStatsType;
use serde::Serialize;

#[derive(Clone, Debug, Default, Serialize)]
pub struct RtpStats {
    pub packets: Option<u64>,
    pub bytes: Option<u64>,
    pub packets_lost: Option<i64>,
    pub jitter_ms: Option<f64>,
    pub nack_count: Option<u64>,
    pub pli_count: Option<u64>,
    pub fir_count: Option<u64>,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct TransportStats {
    /// Media we send (host: video+audio).
    pub outbound_video: Option<RtpStats>,
    pub outbound_audio: Option<RtpStats>,
    /// Media we receive (client).
    pub inbound_video: Option<RtpStats>,
    pub inbound_audio: Option<RtpStats>,
    /// From RTCP receiver reports (host side).
    pub remote_rtt_ms: Option<f64>,
    pub remote_fraction_lost: Option<f64>,
    pub remote_packets_lost: Option<i64>,
    pub remote_jitter_ms: Option<f64>,
    /// "host", "srflx", "prflx" or "relay" for the selected pair.
    pub local_candidate_type: Option<String>,
    pub remote_candidate_type: Option<String>,
    pub local_candidate_protocol: Option<String>,
    pub relay_protocol: Option<String>,
}

impl TransportStats {
    /// True when the selected ICE pair goes through a TURN relay.
    pub fn relayed(&self) -> Option<bool> {
        match (&self.local_candidate_type, &self.remote_candidate_type) {
            (Some(l), Some(r)) => Some(l == "relay" || r == "relay"),
            (Some(l), None) => Some(l == "relay"),
            _ => None,
        }
    }
}

fn get_u64(s: &gst::StructureRef, f: &str) -> Option<u64> {
    s.get::<u64>(f).ok().or_else(|| s.get::<u32>(f).ok().map(u64::from))
}

fn get_i64(s: &gst::StructureRef, f: &str) -> Option<i64> {
    s.get::<i64>(f).ok().or_else(|| s.get::<i32>(f).ok().map(i64::from))
}

fn get_f64(s: &gst::StructureRef, f: &str) -> Option<f64> {
    s.get::<f64>(f).ok()
}

fn get_str(s: &gst::StructureRef, f: &str) -> Option<String> {
    s.get::<String>(f).ok().or_else(|| s.get::<&str>(f).ok().map(str::to_owned))
}

/// Media kind of an RTP stats entry. GStreamer 1.24's webrtcbin reports a
/// wrong `kind`/`codec-id` when streams are bundled, so the SSRC is matched
/// against the codec stats (which carry the right mime type) first.
fn kind_of(s: &gst::StructureRef, ssrc_kinds: &HashMap<u32, String>) -> Option<String> {
    s.get::<u32>("ssrc")
        .ok()
        .and_then(|ssrc| ssrc_kinds.get(&ssrc).cloned())
        .or_else(|| get_str(s, "kind"))
        .or_else(|| get_str(s, "media-type"))
}

fn rtp(s: &gst::StructureRef, inbound: bool) -> RtpStats {
    RtpStats {
        packets: get_u64(s, if inbound { "packets-received" } else { "packets-sent" }),
        bytes: get_u64(s, if inbound { "bytes-received" } else { "bytes-sent" }),
        packets_lost: get_i64(s, "packets-lost"),
        jitter_ms: get_f64(s, "jitter").map(|j| j * 1000.0),
        nack_count: get_u64(s, "nack-count"),
        pli_count: get_u64(s, "pli-count"),
        fir_count: get_u64(s, "fir-count"),
    }
}

/// webrtcbin also lists RTCP-only SSRCs with zero packets; keep the entry
/// that actually carries media.
fn keep_busiest(slot: &mut Option<RtpStats>, new: RtpStats) {
    match slot {
        Some(cur) if cur.packets.unwrap_or(0) >= new.packets.unwrap_or(0) => {}
        _ => *slot = Some(new),
    }
}

pub fn parse_stats(root: &gst::StructureRef) -> TransportStats {
    let mut out = TransportStats::default();
    let mut pairs: Vec<(String, String, String)> = Vec::new();
    let mut selected_pair_id: Option<String> = None;
    let mut candidates: Vec<(String, gst::Structure)> = Vec::new();
    let mut ssrc_kinds = HashMap::new();
    for (_, value) in root.iter() {
        let Ok(s) = value.get::<gst::Structure>() else { continue };
        if s.get::<WebRTCStatsType>("type") == Ok(WebRTCStatsType::Codec) {
            if let (Ok(ssrc), Some(mime)) = (s.get::<u32>("ssrc"), get_str(&s, "mime-type")) {
                let kind = mime.split('/').next().unwrap_or_default().to_ascii_lowercase();
                ssrc_kinds.insert(ssrc, kind);
            }
        }
    }
    for (_, value) in root.iter() {
        let Ok(s) = value.get::<gst::Structure>() else { continue };
        let Ok(ty) = s.get::<WebRTCStatsType>("type") else { continue };
        let id = get_str(&s, "id").unwrap_or_default();
        match ty {
            WebRTCStatsType::InboundRtp => {
                let r = rtp(&s, true);
                match kind_of(&s, &ssrc_kinds).as_deref() {
                    Some("audio") => keep_busiest(&mut out.inbound_audio, r),
                    _ => keep_busiest(&mut out.inbound_video, r),
                }
            }
            WebRTCStatsType::OutboundRtp => {
                let r = rtp(&s, false);
                match kind_of(&s, &ssrc_kinds).as_deref() {
                    Some("audio") => keep_busiest(&mut out.outbound_audio, r),
                    _ => keep_busiest(&mut out.outbound_video, r),
                }
            }
            WebRTCStatsType::RemoteInboundRtp => {
                // Prefer video; RTT is per transport anyway with bundling.
                if out.remote_rtt_ms.is_none() || kind_of(&s, &ssrc_kinds).as_deref() != Some("audio") {
                    out.remote_rtt_ms = get_f64(&s, "round-trip-time").map(|r| r * 1000.0).or(out.remote_rtt_ms);
                    out.remote_fraction_lost = get_f64(&s, "fraction-lost").or(out.remote_fraction_lost);
                    out.remote_packets_lost = get_i64(&s, "packets-lost").or(out.remote_packets_lost);
                    out.remote_jitter_ms = get_f64(&s, "jitter").map(|j| j * 1000.0).or(out.remote_jitter_ms);
                }
            }
            WebRTCStatsType::CandidatePair => {
                if let (Some(l), Some(r)) = (get_str(&s, "local-candidate-id"), get_str(&s, "remote-candidate-id")) {
                    pairs.push((id, l, r));
                }
            }
            WebRTCStatsType::Transport => {
                selected_pair_id = get_str(&s, "selected-candidate-pair-id");
            }
            WebRTCStatsType::LocalCandidate | WebRTCStatsType::RemoteCandidate => {
                candidates.push((id, s.clone()));
            }
            _ => {}
        }
    }
    let selected = match &selected_pair_id {
        Some(sel) => pairs.iter().find(|(id, _, _)| id == sel),
        None => pairs.first(),
    };
    if let Some((_, l, r)) = selected {
        for (id, s) in &candidates {
            if id == l {
                out.local_candidate_type = get_str(s, "candidate-type");
                out.local_candidate_protocol = get_str(s, "protocol");
                out.relay_protocol = get_str(s, "relay-protocol");
            } else if id == r {
                out.remote_candidate_type = get_str(s, "candidate-type");
            }
        }
    }
    out
}

/// Render the raw stats for diagnostics logs.
pub fn describe_raw(root: &gst::StructureRef) -> String {
    root.to_string()
}
