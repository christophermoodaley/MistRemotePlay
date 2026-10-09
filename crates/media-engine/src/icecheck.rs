//! ICE self-test: gathers candidates through the configured STUN/TURN
//! servers. A relay candidate proves that the TURN server accepted our
//! credentials and allocated a relay address — i.e. it *works*, rather than
//! merely being configured.

use std::time::{Duration, Instant};

use gst::prelude::*;
use serde::Serialize;
use tokio::sync::mpsc;

use crate::transport::{IceConfig, TransportEvent};
use crate::webrtc::WebRtcPeer;
use crate::Result;

#[derive(Clone, Debug, Default, Serialize)]
pub struct IceCheckResult {
    pub host_candidates: usize,
    pub srflx_candidates: usize,
    pub relay_candidates: usize,
    /// Relay candidates by transport ("udp"/"tcp").
    pub relay_transports: Vec<String>,
    pub gathering_complete: bool,
    pub elapsed_ms: u64,
    pub candidates: Vec<String>,
}

pub fn check_ice(ice: &IceConfig, timeout: Duration) -> Result<IceCheckResult> {
    crate::init()?;
    let (tx, mut rx) = mpsc::unbounded_channel();
    let pipeline = gst::Pipeline::with_name("mist-ice-check");
    let peer = WebRtcPeer::new("ice-check", ice, None, tx)?;
    pipeline.add(peer.element()).map_err(crate::MediaError::from_bool)?;
    pipeline
        .set_state(gst::State::Playing)
        .map_err(|_| crate::MediaError::Pipeline("ICE check pipeline failed to start".into()))?;
    // A data channel gives the offer an m-line so ICE gathering starts.
    let ch = peer.element().emit_by_name::<Option<gst_webrtc::WebRTCDataChannel>>(
        "create-data-channel",
        &[&"probe", &None::<gst::Structure>],
    );
    peer.create_offer();
    let start = Instant::now();
    let mut out = IceCheckResult::default();
    while start.elapsed() < timeout {
        match rx.try_recv() {
            Ok(TransportEvent::LocalCandidate { candidate, .. }) => {
                let typ = candidate.split_whitespace().skip_while(|t| *t != "typ").nth(1).unwrap_or("");
                match typ {
                    "host" => out.host_candidates += 1,
                    "srflx" | "prflx" => out.srflx_candidates += 1,
                    "relay" => {
                        out.relay_candidates += 1;
                        let proto = candidate.split_whitespace().nth(2).unwrap_or("").to_ascii_lowercase();
                        if !out.relay_transports.contains(&proto) {
                            out.relay_transports.push(proto);
                        }
                    }
                    _ => {}
                }
                out.candidates.push(candidate);
            }
            Ok(TransportEvent::IceGatheringComplete) => {
                out.gathering_complete = true;
                break;
            }
            Ok(_) => {}
            Err(mpsc::error::TryRecvError::Empty) => std::thread::sleep(Duration::from_millis(20)),
            Err(_) => break,
        }
    }
    out.elapsed_ms = start.elapsed().as_millis() as u64;
    drop(ch);
    let _ = pipeline.set_state(gst::State::Null);
    Ok(out)
}
