//! `webrtcbin` wrapper: ICE configuration, SDP negotiation, data channels
//! and statistics. All GStreamer callbacks are turned into
//! [`TransportEvent`]s on an unbounded channel.

use std::collections::HashMap;
use std::sync::{Arc, Weak};

use gst::glib;
use gst::prelude::*;
use mist_protocol::peer::SdpKind;
use mist_protocol::MAX_DATA_CHANNEL_MESSAGE_BYTES;
use parking_lot::Mutex;
use tokio::sync::oneshot;

use crate::stats::{parse_stats, TransportStats};
use crate::transport::{ConnectionState, DataChannelKind, EventSender, IceConfig, TransportEvent};
use crate::{MediaError, Result};

pub struct WebRtcPeer {
    webrtcbin: gst::Element,
    channels: Mutex<HashMap<DataChannelKind, gst_webrtc::WebRTCDataChannel>>,
    events: EventSender,
}

fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// "stun:host:port" -> "stun://host:port"
pub(crate) fn stun_uri(url: &str) -> Option<String> {
    let rest = url.strip_prefix("stun:")?;
    let rest = rest.split('?').next().unwrap_or(rest);
    Some(format!("stun://{}", rest.trim_start_matches("//")))
}

/// "turn:host:port?transport=tcp" + credentials -> webrtcbin TURN URI.
pub(crate) fn turn_uri(url: &str, username: Option<&str>, credential: Option<&str>) -> Option<String> {
    let (scheme, rest) = if let Some(r) = url.strip_prefix("turns:") {
        ("turns", r)
    } else {
        let r = url.strip_prefix("turn:")?;
        ("turn", r)
    };
    let rest = rest.trim_start_matches("//");
    let auth = match (username, credential) {
        (Some(u), Some(c)) => format!("{}:{}@", percent_encode(u), percent_encode(c)),
        _ => String::new(),
    };
    Some(format!("{scheme}://{auth}{rest}"))
}

fn map_ice_state(s: gst_webrtc::WebRTCICEConnectionState) -> ConnectionState {
    use gst_webrtc::WebRTCICEConnectionState as S;
    match s {
        S::New => ConnectionState::New,
        S::Checking => ConnectionState::Connecting,
        S::Connected | S::Completed => ConnectionState::Connected,
        S::Disconnected => ConnectionState::Disconnected,
        S::Failed => ConnectionState::Failed,
        S::Closed => ConnectionState::Closed,
        _ => ConnectionState::New,
    }
}

fn map_peer_state(s: gst_webrtc::WebRTCPeerConnectionState) -> ConnectionState {
    use gst_webrtc::WebRTCPeerConnectionState as S;
    match s {
        S::New => ConnectionState::New,
        S::Connecting => ConnectionState::Connecting,
        S::Connected => ConnectionState::Connected,
        S::Disconnected => ConnectionState::Disconnected,
        S::Failed => ConnectionState::Failed,
        S::Closed => ConnectionState::Closed,
        _ => ConnectionState::New,
    }
}

impl WebRtcPeer {
    pub fn new(name: &str, ice: &IceConfig, jitter_latency_ms: Option<u32>, events: EventSender) -> Result<Arc<Self>> {
        let webrtcbin = gst::ElementFactory::make("webrtcbin")
            .name(name)
            .build()
            .map_err(|_| MediaError::MissingElement("webrtcbin (gst-plugins-bad + libnice)".into()))?;
        webrtcbin.set_property_from_str("bundle-policy", "max-bundle");
        if let Some(ms) = jitter_latency_ms {
            webrtcbin.set_property("latency", ms);
            // Receiver: request retransmission (NACK) of lost packets.
            // webrtcbin enables this only when it can match the stream's
            // SSRC to a transceiver at the moment the jitter buffer is
            // created, which fails with bundled streams here (measured: 0
            // NACKs at 5 % loss, every lost packet waited out the jitter
            // buffer). The sender already negotiates RTX.
            if let Some(rtpbin) = webrtcbin.dynamic_cast_ref::<gst::Bin>().and_then(|b| b.by_name("rtpbin")) {
                rtpbin.connect("new-jitterbuffer", false, |values| {
                    if let Ok(jb) = values[1].get::<gst::Element>() {
                        jb.set_property("do-retransmission", true);
                        jb.set_property("do-lost", true);
                        // NACK as soon as a gap is seen instead of waiting an
                        // extra (jitter-based) delay first, and only for real
                        // gaps: guessing when the next packet is due misfires
                        // between video frames (measured: ~260 needless
                        // NACKs per 15 s without any loss).
                        jb.set_property("rtx-delay", 0i32);
                        jb.set_property("rtx-next-seqnum", false);
                    }
                    None
                });
            } else {
                tracing::warn!("webrtcbin has no rtpbin child; relying on its own NACK setup");
            }
        }
        if ice.relay_only {
            webrtcbin.set_property_from_str("ice-transport-policy", "relay");
        }
        let mut stun_set = false;
        for server in &ice.servers {
            for url in &server.urls {
                if url.starts_with("stun:") {
                    if !stun_set {
                        if let Some(u) = stun_uri(url) {
                            webrtcbin.set_property("stun-server", &u);
                            stun_set = true;
                        }
                    }
                } else if let Some(u) = turn_uri(url, server.username.as_deref(), server.credential.as_deref()) {
                    let ok = webrtcbin.emit_by_name::<bool>("add-turn-server", &[&u]);
                    if !ok {
                        tracing::warn!("webrtcbin rejected TURN server {url}");
                    }
                }
            }
        }

        let peer = Arc::new(WebRtcPeer { webrtcbin: webrtcbin.clone(), channels: Mutex::new(HashMap::new()), events });

        let ev = peer.events.clone();
        webrtcbin.connect("on-ice-candidate", false, move |values| {
            let mline_index = values[1].get::<u32>().unwrap_or(0);
            if let Ok(candidate) = values[2].get::<String>() {
                let _ = ev.send(TransportEvent::LocalCandidate { mline_index, candidate });
            }
            None
        });

        let ev = peer.events.clone();
        webrtcbin.connect_notify(Some("ice-connection-state"), move |el, _| {
            let s = el.property::<gst_webrtc::WebRTCICEConnectionState>("ice-connection-state");
            tracing::debug!("ICE connection state: {s:?}");
            let _ = ev.send(TransportEvent::IceState(map_ice_state(s)));
        });

        let ev = peer.events.clone();
        webrtcbin.connect_notify(Some("connection-state"), move |el, _| {
            let s = el.property::<gst_webrtc::WebRTCPeerConnectionState>("connection-state");
            tracing::debug!("peer connection state: {s:?}");
            let _ = ev.send(TransportEvent::PeerState(map_peer_state(s)));
        });

        let ev = peer.events.clone();
        webrtcbin.connect_notify(Some("ice-gathering-state"), move |el, _| {
            let s = el.property::<gst_webrtc::WebRTCICEGatheringState>("ice-gathering-state");
            if s == gst_webrtc::WebRTCICEGatheringState::Complete {
                let _ = ev.send(TransportEvent::IceGatheringComplete);
            }
        });

        let weak = Arc::downgrade(&peer);
        webrtcbin.connect("on-data-channel", false, move |values| {
            if let (Some(peer), Ok(ch)) = (weak.upgrade(), values[1].get::<gst_webrtc::WebRTCDataChannel>()) {
                peer.adopt_channel(ch);
            }
            None
        });

        Ok(peer)
    }

    pub fn element(&self) -> &gst::Element {
        &self.webrtcbin
    }

    fn adopt_channel(self: &Arc<Self>, ch: gst_webrtc::WebRTCDataChannel) {
        let label = ch.label().map(|l| l.to_string()).unwrap_or_default();
        let Some(kind) = DataChannelKind::from_label(&label) else {
            tracing::warn!("ignoring unexpected data channel `{label}`");
            ch.close();
            return;
        };
        self.wire_channel(kind, &ch);
        let open = ch.ready_state() == gst_webrtc::WebRTCDataChannelState::Open;
        self.channels.lock().insert(kind, ch);
        if open {
            let _ = self.events.send(TransportEvent::ChannelOpen(kind));
        }
    }

    fn wire_channel(&self, kind: DataChannelKind, ch: &gst_webrtc::WebRTCDataChannel) {
        let ev = self.events.clone();
        ch.connect("on-open", false, move |_| {
            let _ = ev.send(TransportEvent::ChannelOpen(kind));
            None
        });
        let ev = self.events.clone();
        ch.connect("on-close", false, move |_| {
            let _ = ev.send(TransportEvent::ChannelClosed(kind));
            None
        });
        let ev = self.events.clone();
        ch.connect("on-error", false, move |values| {
            let msg =
                values.get(1).and_then(|v| v.get::<glib::Error>().ok()).map(|e| e.to_string()).unwrap_or_default();
            let _ = ev.send(TransportEvent::Warning(format!("data channel {} error: {msg}", kind.label())));
            None
        });
        let ev = self.events.clone();
        ch.connect("on-message-string", false, move |values| {
            if let Ok(Some(text)) = values[1].get::<Option<String>>() {
                if text.len() <= MAX_DATA_CHANNEL_MESSAGE_BYTES {
                    let _ = ev.send(TransportEvent::ChannelMessage(kind, text));
                } else {
                    tracing::warn!(channel = kind.label(), len = text.len(), "dropping oversized data channel message");
                }
            }
            None
        });
        ch.connect("on-message-data", false, move |_| {
            tracing::warn!(channel = kind.label(), "dropping unexpected binary data channel message");
            None
        });
    }

    /// Host side: create the three channels before the offer.
    pub fn create_data_channels(self: &Arc<Self>) -> Result<()> {
        for kind in DataChannelKind::ALL {
            let config = match kind {
                DataChannelKind::Gamepad => {
                    gst::Structure::builder("config").field("ordered", false).field("max-retransmits", 0i32).build()
                }
                _ => gst::Structure::builder("config").field("ordered", true).build(),
            };
            let ch = self
                .webrtcbin
                .emit_by_name::<Option<gst_webrtc::WebRTCDataChannel>>("create-data-channel", &[&kind.label(), &config])
                .ok_or_else(|| MediaError::Negotiation(format!("could not create data channel {}", kind.label())))?;
            self.wire_channel(kind, &ch);
            self.channels.lock().insert(kind, ch);
        }
        Ok(())
    }

    fn set_local_and_emit(weak: Weak<Self>, desc: gst_webrtc::WebRTCSessionDescription, kind: SdpKind) {
        let Some(peer) = weak.upgrade() else { return };
        peer.webrtcbin.emit_by_name::<()>("set-local-description", &[&desc, &None::<gst::Promise>]);
        match desc.sdp().as_text() {
            Ok(sdp) => {
                let _ = peer.events.send(TransportEvent::LocalSdp { kind, sdp: sdp.to_string() });
            }
            Err(e) => {
                let _ = peer.events.send(TransportEvent::Error(format!("cannot serialize local SDP: {e}")));
            }
        }
    }

    pub fn create_offer(self: &Arc<Self>) {
        let weak = Arc::downgrade(self);
        let ev = self.events.clone();
        let promise = gst::Promise::with_change_func(move |reply| match reply {
            Ok(Some(s)) => match s.get::<gst_webrtc::WebRTCSessionDescription>("offer") {
                Ok(offer) => Self::set_local_and_emit(weak, offer, SdpKind::Offer),
                Err(_) => {
                    let _ = ev.send(TransportEvent::Error(format!("create-offer failed: {s}")));
                }
            },
            other => {
                let _ = ev.send(TransportEvent::Error(format!("create-offer failed: {other:?}")));
            }
        });
        self.webrtcbin.emit_by_name::<()>("create-offer", &[&None::<gst::Structure>, &promise]);
    }

    fn create_answer(self: &Arc<Self>) {
        let weak = Arc::downgrade(self);
        let ev = self.events.clone();
        let promise = gst::Promise::with_change_func(move |reply| match reply {
            Ok(Some(s)) => match s.get::<gst_webrtc::WebRTCSessionDescription>("answer") {
                Ok(answer) => Self::set_local_and_emit(weak, answer, SdpKind::Answer),
                Err(_) => {
                    let _ = ev.send(TransportEvent::Error(format!("create-answer failed: {s}")));
                }
            },
            other => {
                let _ = ev.send(TransportEvent::Error(format!("create-answer failed: {other:?}")));
            }
        });
        self.webrtcbin.emit_by_name::<()>("create-answer", &[&None::<gst::Structure>, &promise]);
    }

    /// Apply a remote description; an offer is answered automatically.
    pub fn set_remote_sdp(self: &Arc<Self>, kind: SdpKind, sdp: &str) -> Result<()> {
        let msg = gst_sdp::SDPMessage::parse_buffer(sdp.as_bytes())
            .map_err(|_| MediaError::Negotiation("remote SDP does not parse".into()))?;
        let ty = match kind {
            SdpKind::Offer => gst_webrtc::WebRTCSDPType::Offer,
            SdpKind::Answer => gst_webrtc::WebRTCSDPType::Answer,
        };
        let desc = gst_webrtc::WebRTCSessionDescription::new(ty, msg);
        let weak = Arc::downgrade(self);
        let ev = self.events.clone();
        let promise = gst::Promise::with_change_func(move |reply| {
            if let Ok(Some(s)) = &reply {
                if s.has_field("error") {
                    let _ = ev.send(TransportEvent::Error(format!("set-remote-description failed: {s}")));
                    return;
                }
            }
            if kind == SdpKind::Offer {
                if let Some(peer) = weak.upgrade() {
                    peer.create_answer();
                }
            }
        });
        self.webrtcbin.emit_by_name::<()>("set-remote-description", &[&desc, &promise]);
        Ok(())
    }

    pub fn add_remote_candidate(&self, mline_index: u32, candidate: &str) {
        if candidate.is_empty() {
            return; // end-of-candidates marker
        }
        self.webrtcbin.emit_by_name::<()>("add-ice-candidate", &[&mline_index, &candidate]);
    }

    pub fn send(&self, kind: DataChannelKind, text: &str) -> Result<()> {
        let channels = self.channels.lock();
        let ch = channels
            .get(&kind)
            .ok_or_else(|| MediaError::Channel(format!("{} channel not available", kind.label())))?;
        if ch.ready_state() != gst_webrtc::WebRTCDataChannelState::Open {
            return Err(MediaError::Channel(format!("{} channel not open", kind.label())));
        }
        ch.send_string_full(Some(text)).map_err(|e| MediaError::Channel(e.to_string()))
    }

    pub fn request_stats(&self) -> oneshot::Receiver<TransportStats> {
        let (tx, rx) = oneshot::channel();
        let tx = Mutex::new(Some(tx));
        let promise = gst::Promise::with_change_func(move |reply| {
            if let (Ok(Some(s)), Some(tx)) = (reply, tx.lock().take()) {
                tracing::trace!(raw = %s, "webrtc stats");
                let _ = tx.send(parse_stats(s));
            }
        });
        self.webrtcbin.emit_by_name::<()>("get-stats", &[&None::<gst::Pad>, &promise]);
        rx
    }

    pub fn close_channels(&self) {
        for ch in self.channels.lock().values() {
            ch.close();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ice_uri_conversion() {
        assert_eq!(stun_uri("stun:stun.example.org:3478").unwrap(), "stun://stun.example.org:3478");
        assert_eq!(
            turn_uri("turn:relay.example.org:3478?transport=tcp", Some("1700000000:abc"), Some("p/w+=")).unwrap(),
            "turn://1700000000%3Aabc:p%2Fw%2B%3D@relay.example.org:3478?transport=tcp"
        );
        assert_eq!(turn_uri("turns:relay.example.org:5349", None, None).unwrap(), "turns://relay.example.org:5349");
        assert!(turn_uri("stun:x", None, None).is_none());
    }
}
