//! Client media session: WebRTC -> RTP depay -> H.264 decode -> BGRx frames
//! handed to a renderer, and Opus -> audio output.
//!
//! The appsink keeps at most one decoded frame and drops older ones, so a
//! slow renderer never builds up latency; renderers should likewise keep
//! only the newest frame (see `FrameSink`).

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

use gst::prelude::*;
use mist_protocol::peer::SdpKind;
use parking_lot::Mutex;
use tokio::sync::oneshot;

use crate::codecs::{self, try_set, DecoderBackend, GstDecoder};
use crate::stats::TransportStats;
use crate::timing::{add_ref_ts, now_us, ref_ts, Ewma, ARRIVAL_REF, CAPTURE_REF, DECODE_REF};
use crate::transport::{DataChannelKind, EventSender, IceConfig, MediaTransport, TransportEvent};
use crate::webrtc::WebRtcPeer;
use crate::{make, sei, MediaError, Result};

pub struct ClientMediaConfig {
    pub ice: IceConfig,
    pub decoder: Option<String>,
    /// RTP jitter buffer size; lower = less latency, less loss tolerance.
    pub jitter_latency_ms: u32,
    pub audio_enabled: bool,
}

impl Default for ClientMediaConfig {
    fn default() -> Self {
        ClientMediaConfig { ice: IceConfig::default(), decoder: None, jitter_latency_ms: 40, audio_enabled: true }
    }
}

/// A decoded BGRx frame in system memory.
pub struct VideoFrame {
    buffer: gst::Buffer,
    pub width: u32,
    pub height: u32,
    pub stride: usize,
    /// Host capture time (host clock, µs) from the frame's SEI timestamp.
    pub capture_host_us: Option<u64>,
    /// When the encoded frame left the RTP depayloader (client clock, µs).
    pub arrival_us: Option<u64>,
    /// When the decoded frame reached the client sink (client clock, µs).
    pub decoded_us: u64,
}

impl VideoFrame {
    /// Run `f` with the mapped pixel data (`stride * height` bytes, BGRx).
    pub fn with_data<R>(&self, f: impl FnOnce(&[u8]) -> R) -> Option<R> {
        let map = self.buffer.map_readable().ok()?;
        Some(f(map.as_slice()))
    }
}

pub trait FrameSink: Send + Sync {
    fn push(&self, frame: VideoFrame);
}

struct ClientTiming {
    decode_ms: Mutex<Ewma>,
    frames_decoded: AtomicU64,
}

pub struct ClientMedia {
    pipeline: gst::Pipeline,
    peer: Arc<WebRtcPeer>,
    timing: Arc<ClientTiming>,
    volume: Arc<Mutex<Option<gst::Element>>>,
    output_filter: Arc<Mutex<Option<gst::Element>>>,
    overlay: Arc<Mutex<Option<gst::Element>>>,
    output_size: Arc<Mutex<Option<(u32, u32)>>>,
    decoder: &'static GstDecoder,
    stopping: Arc<AtomicBool>,
}

fn output_caps(size: Option<(u32, u32)>) -> gst::Caps {
    let mut b = gst::Caps::builder("video/x-raw").field("format", "BGRx");
    if let Some((w, h)) = size {
        b = b.field("width", w as i32).field("height", h as i32).field("pixel-aspect-ratio", gst::Fraction::new(1, 1));
    }
    b.build()
}

impl ClientMedia {
    pub fn start(cfg: ClientMediaConfig, events: EventSender, sink: Arc<dyn FrameSink>) -> Result<Arc<ClientMedia>> {
        crate::init()?;
        let decoder = codecs::select_decoder(cfg.decoder.as_deref())?;
        tracing::info!(decoder = decoder.element_name(), jitter_ms = cfg.jitter_latency_ms, "starting client media");
        let pipeline = gst::Pipeline::with_name("mist-client");
        let peer = WebRtcPeer::new("webrtc", &cfg.ice, Some(cfg.jitter_latency_ms), events.clone())?;
        let webrtc = peer.element().clone();
        webrtc.connect("on-new-transceiver", false, |values| {
            if let Ok(t) = values[1].get::<gst_webrtc::WebRTCRTPTransceiver>() {
                t.set_property("do-nack", true);
            }
            None
        });
        pipeline.add(&webrtc).map_err(MediaError::from_bool)?;

        let media = Arc::new(ClientMedia {
            pipeline: pipeline.clone(),
            peer: peer.clone(),
            timing: Arc::new(ClientTiming {
                decode_ms: Mutex::new(Ewma::new(0.05)),
                frames_decoded: AtomicU64::new(0),
            }),
            volume: Arc::new(Mutex::new(None)),
            output_filter: Arc::new(Mutex::new(None)),
            overlay: Arc::new(Mutex::new(None)),
            output_size: Arc::new(Mutex::new(None)),
            decoder,
            stopping: Arc::new(AtomicBool::new(false)),
        });

        let weak = Arc::downgrade(&media);
        let ev = events.clone();
        let audio_enabled = cfg.audio_enabled;
        webrtc.connect_pad_added(move |_, pad| {
            let Some(media) = weak.upgrade() else { return };
            let caps = pad.current_caps().or_else(|| Some(pad.query_caps(None)));
            let kind = caps
                .as_ref()
                .and_then(|c| c.structure(0))
                .and_then(|s| s.get::<String>("media").ok())
                .unwrap_or_default();
            let result = match kind.as_str() {
                "video" => media.add_video_branch(pad, sink.clone()),
                "audio" if audio_enabled => media.add_audio_branch(pad, &ev),
                "audio" => media.add_discard_branch(pad),
                other => Err(MediaError::Pipeline(format!("unexpected incoming stream `{other}`"))),
            };
            if let Err(e) = result {
                let _ = ev.send(TransportEvent::Error(format!("cannot handle incoming {kind} stream: {e}")));
            }
        });

        crate::watch_bus(&pipeline, media.stopping.clone(), events);
        pipeline
            .set_state(gst::State::Playing)
            .map_err(|_| MediaError::Pipeline("client pipeline failed to start".into()))?;
        Ok(media)
    }

    fn add_video_branch(&self, pad: &gst::Pad, sink: Arc<dyn FrameSink>) -> Result<()> {
        let depay = make("rtph264depay")?;
        let es = make("capsfilter")?;
        es.set_property(
            "caps",
            gst::Caps::builder("video/x-h264").field("stream-format", "byte-stream").field("alignment", "au").build(),
        );
        let parse = make("h264parse")?;
        let decoder = self.decoder.create()?;
        let convert = make("videoconvertscale")?;
        try_set(&convert, "add-borders", "true");
        try_set(
            &convert,
            "n-threads",
            &std::thread::available_parallelism().map(|n| n.get().min(8)).unwrap_or(2).to_string(),
        );
        let filter = make("capsfilter")?;
        filter.set_property("caps", output_caps(*self.output_size.lock()));
        let appsink = gst_app::AppSink::builder().name("video-sink").sync(false).max_buffers(1).drop(true).build();
        let mut els: Vec<gst::Element> = vec![depay, es.clone(), parse, decoder.clone(), convert, filter.clone()];
        // Statistics overlay, drawn after scaling so text stays crisp.
        match gst::ElementFactory::make("textoverlay").name("stats-overlay").build() {
            Ok(overlay) => {
                overlay.set_property("silent", true);
                try_set(&overlay, "font-desc", "Monospace 11");
                try_set(&overlay, "valignment", "top");
                try_set(&overlay, "halignment", "left");
                try_set(&overlay, "shaded-background", "true");
                try_set(&overlay, "wait-text", "false");
                els.push(overlay.clone());
                *self.overlay.lock() = Some(overlay);
            }
            Err(_) => tracing::info!("textoverlay not available; statistics overlay disabled"),
        }
        els.push(appsink.clone().upcast());

        es.static_pad("src").expect("src pad").add_probe(gst::PadProbeType::BUFFER, move |_, info| {
            if let Some(gst::PadProbeData::Buffer(buf)) = &mut info.data {
                let capture = buf.map_readable().ok().and_then(|m| sei::extract_timestamp(m.as_slice()));
                let b = buf.make_mut();
                add_ref_ts(b, &ARRIVAL_REF, now_us());
                if let Some(c) = capture {
                    add_ref_ts(b, &CAPTURE_REF, c);
                }
            }
            gst::PadProbeReturn::Ok
        });
        decoder.static_pad("sink").expect("sink pad").add_probe(gst::PadProbeType::BUFFER, move |_, info| {
            if let Some(gst::PadProbeData::Buffer(buf)) = &mut info.data {
                add_ref_ts(buf.make_mut(), &DECODE_REF, now_us());
            }
            gst::PadProbeReturn::Ok
        });
        let timing = self.timing.clone();
        decoder.static_pad("src").expect("src pad").add_probe(gst::PadProbeType::BUFFER, move |_, info| {
            if let Some(gst::PadProbeData::Buffer(buf)) = &info.data {
                if let Some(t) = ref_ts(buf, &DECODE_REF) {
                    timing.decode_ms.lock().add(now_us().saturating_sub(t) as f64 / 1000.0);
                }
                timing.frames_decoded.fetch_add(1, Ordering::Relaxed);
            }
            gst::PadProbeReturn::Ok
        });

        appsink.set_callbacks(
            gst_app::AppSinkCallbacks::builder()
                .new_sample(move |s| {
                    let sample = s.pull_sample().map_err(|_| gst::FlowError::Eos)?;
                    let (Some(buffer), Some(caps)) = (sample.buffer_owned(), sample.caps()) else {
                        return Ok(gst::FlowSuccess::Ok);
                    };
                    let Ok(info) = gst_video::VideoInfo::from_caps(caps) else { return Ok(gst::FlowSuccess::Ok) };
                    let capture_host_us = ref_ts(&buffer, &CAPTURE_REF);
                    let arrival_us = ref_ts(&buffer, &ARRIVAL_REF);
                    sink.push(VideoFrame {
                        buffer,
                        width: info.width(),
                        height: info.height(),
                        stride: info.stride()[0] as usize,
                        capture_host_us,
                        arrival_us,
                        decoded_us: now_us(),
                    });
                    Ok(gst::FlowSuccess::Ok)
                })
                .build(),
        );

        self.pipeline.add_many(&els).map_err(MediaError::from_bool)?;
        gst::Element::link_many(&els).map_err(|e| MediaError::Pipeline(format!("linking video branch: {e}")))?;
        for e in &els {
            e.sync_state_with_parent().map_err(MediaError::from_bool)?;
        }
        pad.link(&els[0].static_pad("sink").expect("depay sink"))
            .map_err(|e| MediaError::Pipeline(format!("linking webrtc video pad: {e:?}")))?;
        *self.output_filter.lock() = Some(filter);
        Ok(())
    }

    fn audio_sink(events: &EventSender) -> Result<gst::Element> {
        let candidates: &[&str] =
            if cfg!(windows) { &["wasapi2sink", "autoaudiosink"] } else { &["pulsesink", "autoaudiosink"] };
        for name in candidates {
            let Ok(sink) = gst::ElementFactory::make(name).name("audio-sink").build() else { continue };
            try_set(&sink, "buffer-time", "40000");
            try_set(&sink, "latency-time", "10000");
            try_set(&sink, "low-latency", "true");
            if sink.set_state(gst::State::Ready).is_ok() {
                return Ok(sink);
            }
            let _ = sink.set_state(gst::State::Null);
            tracing::warn!("audio output {name} cannot be opened");
        }
        let _ = events.send(TransportEvent::Warning("no usable audio output device; audio is muted".into()));
        Ok(make("fakesink")?)
    }

    fn add_audio_branch(&self, pad: &gst::Pad, events: &EventSender) -> Result<()> {
        let depay = make("rtpopusdepay")?;
        let dec = make("opusdec")?;
        try_set(&dec, "plc", "true");
        try_set(&dec, "use-inband-fec", "true");
        let convert = make("audioconvert")?;
        let resample = make("audioresample")?;
        let volume = gst::ElementFactory::make("volume")
            .name("audio-volume")
            .build()
            .map_err(|_| MediaError::MissingElement("volume".into()))?;
        let sink = Self::audio_sink(events)?;
        let els = [depay, dec, convert, resample, volume.clone(), sink];
        self.pipeline.add_many(&els).map_err(MediaError::from_bool)?;
        gst::Element::link_many(&els).map_err(|e| MediaError::Pipeline(format!("linking audio branch: {e}")))?;
        for e in &els {
            e.sync_state_with_parent().map_err(MediaError::from_bool)?;
        }
        pad.link(&els[0].static_pad("sink").expect("depay sink"))
            .map_err(|e| MediaError::Pipeline(format!("linking webrtc audio pad: {e:?}")))?;
        let mut slot = self.volume.lock();
        if let Some(prev) = slot.as_ref() {
            volume.set_property("volume", prev.property::<f64>("volume"));
            volume.set_property("mute", prev.property::<bool>("mute"));
        }
        *slot = Some(volume);
        Ok(())
    }

    fn add_discard_branch(&self, pad: &gst::Pad) -> Result<()> {
        let sink = make("fakesink")?;
        sink.set_property("async", false);
        self.pipeline.add(&sink).map_err(MediaError::from_bool)?;
        sink.sync_state_with_parent().map_err(MediaError::from_bool)?;
        pad.link(&sink.static_pad("sink").expect("sink pad"))
            .map_err(|e| MediaError::Pipeline(format!("linking discard: {e:?}")))?;
        Ok(())
    }

    /// Ask the pipeline to scale decoded frames to the window size
    /// (letterboxed); `None` = native stream size.
    pub fn set_output_size(&self, size: Option<(u32, u32)>) {
        let size = size.map(|(w, h)| (w.max(2) & !1, h.max(2) & !1));
        let mut cur = self.output_size.lock();
        if *cur == size {
            return;
        }
        *cur = size;
        if let Some(f) = self.output_filter.lock().as_ref() {
            f.set_property("caps", output_caps(size));
        }
    }

    /// Show (or hide with `None`) text drawn over the video.
    pub fn set_overlay_text(&self, text: Option<&str>) {
        if let Some(o) = self.overlay.lock().as_ref() {
            match text {
                Some(t) => {
                    o.set_property("text", t);
                    o.set_property("silent", false);
                }
                None => o.set_property("silent", true),
            }
        }
    }

    pub fn set_volume(&self, volume: f64) {
        if let Some(v) = self.volume.lock().as_ref() {
            v.set_property("volume", volume.clamp(0.0, 2.0));
        }
    }

    pub fn set_muted(&self, muted: bool) {
        if let Some(v) = self.volume.lock().as_ref() {
            v.set_property("mute", muted);
        }
    }

    pub fn decode_ms_avg(&self) -> Option<f64> {
        self.timing.decode_ms.lock().get()
    }

    pub fn frames_decoded(&self) -> u64 {
        self.timing.frames_decoded.load(Ordering::Relaxed)
    }

    pub fn decoder_name(&self) -> &'static str {
        self.decoder.element_name()
    }

    pub fn stop(&self) {
        if self.stopping.swap(true, Ordering::SeqCst) {
            return;
        }
        self.peer.close_channels();
        let _ = self.pipeline.set_state(gst::State::Null);
        tracing::info!("client media stopped");
    }
}

impl Drop for ClientMedia {
    fn drop(&mut self) {
        self.stop();
    }
}

impl MediaTransport for ClientMedia {
    fn set_remote_sdp(&self, kind: SdpKind, sdp: &str) -> Result<()> {
        self.peer.set_remote_sdp(kind, sdp)
    }
    fn add_remote_candidate(&self, mline_index: u32, candidate: &str) -> Result<()> {
        self.peer.add_remote_candidate(mline_index, candidate);
        Ok(())
    }
    fn send(&self, channel: DataChannelKind, text: &str) -> Result<()> {
        self.peer.send(channel, text)
    }
    fn request_stats(&self) -> oneshot::Receiver<TransportStats> {
        self.peer.request_stats()
    }
    fn close(&self) {
        self.stop();
    }
}
