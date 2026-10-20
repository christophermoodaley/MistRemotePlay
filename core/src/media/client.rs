//! Client media session: WebRTC -> RTP depay -> H.264 decode -> BGRx frames
//! handed to a renderer, and Opus -> audio output.
//!
//! The appsink keeps at most one decoded frame and drops older ones, so a
//! slow renderer never builds up latency; renderers should likewise keep
//! only the newest frame (see `FrameSink`).

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

use crate::protocol::peer::SdpKind;
use gst::prelude::*;
use gst_video::prelude::*;
use parking_lot::Mutex;
use tokio::sync::oneshot;

use crate::media::codecs::{self, try_set, DecoderBackend, GstDecoder};
use crate::media::stats::TransportStats;
use crate::media::timing::{
    add_ref_ts, now_us, ref_ts, Ewma, ARRIVAL_REF, CAPTURE_REF, DECODE_OUT_REF, DECODE_REF, ENCODED_REF,
};
use crate::media::transport::{DataChannelKind, EventSender, IceConfig, MediaTransport, TransportEvent};
use crate::media::webrtc::WebRtcPeer;
use crate::media::{make, sei, MediaError, Result};

pub struct ClientMediaConfig {
    pub ice: IceConfig,
    pub decoder: Option<String>,
    /// Requested stream size, to skip decoders that cannot handle it.
    pub stream_size: Option<(u32, u32)>,
    /// RTP jitter buffer size; lower = less latency, less loss tolerance.
    pub jitter_latency_ms: u32,
    pub audio_enabled: bool,
}

impl Default for ClientMediaConfig {
    fn default() -> Self {
        ClientMediaConfig {
            ice: IceConfig::default(),
            decoder: None,
            stream_size: None,
            jitter_latency_ms: 40,
            audio_enabled: true,
        }
    }
}

/// Per-frame timestamps used to break down latency.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FrameTimestamps {
    /// Host capture time (host clock, µs) from the frame's SEI timestamp.
    pub capture_host_us: Option<u64>,
    /// When the host finished encoding the frame (host clock, µs, SEI).
    pub encoded_host_us: Option<u64>,
    /// When the encoded frame left the RTP depayloader (client clock, µs).
    pub arrival_us: Option<u64>,
    /// When the decoder produced the frame (client clock, µs).
    pub decode_end_us: Option<u64>,
    /// When the frame reached the renderer: the appsink callback for
    /// [`VideoOutput::Frames`], the draw call for [`VideoOutput::Window`]
    /// (client clock, µs).
    pub sink_us: u64,
}

impl FrameTimestamps {
    fn from_buffer(buf: &gst::BufferRef, sink_us: u64) -> Self {
        FrameTimestamps {
            capture_host_us: ref_ts(buf, &CAPTURE_REF),
            encoded_host_us: ref_ts(buf, &ENCODED_REF),
            arrival_us: ref_ts(buf, &ARRIVAL_REF),
            decode_end_us: ref_ts(buf, &DECODE_OUT_REF),
            sink_us,
        }
    }
}

/// A decoded BGRx frame in system memory.
pub struct VideoFrame {
    buffer: gst::Buffer,
    pub width: u32,
    pub height: u32,
    pub stride: usize,
    pub times: FrameTimestamps,
}

/// A native window the video sink draws into.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WindowTarget {
    /// X11 window id (the GL sink adds a child window that passes input
    /// events through to it).
    X11 { window: u64 },
}

/// Called (from the render thread) when a frame is drawn.
pub type PresentCallback = Arc<dyn Fn(&FrameTimestamps) + Send + Sync>;

/// Where decoded video goes.
pub enum VideoOutput {
    /// BGRx frames in system memory, converted and scaled on the CPU
    /// (headless sinks, the software window renderer).
    Frames(Arc<dyn FrameSink>),
    /// A GPU video sink embedded in a native window: colour conversion,
    /// scaling and the statistics overlay run on the GPU and no frame is
    /// copied back to system memory.
    Window { target: WindowTarget, on_present: PresentCallback },
}

impl VideoOutput {
    pub fn is_window(&self) -> bool {
        matches!(self, VideoOutput::Window { .. })
    }
}

impl VideoFrame {
    /// Run `f` with the mapped pixel data (`stride * height` bytes, BGRx).
    pub fn with_data<R>(&self, f: impl FnOnce(&[u8]) -> R) -> Option<R> {
        let map = self.buffer.map_readable().ok()?;
        Some(f(map.as_slice()))
    }
}

/// The statistics `textoverlay`, linked between the output capsfilter and
/// the appsink only while visible. Relinking happens from an IDLE pad
/// probe, i.e. between buffers, so the stream is never interrupted.
struct StatsOverlay {
    element: gst::Element,
    upstream: gst::Element,
    downstream: gst::Element,
    pipeline: gst::Pipeline,
    want_visible: Arc<AtomicBool>,
    /// An IDLE probe is installed and will apply `want_visible`.
    pending: Arc<AtomicBool>,
}

impl StatsOverlay {
    fn set_visible(&self, visible: bool) {
        self.want_visible.store(visible, Ordering::SeqCst);
        if self.pending.swap(true, Ordering::SeqCst) {
            return;
        }
        let (overlay, up, down, pipeline) =
            (self.element.clone(), self.upstream.clone(), self.downstream.clone(), self.pipeline.clone());
        let (want, pending) = (self.want_visible.clone(), self.pending.clone());
        let Some(pad) = up.static_pad("src") else { return };
        pad.add_probe(gst::PadProbeType::IDLE, move |up_src, _| {
            pending.store(false, Ordering::SeqCst);
            let linked = overlay.parent().is_some();
            let visible = want.load(Ordering::SeqCst);
            let result = if visible && !linked {
                insert_between(&pipeline, &overlay, up_src, &down)
            } else if !visible && linked {
                remove_between(&pipeline, &overlay, up_src, &down)
            } else {
                Ok(())
            };
            if let Err(e) = result {
                tracing::warn!("statistics overlay relink failed: {e}");
            }
            gst::PadProbeReturn::Remove
        });
    }
}

fn pads(overlay: &gst::Element, down: &gst::Element) -> std::result::Result<(gst::Pad, gst::Pad, gst::Pad), String> {
    let ov_sink = overlay.static_pad("video_sink").ok_or("overlay has no video_sink pad")?;
    let ov_src = overlay.static_pad("src").ok_or("overlay has no src pad")?;
    let down_sink = down.static_pad("sink").ok_or("sink element has no sink pad")?;
    Ok((ov_sink, ov_src, down_sink))
}

/// `up_src -> down` becomes `up_src -> overlay -> down`; restores the
/// direct link on failure so video keeps flowing.
fn insert_between(
    pipeline: &gst::Pipeline,
    overlay: &gst::Element,
    up_src: &gst::Pad,
    down: &gst::Element,
) -> std::result::Result<(), String> {
    let (ov_sink, ov_src, down_sink) = pads(overlay, down)?;
    pipeline.add(overlay).map_err(|e| e.to_string())?;
    // Activate before linking: a buffer pushed into an inactive (flushing)
    // pad would stop the upstream streaming thread.
    if let Err(e) = overlay.sync_state_with_parent() {
        let _ = pipeline.remove(overlay);
        return Err(e.to_string());
    }
    let _ = up_src.unlink(&down_sink);
    // No caps check at link time: a sink that already negotiated reports
    // only its current caps; the sticky caps event renegotiates the new
    // path before the next buffer.
    let check = gst::PadLinkCheck::HIERARCHY;
    let linked = up_src
        .link_full(&ov_sink, check)
        .map_err(|e| format!("upstream -> overlay: {e:?}"))
        .and_then(|_| ov_src.link_full(&down_sink, check).map(|_| ()).map_err(|e| format!("overlay -> sink: {e:?}")));
    if linked.is_err() {
        let _ = up_src.unlink(&ov_sink);
        let _ = ov_src.unlink(&down_sink);
        let _ = overlay.set_state(gst::State::Null);
        let _ = pipeline.remove(overlay);
        let _ = up_src.link_full(&down_sink, check);
    }
    linked
}

fn remove_between(
    pipeline: &gst::Pipeline,
    overlay: &gst::Element,
    up_src: &gst::Pad,
    down: &gst::Element,
) -> std::result::Result<(), String> {
    let (ov_sink, ov_src, down_sink) = pads(overlay, down)?;
    let _ = up_src.unlink(&ov_sink);
    let _ = ov_src.unlink(&down_sink);
    let _ = overlay.set_state(gst::State::Null);
    pipeline.remove(overlay).map_err(|e| e.to_string())?;
    up_src
        .link_full(&down_sink, gst::PadLinkCheck::HIERARCHY)
        .map(|_| ())
        .map_err(|e| format!("upstream -> sink: {e:?}"))
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
    overlay: Arc<Mutex<Option<StatsOverlay>>>,
    output_size: Arc<Mutex<Option<(u32, u32)>>>,
    decoder: &'static GstDecoder,
    /// The embedded GPU sink when rendering into a window.
    window_sink: Arc<Mutex<Option<gst::Element>>>,
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
    pub fn start(cfg: ClientMediaConfig, events: EventSender, output: VideoOutput) -> Result<Arc<ClientMedia>> {
        crate::media::init()?;
        let decoder = codecs::select_decoder(cfg.decoder.as_deref(), cfg.stream_size)?;
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
            window_sink: Arc::new(Mutex::new(None)),
            stopping: Arc::new(AtomicBool::new(false)),
        });
        let output = Arc::new(output);

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
                "video" => media.add_video_branch(pad, &output),
                "audio" if audio_enabled => media.add_audio_branch(pad, &ev),
                "audio" => media.add_discard_branch(pad),
                other => Err(MediaError::Pipeline(format!("unexpected incoming stream `{other}`"))),
            };
            if let Err(e) = result {
                let _ = ev.send(TransportEvent::Error(format!("cannot handle incoming {kind} stream: {e}")));
            }
        });

        crate::media::watch_bus(&pipeline, media.stopping.clone(), events);
        pipeline
            .set_state(gst::State::Playing)
            .map_err(|_| MediaError::Pipeline("client pipeline failed to start".into()))?;
        Ok(media)
    }

    fn add_video_branch(&self, pad: &gst::Pad, output: &VideoOutput) -> Result<()> {
        let depay = make("rtph264depay")?;
        let es = make("capsfilter")?;
        es.set_property(
            "caps",
            gst::Caps::builder("video/x-h264").field("stream-format", "byte-stream").field("alignment", "au").build(),
        );
        let parse = make("h264parse")?;
        let decoder = self.decoder.create()?;
        self.install_timing_probes(&es, &decoder);
        let mut els: Vec<gst::Element> = vec![depay, es, parse, decoder.clone()];

        // The overlay goes right before the sink: on the CPU path after
        // scaling (so text stays crisp), on the GPU path the sink composes
        // it as an overlay-composition meta.
        let (overlay_upstream, sink) = match output {
            VideoOutput::Frames(frames) => {
                let convert = make("videoconvertscale")?;
                try_set(&convert, "add-borders", "true");
                try_set(
                    &convert,
                    "n-threads",
                    &std::thread::available_parallelism().map(|n| n.get().min(8)).unwrap_or(2).to_string(),
                );
                let filter = make("capsfilter")?;
                filter.set_property("caps", output_caps(*self.output_size.lock()));
                let appsink = Self::frame_appsink(frames.clone());
                els.extend([convert, filter.clone()]);
                *self.output_filter.lock() = Some(filter.clone());
                (filter, appsink.upcast())
            }
            VideoOutput::Window { target, on_present } => {
                // Like the appsink's drop policy: a slow renderer drops
                // frames instead of stalling the decoder and filling the
                // jitter buffer.
                let queue = make("queue")?;
                queue.set_property_from_str("leaky", "downstream");
                queue.set_property("max-size-buffers", 1u32);
                queue.set_property("max-size-bytes", 0u32);
                queue.set_property("max-size-time", 0u64);
                let sink = window_sink(*target)?;
                connect_present(&sink, on_present.clone());
                *self.window_sink.lock() = Some(sink.clone());
                els.push(queue.clone());
                (queue, sink)
            }
        };
        self.prepare_overlay(&overlay_upstream, &sink);
        els.push(sink);

        self.pipeline.add_many(&els).map_err(MediaError::from_bool)?;
        gst::Element::link_many(&els).map_err(|e| MediaError::Pipeline(format!("linking video branch: {e}")))?;
        for e in &els {
            e.sync_state_with_parent().map_err(MediaError::from_bool)?;
        }
        pad.link(&els[0].static_pad("sink").expect("depay sink"))
            .map_err(|e| MediaError::Pipeline(format!("linking webrtc video pad: {e:?}")))?;
        Ok(())
    }

    /// Per-frame timestamps: SEI host times and arrival after the
    /// depayloader, decode start/end around the decoder.
    fn install_timing_probes(&self, es: &gst::Element, decoder: &gst::Element) {
        es.static_pad("src").expect("src pad").add_probe(gst::PadProbeType::BUFFER, move |_, info| {
            if let Some(gst::PadProbeData::Buffer(buf)) = &mut info.data {
                let arrival = now_us();
                let times = buf.map_readable().ok().and_then(|m| sei::extract_times(m.as_slice()));
                let b = buf.make_mut();
                add_ref_ts(b, &ARRIVAL_REF, arrival);
                if let Some(t) = times {
                    add_ref_ts(b, &CAPTURE_REF, t.capture_us);
                    if let Some(e) = t.encoded_us {
                        add_ref_ts(b, &ENCODED_REF, e);
                    }
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
            if let Some(gst::PadProbeData::Buffer(buf)) = &mut info.data {
                let now = now_us();
                if let Some(t) = ref_ts(buf, &DECODE_REF) {
                    timing.decode_ms.lock().add(now.saturating_sub(t) as f64 / 1000.0);
                }
                add_ref_ts(buf.make_mut(), &DECODE_OUT_REF, now);
                timing.frames_decoded.fetch_add(1, Ordering::Relaxed);
            }
            gst::PadProbeReturn::Ok
        });
    }

    fn frame_appsink(sink: Arc<dyn FrameSink>) -> gst_app::AppSink {
        let appsink = gst_app::AppSink::builder().name("video-sink").sync(false).max_buffers(1).drop(true).build();
        appsink.set_callbacks(
            gst_app::AppSinkCallbacks::builder()
                .new_sample(move |s| {
                    let sample = s.pull_sample().map_err(|_| gst::FlowError::Eos)?;
                    let (Some(buffer), Some(caps)) = (sample.buffer_owned(), sample.caps()) else {
                        return Ok(gst::FlowSuccess::Ok);
                    };
                    let Ok(info) = gst_video::VideoInfo::from_caps(caps) else { return Ok(gst::FlowSuccess::Ok) };
                    sink.push(VideoFrame {
                        width: info.width(),
                        height: info.height(),
                        stride: info.stride()[0] as usize,
                        times: FrameTimestamps::from_buffer(&buffer, now_us()),
                        buffer,
                    });
                    Ok(gst::FlowSuccess::Ok)
                })
                .build(),
        );
        appsink
    }

    /// Statistics overlay, linked between `upstream` and `downstream` only
    /// while visible: even a silent textoverlay costs milliseconds per
    /// frame at 1080p.
    fn prepare_overlay(&self, upstream: &gst::Element, downstream: &gst::Element) {
        match gst::ElementFactory::make("textoverlay").name("stats-overlay").build() {
            Ok(overlay) => {
                try_set(&overlay, "font-desc", "Monospace 11");
                try_set(&overlay, "valignment", "top");
                try_set(&overlay, "halignment", "left");
                try_set(&overlay, "line-alignment", "left");
                try_set(&overlay, "shaded-background", "true");
                try_set(&overlay, "wait-text", "false");
                *self.overlay.lock() = Some(StatsOverlay {
                    element: overlay,
                    upstream: upstream.clone(),
                    downstream: downstream.clone(),
                    pipeline: self.pipeline.clone(),
                    want_visible: Arc::new(AtomicBool::new(false)),
                    pending: Arc::new(AtomicBool::new(false)),
                });
            }
            Err(_) => tracing::info!("textoverlay not available; statistics overlay disabled"),
        }
    }

    /// Whether video is drawn by an embedded GPU sink.
    pub fn renders_to_window(&self) -> bool {
        self.window_sink.lock().is_some()
    }

    /// Redraw the last frame (window resized or exposed).
    pub fn expose(&self) {
        if let Some(sink) = self.window_sink.lock().as_ref() {
            if let Some(o) = sink.dynamic_cast_ref::<gst_video::VideoOverlay>() {
                o.expose();
            }
        }
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
        make("fakesink")
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
            if let Some(t) = text {
                o.element.set_property("text", t);
            }
            o.set_visible(text.is_some());
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

/// Create the GPU video sink for `target`.
fn window_sink(target: WindowTarget) -> Result<gst::Element> {
    match target {
        WindowTarget::X11 { window } => {
            let sink = gst::ElementFactory::make("glimagesink")
                .name("video-window-sink")
                .build()
                .map_err(|_| MediaError::MissingElement("glimagesink".into()))?;
            sink.set_property("sync", false);
            try_set(&sink, "force-aspect-ratio", "true");
            // Leave input to the application window: with event handling
            // off the GL child window selects no input events, so X11
            // delivers them to our window.
            try_set(&sink, "handle-events", "false");
            try_set(&sink, "qos", "false");
            let overlay = sink
                .dynamic_cast_ref::<gst_video::VideoOverlay>()
                .ok_or_else(|| MediaError::Pipeline("glimagesink does not implement VideoOverlay".into()))?;
            // SAFETY: `window` is a live X11 window owned by the caller for
            // at least as long as this pipeline runs.
            unsafe { overlay.set_window_handle(window as usize) };
            Ok(sink)
        }
    }
}

/// Report each newly drawn frame. `client-draw` is emitted on the GL
/// thread right before a frame is drawn; returning `false` keeps the
/// sink's own drawing. Redraws of the same frame (expose) are ignored.
fn connect_present(sink: &gst::Element, on_present: PresentCallback) {
    if sink.find_property("last-sample").is_none() {
        return;
    }
    let last = Mutex::new(None::<(Option<gst::ClockTime>, Option<u64>)>);
    sink.connect("client-draw", false, move |values| {
        let now = now_us();
        if let Some(sample) = values.get(2).and_then(|v| v.get::<gst::Sample>().ok()) {
            if let Some(buf) = sample.buffer() {
                let times = FrameTimestamps::from_buffer(buf, now);
                let key = (buf.pts(), times.capture_host_us);
                let mut last = last.lock();
                if *last != Some(key) {
                    *last = Some(key);
                    drop(last);
                    on_present(&times);
                }
            }
        }
        Some(false.to_value())
    });
}

/// Frames drawn by [`window_sink_works`]; the first few (context and
/// shader setup) are not timed.
const RENDER_PROBE_FRAMES: i32 = 24;
const RENDER_PROBE_WARMUP: usize = 6;
/// A GPU renderer slower than this per 1080p frame (e.g. software OpenGL
/// such as llvmpipe) is rejected in favour of the CPU renderer. ARM boards
/// such as the Raspberry Pi get more room: their GPUs are slower, and the
/// CPU renderer is slower still there.
pub const MAX_GPU_FRAME_MS: f64 = if cfg!(target_arch = "aarch64") { 10.0 } else { 4.0 };

/// Check that the GPU sink can draw into `target`: draws 1080p frames as
/// fast as possible and returns the measured mean time per frame (ms).
/// Compare with [`MAX_GPU_FRAME_MS`] to reject slow (software) OpenGL.
pub fn window_sink_works(target: WindowTarget) -> std::result::Result<f64, String> {
    crate::media::init().map_err(|e| e.to_string())?;
    let pipeline = gst::Pipeline::new();
    let src = gst::ElementFactory::make("videotestsrc")
        .property("num-buffers", RENDER_PROBE_FRAMES)
        .property_from_str("pattern", "black")
        .build()
        .map_err(|e| e.to_string())?;
    let caps = gst::ElementFactory::make("capsfilter")
        .property(
            "caps",
            gst::Caps::builder("video/x-raw")
                .field("format", "I420")
                .field("width", 1920i32)
                .field("height", 1080i32)
                .build(),
        )
        .build()
        .map_err(|e| e.to_string())?;
    let sink = window_sink(target).map_err(|e| e.to_string())?;
    let draws = Arc::new(Mutex::new(Vec::<u64>::new()));
    let d = draws.clone();
    sink.connect("client-draw", false, move |_| {
        d.lock().push(now_us());
        Some(false.to_value())
    });
    pipeline.add_many([&src, &caps, &sink]).map_err(|e| e.to_string())?;
    gst::Element::link_many([&src, &caps, &sink]).map_err(|e| e.to_string())?;
    let bus = pipeline.bus().ok_or("no bus")?;
    pipeline.set_state(gst::State::Playing).map_err(|e| format!("cannot start: {e}"))?;
    let res = match bus
        .timed_pop_filtered(gst::ClockTime::from_seconds(10), &[gst::MessageType::Eos, gst::MessageType::Error])
    {
        Some(msg) => match msg.view() {
            gst::MessageView::Eos(_) => Ok(()),
            gst::MessageView::Error(e) => Err(format!("{}", e.error())),
            _ => Err("unexpected message".into()),
        },
        None => Err("timed out".into()),
    };
    let _ = pipeline.set_state(gst::State::Null);
    res?;
    let draws = draws.lock();
    let timed = draws.get(RENDER_PROBE_WARMUP..).filter(|d| d.len() >= 2).ok_or("the sink did not draw")?;
    Ok((timed[timed.len() - 1] - timed[0]) as f64 / 1000.0 / (timed.len() - 1) as f64)
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
