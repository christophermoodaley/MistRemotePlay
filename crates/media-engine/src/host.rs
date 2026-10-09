//! Host media session: capture -> scale/convert -> H.264 -> RTP -> WebRTC,
//! plus system audio -> Opus -> RTP -> WebRTC.
//!
//! Backpressure: a leaky single-buffer queue sits between capture and the
//! encoder, so when encoding falls behind the oldest frame is discarded
//! instead of accumulating latency. The audio capture device runs in its
//! own pipeline (feeding an `appsrc`) so device loss or default-device
//! changes restart audio without tearing down video.

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use gst::prelude::*;
use mist_audio_api::AudioSourceSpec;
use mist_capture_api::CaptureSourceSpec;
use mist_protocol::control::HostStats;
use mist_protocol::peer::SdpKind;
use mist_protocol::settings::StreamSettings;
use parking_lot::Mutex;
use tokio::sync::oneshot;

use crate::codecs::{self, try_set, EncoderBackend, EncoderParams, GstEncoder, InputMemory};
use crate::stats::TransportStats;
use crate::timing::{add_ref_ts, now_us, ref_ts, Ewma, RateCounter, CAPTURE_REF, ENCODE_REF};
use crate::transport::{DataChannelKind, EventSender, IceConfig, MediaTransport, TransportEvent};
use crate::webrtc::WebRtcPeer;
use crate::{make, sei, MediaError, Result};

pub const VIDEO_PT: u32 = 96;
pub const AUDIO_PT: u32 = 111;

pub struct HostMediaConfig {
    pub capture: CaptureSourceSpec,
    pub audio: Option<AudioSourceSpec>,
    pub settings: StreamSettings,
    pub ice: IceConfig,
}

#[derive(Debug)]
struct HostTiming {
    encode_ms: Mutex<Ewma>,
    capture_rate: Mutex<RateCounter>,
    queue_in: AtomicU64,
    queue_out: AtomicU64,
}

impl Default for HostTiming {
    fn default() -> Self {
        HostTiming {
            encode_ms: Mutex::new(Ewma::new(0.05)),
            capture_rate: Mutex::new(RateCounter::new(Duration::from_secs(2))),
            queue_in: AtomicU64::new(0),
            queue_out: AtomicU64::new(0),
        }
    }
}

pub struct HostMedia {
    pipeline: gst::Pipeline,
    peer: Arc<WebRtcPeer>,
    encoder: gst::Element,
    backend: &'static GstEncoder,
    timing: Arc<HostTiming>,
    width: u32,
    height: u32,
    bitrate_kbps: AtomicU32,
    stopping: Arc<AtomicBool>,
    audio: Option<AudioCapture>,
    _capture_lease: Option<Arc<dyn mist_capture_api::CaptureLease>>,
}

fn fit_within(src: (u32, u32), max: (u32, u32)) -> (u32, u32) {
    let scale = f64::min(max.0 as f64 / src.0 as f64, max.1 as f64 / src.1 as f64).min(1.0);
    let w = ((src.0 as f64 * scale).round() as u32).max(2) & !1;
    let h = ((src.1 as f64 * scale).round() as u32).max(2) & !1;
    (w, h)
}

/// Elements producing raw video for the given capture source.
fn video_source(spec: &CaptureSourceSpec, fps: u32, gpu_d3d11: bool) -> Result<Vec<gst::Element>> {
    let fps_caps = |name: &str| gst::Caps::builder(name).field("framerate", gst::Fraction::new(fps as i32, 1)).build();
    match spec {
        CaptureSourceSpec::X11 { display_name, x, y, width, height, show_cursor } => {
            let src = make("ximagesrc")?;
            if let Some(d) = display_name {
                src.set_property("display-name", d);
            }
            src.set_property("startx", *x.max(&0) as u32);
            src.set_property("starty", *y.max(&0) as u32);
            src.set_property("endx", (*x.max(&0) as u32) + width - 1);
            src.set_property("endy", (*y.max(&0) as u32) + height - 1);
            src.set_property("use-damage", false);
            src.set_property("show-pointer", *show_cursor);
            let caps = make("capsfilter")?;
            caps.set_property("caps", fps_caps("video/x-raw"));
            Ok(vec![src, caps])
        }
        CaptureSourceSpec::PipeWire { fd, node_id, .. } => {
            let src = make("pipewiresrc")?;
            src.set_property("fd", *fd);
            src.set_property("path", node_id.to_string());
            try_set(&src, "do-timestamp", "true");
            // Re-send the last frame when the screen is static so the
            // encoder keeps producing (needed for fresh joins/keyframes).
            try_set(&src, "keepalive-time", "1000");
            let rate = make("videorate")?;
            try_set(&rate, "drop-only", "true");
            try_set(&rate, "max-rate", &fps.to_string());
            Ok(vec![src, rate])
        }
        CaptureSourceSpec::D3D11 { monitor_handle, show_cursor, .. } => {
            let src = make("d3d11screencapturesrc")?;
            src.set_property("monitor-handle", *monitor_handle);
            src.set_property("show-cursor", *show_cursor);
            let caps = make("capsfilter")?;
            caps.set_property("caps", fps_caps("video/x-raw(memory:D3D11Memory)"));
            if gpu_d3d11 {
                Ok(vec![src, caps])
            } else {
                Ok(vec![src, caps, make("d3d11download")?])
            }
        }
        CaptureSourceSpec::TestPattern { pattern, width, height } => {
            let src = make("videotestsrc")?;
            src.set_property("is-live", true);
            src.set_property_from_str("pattern", pattern);
            let caps = make("capsfilter")?;
            caps.set_property(
                "caps",
                gst::Caps::builder("video/x-raw")
                    .field("width", *width as i32)
                    .field("height", *height as i32)
                    .field("framerate", gst::Fraction::new(fps as i32, 1))
                    .build(),
            );
            // A running clock makes every frame unique (end-to-end tests
            // verify the client sees changing content).
            let overlay = gst::ElementFactory::make("timeoverlay").build().ok();
            let mut v = vec![src, caps];
            if let Some(o) = overlay {
                o.set_property_from_str("time-mode", "running-time");
                v.push(o);
            }
            Ok(v)
        }
    }
}

impl HostMedia {
    pub fn start(cfg: HostMediaConfig, events: EventSender) -> Result<Arc<HostMedia>> {
        crate::init()?;
        let settings = &cfg.settings;
        let backend = codecs::select_encoder(settings.encoder.as_deref())?;
        let gpu_d3d11 =
            matches!(cfg.capture, CaptureSourceSpec::D3D11 { .. }) && backend.input_memory() == InputMemory::D3D11;
        let (width, height) = match cfg.capture.native_size() {
            Some(native) => fit_within(native, (settings.width, settings.height)),
            None => (settings.width, settings.height),
        };
        let bitrate = settings.start_bitrate_kbps();
        tracing::info!(
            encoder = backend.element_name(),
            width,
            height,
            fps = settings.fps,
            bitrate_kbps = bitrate,
            capture = ?cfg.capture.kind(),
            "starting host media"
        );

        let pipeline = gst::Pipeline::with_name("mist-host");
        let peer = WebRtcPeer::new("webrtc", &cfg.ice, None, events.clone())?;
        let webrtc = peer.element().clone();
        webrtc.connect("on-new-transceiver", false, |values| {
            if let Ok(t) = values[1].get::<gst_webrtc::WebRTCRTPTransceiver>() {
                t.set_property("direction", gst_webrtc::WebRTCRTPTransceiverDirection::Sendonly);
                t.set_property("do-nack", true);
            }
            None
        });
        pipeline.add(&webrtc).map_err(MediaError::from_bool)?;

        // ---- video branch ----
        let timing = Arc::new(HostTiming::default());
        let mut chain = video_source(&cfg.capture, settings.fps, gpu_d3d11)?;
        let source_tail = chain.last().cloned().expect("source chain is never empty");
        let queue = make("queue")?;
        queue.set_property_from_str("leaky", "downstream");
        queue.set_property("max-size-buffers", 1u32);
        queue.set_property("max-size-bytes", 0u32);
        queue.set_property("max-size-time", 0u64);
        chain.push(queue.clone());
        let raw_caps = if gpu_d3d11 {
            chain.push(make("d3d11convert")?);
            gst::Caps::builder("video/x-raw(memory:D3D11Memory)")
        } else {
            let convert = make("videoconvertscale")?;
            try_set(&convert, "add-borders", "true");
            try_set(&convert, "n-threads", &std::thread::available_parallelism().map(|n| n.get().min(8)).unwrap_or(2).to_string());
            chain.push(convert);
            gst::Caps::builder("video/x-raw")
        }
        .field("format", backend.input_format())
        .field("width", width as i32)
        .field("height", height as i32)
        .field("pixel-aspect-ratio", gst::Fraction::new(1, 1))
        .build();
        let raw_filter = make("capsfilter")?;
        raw_filter.set_property("caps", raw_caps);
        chain.push(raw_filter);
        let params =
            EncoderParams { width, height, fps: settings.fps, bitrate_kbps: bitrate, quality: settings.encoder_quality };
        let encoder = backend.create(&params)?;
        chain.push(encoder.clone());
        let parse = make("h264parse")?;
        parse.set_property("config-interval", -1i32);
        chain.push(parse.clone());
        let es_filter = make("capsfilter")?;
        es_filter.set_property(
            "caps",
            gst::Caps::builder("video/x-h264").field("stream-format", "byte-stream").field("alignment", "au").build(),
        );
        chain.push(es_filter.clone());
        let pay = make("rtph264pay")?;
        pay.set_property("config-interval", -1i32);
        pay.set_property("pt", VIDEO_PT);
        pay.set_property("mtu", 1200u32);
        try_set(&pay, "aggregate-mode", "zero-latency");
        chain.push(pay);
        let rtp_filter = make("capsfilter")?;
        rtp_filter.set_property(
            "caps",
            gst::Caps::builder("application/x-rtp")
                .field("media", "video")
                .field("encoding-name", "H264")
                .field("payload", VIDEO_PT as i32)
                .field("clock-rate", 90000i32)
                .build(),
        );
        chain.push(rtp_filter.clone());
        pipeline.add_many(&chain).map_err(MediaError::from_bool)?;
        gst::Element::link_many(&chain).map_err(|e| MediaError::Pipeline(format!("linking video chain: {e}")))?;
        rtp_filter.link(&webrtc).map_err(|e| MediaError::Pipeline(format!("linking video to webrtcbin: {e}")))?;

        Self::install_probes(&timing, &source_tail, &queue, &encoder, &es_filter);

        // ---- audio branch ----
        let audio = match &cfg.audio {
            Some(spec) if settings.audio.enabled => {
                Some(AudioCapture::attach(&pipeline, &webrtc, spec.clone(), settings, events.clone())?)
            }
            _ => None,
        };

        let lease = match &cfg.capture {
            CaptureSourceSpec::PipeWire { lease, .. } => Some(lease.clone()),
            _ => None,
        };
        let media = Arc::new(HostMedia {
            pipeline: pipeline.clone(),
            peer: peer.clone(),
            encoder,
            backend,
            timing,
            width,
            height,
            bitrate_kbps: AtomicU32::new(bitrate),
            stopping: Arc::new(AtomicBool::new(false)),
            audio,
            _capture_lease: lease,
        });
        crate::watch_bus(&pipeline, media.stopping.clone(), events.clone());

        pipeline
            .set_state(gst::State::Playing)
            .map_err(|_| MediaError::Pipeline("host pipeline failed to start (see logs for the failing element)".into()))?;
        if let Some(a) = &media.audio {
            a.start();
        }
        peer.create_data_channels()?;
        peer.create_offer();
        Ok(media)
    }

    fn install_probes(
        timing: &Arc<HostTiming>,
        source_tail: &gst::Element,
        queue: &gst::Element,
        encoder: &gst::Element,
        es_filter: &gst::Element,
    ) {
        // Timestamps ride along as GstReferenceTimestampMeta, which video
        // encoders copy to their output (PTS cannot be used as a key: many
        // encoders offset it).
        let t = timing.clone();
        source_tail.static_pad("src").expect("src pad").add_probe(gst::PadProbeType::BUFFER, move |_, info| {
            if let Some(gst::PadProbeData::Buffer(buf)) = &mut info.data {
                add_ref_ts(buf.make_mut(), &CAPTURE_REF, now_us());
                t.capture_rate.lock().tick();
            }
            gst::PadProbeReturn::Ok
        });
        let t = timing.clone();
        queue.static_pad("sink").expect("sink pad").add_probe(gst::PadProbeType::BUFFER, move |_, _| {
            t.queue_in.fetch_add(1, Ordering::Relaxed);
            gst::PadProbeReturn::Ok
        });
        let t = timing.clone();
        queue.static_pad("src").expect("src pad").add_probe(gst::PadProbeType::BUFFER, move |_, _| {
            t.queue_out.fetch_add(1, Ordering::Relaxed);
            gst::PadProbeReturn::Ok
        });
        encoder.static_pad("sink").expect("sink pad").add_probe(gst::PadProbeType::BUFFER, move |_, info| {
            if let Some(gst::PadProbeData::Buffer(buf)) = &mut info.data {
                add_ref_ts(buf.make_mut(), &ENCODE_REF, now_us());
            }
            gst::PadProbeReturn::Ok
        });
        // After parsing: measure encode time and embed the capture timestamp.
        let t = timing.clone();
        es_filter.static_pad("src").expect("src pad").add_probe(gst::PadProbeType::BUFFER, move |_, info| {
            let Some(gst::PadProbeData::Buffer(buf)) = &mut info.data else { return gst::PadProbeReturn::Ok };
            if let Some(t_in) = ref_ts(buf, &ENCODE_REF) {
                t.encode_ms.lock().add(now_us().saturating_sub(t_in) as f64 / 1000.0);
            }
            let Some(captured) = ref_ts(buf, &CAPTURE_REF) else { return gst::PadProbeReturn::Ok };
            let Ok(map) = buf.map_readable() else { return gst::PadProbeReturn::Ok };
            let data = sei::insert_sei(map.as_slice(), &sei::build_sei_nal(captured));
            drop(map);
            let mut out = gst::Buffer::from_mut_slice(data);
            {
                let out_mut = out.get_mut().expect("new buffer is writable");
                out_mut.set_pts(buf.pts());
                out_mut.set_dts(buf.dts());
                out_mut.set_duration(buf.duration());
                out_mut.set_flags(buf.flags());
            }
            *buf = out;
            gst::PadProbeReturn::Ok
        });
    }

    pub fn set_bitrate(&self, kbps: u32) {
        let prev = self.bitrate_kbps.swap(kbps, Ordering::Relaxed);
        if prev != kbps {
            tracing::debug!(kbps, "encoder bitrate");
            self.backend.set_bitrate(&self.encoder, kbps);
        }
    }

    pub fn bitrate_kbps(&self) -> u32 {
        self.bitrate_kbps.load(Ordering::Relaxed)
    }

    pub fn force_keyframe(&self) {
        let ev = gst_video::UpstreamForceKeyUnitEvent::builder().all_headers(true).build();
        if let Some(pad) = self.encoder.static_pad("src") {
            pad.send_event(ev);
        }
    }

    pub fn host_stats(&self) -> HostStats {
        let q_in = self.timing.queue_in.load(Ordering::Relaxed);
        let q_out = self.timing.queue_out.load(Ordering::Relaxed);
        HostStats {
            encoder: self.backend.element_name().to_string(),
            width: self.width,
            height: self.height,
            target_bitrate_kbps: self.bitrate_kbps(),
            capture_fps: self.timing.capture_rate.lock().rate() as f32,
            encode_ms_avg: self.timing.encode_ms.lock().get().unwrap_or(0.0) as f32,
            frames_dropped_before_encode: q_in.saturating_sub(q_out).saturating_sub(1),
        }
    }

    pub fn encoder_hardware(&self) -> bool {
        self.backend.hardware()
    }

    pub fn stop(&self) {
        if self.stopping.swap(true, Ordering::SeqCst) {
            return;
        }
        if let Some(a) = &self.audio {
            a.stop();
        }
        self.peer.close_channels();
        let _ = self.pipeline.set_state(gst::State::Null);
        tracing::info!("host media stopped");
    }
}

impl Drop for HostMedia {
    fn drop(&mut self) {
        self.stop();
    }
}

impl MediaTransport for HostMedia {
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

/// Audio capture in its own pipeline, pushed into the session's `appsrc`.
struct AudioCapture {
    spec: AudioSourceSpec,
    appsrc: gst_app::AppSrc,
    capture: Arc<Mutex<Option<gst::Pipeline>>>,
    stopping: Arc<AtomicBool>,
    events: EventSender,
}

const AUDIO_CAPS: &str = "audio/x-raw,format=S16LE,layout=interleaved,rate=48000,channels=2";

impl AudioCapture {
    fn attach(
        pipeline: &gst::Pipeline,
        webrtc: &gst::Element,
        spec: AudioSourceSpec,
        settings: &StreamSettings,
        events: EventSender,
    ) -> Result<Self> {
        let caps: gst::Caps = AUDIO_CAPS.parse().expect("static caps parse");
        let appsrc = gst_app::AppSrc::builder()
            .name("audio-in")
            .caps(&caps)
            .is_live(true)
            .format(gst::Format::Time)
            .do_timestamp(true)
            .max_bytes(48_000 * 4 / 5) // ~200 ms
            .leaky_type(gst_app::AppLeakyType::Upstream)
            .build();
        let queue = make("queue")?;
        queue.set_property_from_str("leaky", "downstream");
        queue.set_property("max-size-time", 100_000_000u64);
        queue.set_property("max-size-buffers", 0u32);
        queue.set_property("max-size-bytes", 0u32);
        let convert = make("audioconvert")?;
        let resample = make("audioresample")?;
        let channels = make("capsfilter")?;
        channels.set_property(
            "caps",
            gst::Caps::builder("audio/x-raw").field("channels", settings.audio.channels as i32).build(),
        );
        let opus = make("opusenc")?;
        opus.set_property("bitrate", (settings.audio.bitrate_kbps * 1000) as i32);
        try_set(&opus, "frame-size", "10");
        try_set(&opus, "audio-type", "restricted-lowdelay");
        try_set(&opus, "inband-fec", "true");
        let pay = make("rtpopuspay")?;
        pay.set_property("pt", AUDIO_PT);
        let rtp_filter = make("capsfilter")?;
        rtp_filter.set_property(
            "caps",
            gst::Caps::builder("application/x-rtp")
                .field("media", "audio")
                .field("encoding-name", "OPUS")
                .field("payload", AUDIO_PT as i32)
                .field("clock-rate", 48000i32)
                .build(),
        );
        let els: [&gst::Element; 8] =
            [appsrc.upcast_ref(), &queue, &convert, &resample, &channels, &opus, &pay, &rtp_filter];
        pipeline.add_many(els).map_err(MediaError::from_bool)?;
        gst::Element::link_many(els).map_err(|e| MediaError::Pipeline(format!("linking audio chain: {e}")))?;
        rtp_filter.link(webrtc).map_err(|e| MediaError::Pipeline(format!("linking audio to webrtcbin: {e}")))?;
        Ok(AudioCapture {
            spec,
            appsrc,
            capture: Arc::new(Mutex::new(None)),
            stopping: Arc::new(AtomicBool::new(false)),
            events,
        })
    }

    fn build_capture(spec: &AudioSourceSpec, appsrc: gst_app::AppSrc) -> Result<gst::Pipeline> {
        let p = gst::Pipeline::with_name("mist-audio-capture");
        let src = match spec {
            AudioSourceSpec::PulseMonitor { device } => {
                let s = make("pulsesrc")?;
                // Never fall back to the default *input* (microphone).
                s.set_property("device", device.as_deref().unwrap_or("@DEFAULT_MONITOR@"));
                try_set(&s, "client-name", "Mist Remote Play");
                try_set(&s, "buffer-time", "40000");
                try_set(&s, "latency-time", "10000");
                s
            }
            AudioSourceSpec::WasapiLoopback { device } => {
                let s = make("wasapi2src")?;
                s.set_property("loopback", true);
                try_set(&s, "low-latency", "true");
                if let Some(d) = device {
                    s.set_property("device", d);
                }
                s
            }
            AudioSourceSpec::TestTone { frequency_hz } => {
                let s = make("audiotestsrc")?;
                s.set_property("is-live", true);
                s.set_property("freq", *frequency_hz as f64);
                s.set_property("volume", 0.2f64);
                s
            }
        };
        let convert = make("audioconvert")?;
        let resample = make("audioresample")?;
        let caps = make("capsfilter")?;
        caps.set_property("caps", AUDIO_CAPS.parse::<gst::Caps>().expect("static caps parse"));
        let sink = gst_app::AppSink::builder().sync(false).max_buffers(20).drop(true).build();
        sink.set_callbacks(
            gst_app::AppSinkCallbacks::builder()
                .new_sample(move |s| {
                    let sample = s.pull_sample().map_err(|_| gst::FlowError::Eos)?;
                    if let Some(buf) = sample.buffer_owned() {
                        let mut buf = buf;
                        {
                            let b = buf.make_mut();
                            b.set_pts(gst::ClockTime::NONE);
                            b.set_dts(gst::ClockTime::NONE);
                        }
                        // Downstream flushing/stopping is not an error for capture.
                        let _ = appsrc.push_buffer(buf);
                    }
                    Ok(gst::FlowSuccess::Ok)
                })
                .build(),
        );
        let els: [&gst::Element; 5] = [&src, &convert, &resample, &caps, sink.upcast_ref()];
        p.add_many(els).map_err(MediaError::from_bool)?;
        gst::Element::link_many(els).map_err(|e| MediaError::Pipeline(format!("linking audio capture: {e}")))?;
        Ok(p)
    }

    /// Start capture; restarts with backoff when the device fails.
    fn start(&self) {
        let spec = self.spec.clone();
        let appsrc = self.appsrc.clone();
        let slot = self.capture.clone();
        let stopping = self.stopping.clone();
        let events = self.events.clone();
        std::thread::Builder::new()
            .name("audio-capture".into())
            .spawn(move || {
                let mut backoff = Duration::from_millis(500);
                while !stopping.load(Ordering::SeqCst) {
                    let result = Self::build_capture(&spec, appsrc.clone()).and_then(|p| {
                        p.set_state(gst::State::Playing)
                            .map_err(|_| MediaError::Pipeline("audio capture failed to start".into()))?;
                        Ok(p)
                    });
                    let p = match result {
                        Ok(p) => p,
                        Err(e) => {
                            let _ = events.send(TransportEvent::Warning(format!("system audio unavailable: {e}")));
                            std::thread::sleep(backoff);
                            backoff = (backoff * 2).min(Duration::from_secs(10));
                            continue;
                        }
                    };
                    *slot.lock() = Some(p.clone());
                    let bus = p.bus().expect("pipeline has a bus");
                    let mut failed = None;
                    while !stopping.load(Ordering::SeqCst) {
                        if let Some(msg) = bus.timed_pop_filtered(
                            gst::ClockTime::from_mseconds(200),
                            &[gst::MessageType::Error, gst::MessageType::Eos],
                        ) {
                            failed = Some(match msg.view() {
                                gst::MessageView::Error(e) => e.error().to_string(),
                                _ => "audio device stream ended".to_string(),
                            });
                            break;
                        }
                        backoff = Duration::from_millis(500);
                    }
                    let _ = p.set_state(gst::State::Null);
                    *slot.lock() = None;
                    if let Some(err) = failed {
                        tracing::warn!("audio capture failed: {err}; restarting");
                        let _ = events.send(TransportEvent::Warning(format!("audio device error: {err}; retrying")));
                        std::thread::sleep(backoff);
                        backoff = (backoff * 2).min(Duration::from_secs(10));
                    }
                }
            })
            .expect("spawn audio capture thread");
    }

    fn stop(&self) {
        self.stopping.store(true, Ordering::SeqCst);
        if let Some(p) = self.capture.lock().take() {
            let _ = p.set_state(gst::State::Null);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fit_within;

    #[test]
    fn fit_preserves_aspect_and_never_upscales() {
        assert_eq!(fit_within((1920, 1080), (1280, 720)), (1280, 720));
        assert_eq!(fit_within((2560, 1080), (1920, 1080)), (1920, 810));
        assert_eq!(fit_within((1280, 720), (1920, 1080)), (1280, 720));
        assert_eq!(fit_within((1366, 768), (1920, 1080)), (1366, 768));
    }
}
