//! Media engine: the only crate that talks to GStreamer.
//!
//! Public API is expressed in plain Rust types (`HostMedia`, `ClientMedia`,
//! `MediaTransport`, `TransportEvent`, `CodecInfo`, stats structs), so the
//! rest of the application could switch to another media backend without
//! changes.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use gst::prelude::*;

pub mod audio_devices;
pub mod client;
pub mod codecs;
pub mod host;
pub mod icecheck;
pub mod sei;
pub mod stats;
pub mod timing;
pub mod transport;
pub mod webrtc;

pub use client::{
    window_sink_works, ClientMedia, ClientMediaConfig, FrameSink, FrameTimestamps, PresentCallback, VideoFrame,
    VideoOutput, WindowTarget,
};
pub use codecs::{probe_decoders, probe_encoders, CodecInfo};
pub use host::{HostMedia, HostMediaConfig};
pub use stats::TransportStats;
pub use transport::{ConnectionState, DataChannelKind, EventSender, IceConfig, MediaTransport, TransportEvent};

#[derive(Debug, thiserror::Error)]
pub enum MediaError {
    #[error("GStreamer initialization failed: {0}")]
    Init(String),
    #[error("required GStreamer element `{0}` is not installed")]
    MissingElement(String),
    #[error("pipeline error: {0}")]
    Pipeline(String),
    #[error("negotiation error: {0}")]
    Negotiation(String),
    #[error("encoder error: {0}")]
    Encoder(String),
    #[error("decoder error: {0}")]
    Decoder(String),
    #[error("data channel error: {0}")]
    Channel(String),
}

impl MediaError {
    pub(crate) fn from_bool(e: gst::glib::BoolError) -> Self {
        MediaError::Pipeline(e.to_string())
    }
}

pub type Result<T> = std::result::Result<T, MediaError>;

/// Initialize GStreamer once per process.
pub fn init() -> Result<()> {
    static INIT: std::sync::OnceLock<std::result::Result<(), String>> = std::sync::OnceLock::new();
    INIT.get_or_init(|| gst::init().map_err(|e| e.to_string())).clone().map_err(MediaError::Init)
}

pub fn gstreamer_version() -> String {
    let _ = init();
    gst::version_string().to_string()
}

pub(crate) fn make(name: &str) -> Result<gst::Element> {
    gst::ElementFactory::make(name).build().map_err(|_| MediaError::MissingElement(name.into()))
}

/// Forward pipeline bus errors/warnings as transport events. Errors raised
/// by audio-output elements are downgraded to warnings: losing the audio
/// device must not end the video session.
pub(crate) fn watch_bus(pipeline: &gst::Pipeline, stopping: Arc<AtomicBool>, events: EventSender) {
    let bus = pipeline.bus().expect("pipeline has a bus");
    let name = pipeline.name().to_string();
    std::thread::Builder::new()
        .name(format!("{name}-bus"))
        .spawn(move || {
            while !stopping.load(Ordering::SeqCst) {
                let Some(msg) = bus.timed_pop(gst::ClockTime::from_mseconds(200)) else { continue };
                match msg.view() {
                    gst::MessageView::Error(e) => {
                        let src = e.src().map(|s| s.path_string().to_string()).unwrap_or_default();
                        let text = format!("{} ({src})", e.error());
                        tracing::debug!(debug = ?e.debug(), "pipeline error from {src}");
                        if src.contains("audio-") {
                            let _ = events.send(TransportEvent::Warning(format!("audio output error: {text}")));
                        } else {
                            let _ = events.send(TransportEvent::Error(text));
                            break;
                        }
                    }
                    gst::MessageView::Warning(w) => {
                        let src = w.src().map(|s| s.path_string().to_string()).unwrap_or_default();
                        tracing::warn!("pipeline warning from {src}: {}", w.error());
                    }
                    gst::MessageView::Eos(_) => {
                        let _ = events.send(TransportEvent::Error("media stream ended".into()));
                        break;
                    }
                    _ => {}
                }
            }
        })
        .expect("spawn bus watcher");
}
