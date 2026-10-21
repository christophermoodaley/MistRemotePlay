//! Host media: one shared capture -> scale/convert -> H.264 encode, plus
//! system audio -> Opus, fanned out through `tee`s to one WebRTC peer per
//! viewer (RTP payloader -> `webrtcbin`).
//!
//! Every viewer receives the same encoded stream, so capture and encoding
//! cost the same for one viewer or eight. The encoder runs at the lowest
//! bitrate any viewer's congestion control asks for. Viewers are attached
//! to the running pipeline with [`HostStream::add_viewer`] and detached by
//! stopping (or dropping) the [`HostViewer`].
//!
//! Backpressure: a leaky single-buffer queue sits between capture and the
//! encoder, so when encoding falls behind the oldest frame is discarded
//! instead of accumulating latency. The audio capture device runs in its
//! own pipeline (feeding an `appsrc`) so device loss or default-device
//! changes restart audio without tearing down video.
//!
//! Screen capture on Windows stops working while the host shows a secure
//! desktop (lock screen, a password-protected screen saver, UAC prompts).
//! The capture source is then restarted in place until the desktop is
//! back, instead of ending everyone's session; viewers see the last frame
//! meanwhile.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{mpsc, Arc};
use std::time::Duration;

use crate::audio::AudioSourceSpec;
use crate::capture::CaptureSourceSpec;
use crate::protocol::control::HostStats;
use crate::protocol::peer::SdpKind;
use crate::protocol::settings::StreamSettings;
use gst::prelude::*;
use parking_lot::Mutex;
use tokio::sync::oneshot;

use crate::media::codecs::{self, try_set, EncoderBackend, EncoderParams, GstEncoder, InputMemory};
use crate::media::stats::TransportStats;
use crate::media::timing::{add_ref_ts, now_us, ref_ts, Ewma, RateCounter, CAPTURE_REF, ENCODE_REF};
use crate::media::transport::{DataChannelKind, EventSender, IceConfig, MediaTransport, TransportEvent};
use crate::media::webrtc::WebRtcPeer;
use crate::media::{make, sei, MediaError, Result};

pub const VIDEO_PT: u32 = 96;
pub const AUDIO_PT: u32 = 111;

pub struct HostStreamConfig {
    pub capture: CaptureSourceSpec,
    pub audio: Option<AudioSourceSpec>,
    /// Resolution, frame rate and bitrate limits of the shared stream.
    pub settings: StreamSettings,
}

#[derive(Debug)]
struct HostTiming {
    encode_ms: Mutex<Ewma>,
    /// Frame sampled by the source -> handed downstream (screen grab).
    grab_ms: Mutex<Ewma>,
    capture_rate: Mutex<RateCounter>,
    queue_in: AtomicU64,
    queue_out: AtomicU64,
    /// Capture restarts since the last captured frame.
    capture_failures: AtomicU32,
}

impl Default for HostTiming {
    fn default() -> Self {
        HostTiming {
            encode_ms: Mutex::new(Ewma::new(0.05)),
            grab_ms: Mutex::new(Ewma::new(0.05)),
            capture_rate: Mutex::new(RateCounter::new(Duration::from_secs(2))),
            queue_in: AtomicU64::new(0),
            queue_out: AtomicU64::new(0),
            capture_failures: AtomicU32::new(0),
        }
    }
}

/// The shared capture and encoders. Viewers hang off its `tee`s.
pub struct HostStream {
    pipeline: gst::Pipeline,
    encoder: gst::Element,
    backend: &'static GstEncoder,
    timing: Arc<HostTiming>,
    settings: StreamSettings,
    width: u32,
    height: u32,
    video_tee: gst::Element,
    audio_tee: Option<gst::Element>,
    bitrate_kbps: AtomicU32,
    /// Bitrate each viewer's congestion control last asked for.
    viewer_bitrates: Mutex<HashMap<u64, u32>>,
    /// Where errors raised inside a viewer's elements are reported.
    viewer_events: Mutex<HashMap<u64, EventSender>>,
    next_viewer: AtomicU64,
    stopping: Arc<AtomicBool>,
    audio: Option<AudioCapture>,
    _capture_lease: Option<Arc<dyn crate::capture::CaptureLease>>,
    /// The screen capture element, when it can be restarted after errors.
    capture_src: Option<gst::Element>,
    capture_restarting: AtomicBool,
    /// Holds off the screen saver and sleep while streaming.
    _awake: Option<crate::power::KeepAwake>,
}

/// How long the capture source keeps being restarted without producing a
/// frame (lock screen, secure desktop) before the stream gives up.
const CAPTURE_RETRY_LIMIT: u32 = 600;
const CAPTURE_RETRY_DELAY: Duration = Duration::from_secs(1);

/// One viewer's peer connection: RTP payloaders and a `webrtcbin` in their
/// own bin, fed from the stream's `tee`s.
pub struct HostViewer {
    id: u64,
    stream: Arc<HostStream>,
    peer: Arc<WebRtcPeer>,
    bin: gst::Bin,
    /// (tee, its request pad feeding this viewer)
    tee_pads: Vec<(gst::Element, gst::Pad)>,
    detached: AtomicBool,
}

/// How long detaching a viewer waits for its tee pads to go idle.
const DETACH_WAIT: Duration = Duration::from_secs(1);

/// Name of a viewer's bin; bus messages are routed by it.
fn viewer_bin_name(id: u64) -> String {
    format!("viewer-{id}")
}

fn fit_within(src: (u32, u32), max: (u32, u32)) -> (u32, u32) {
    let scale = f64::min(max.0 as f64 / src.0 as f64, max.1 as f64 / src.1 as f64).min(1.0);
    let w = ((src.0 as f64 * scale).round() as u32).max(2) & !1;
    let h = ((src.1 as f64 * scale).round() as u32).max(2) & !1;
    (w, h)
}

/// Elements producing raw video for the given capture source.
/// Raw video in Direct3D 11 textures (`d3d11`) or in system memory. The
/// memory feature must be set on the caps: a builder named
/// "video/x-raw(memory:D3D11Memory)" would create a media type of that
/// literal name, which links to nothing.
fn raw_video_caps(d3d11: bool) -> gst::caps::Builder<gst::caps::HasFeatures> {
    let memory = if d3d11 { "memory:D3D11Memory" } else { "memory:SystemMemory" };
    gst::Caps::builder("video/x-raw").features([memory])
}

fn video_source(spec: &CaptureSourceSpec, fps: u32, gpu_d3d11: bool) -> Result<Vec<gst::Element>> {
    let rate = gst::Fraction::new(fps as i32, 1);
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
            caps.set_property("caps", gst::Caps::builder("video/x-raw").field("framerate", rate).build());
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
            caps.set_property("caps", raw_video_caps(true).field("framerate", rate).build());
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
                    .field("framerate", rate)
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

/// System-clock time (µs, same base as [`now_us`]) at which `element`
/// sampled a buffer with running time `pts`. Only valid when the pipeline
/// runs on the monotonic system clock (true for the host pipeline: its
/// audio is fed through an `appsrc`).
fn sampled_at_us(element: &gst::Element, pts: Option<gst::ClockTime>) -> Option<u64> {
    let clock = element.clock()?;
    if !clock.is::<gst::SystemClock>() {
        return None;
    }
    let base = element.base_time()?;
    Some((base + pts?).useconds())
}

impl HostStream {
    /// Start capturing and encoding. `events` receives errors of the shared
    /// pipeline (which end every viewer); errors inside a viewer's elements
    /// go to that viewer's own events.
    pub fn start(cfg: HostStreamConfig, events: EventSender) -> Result<Arc<HostStream>> {
        crate::media::init()?;
        let settings = &cfg.settings;
        let backend = codecs::select_encoder(settings.encoder.as_deref())?;
        if let Some(p) = settings.encoder.as_deref().filter(|p| *p != backend.element_name()) {
            let why = if p.starts_with("nv") && !codecs::nvenc_usable() {
                "NVENC stops working in a running Mist once Media Foundation has been used; restart Mist to use it"
            } else {
                "it is not available on this PC"
            };
            let _ = events.send(TransportEvent::Warning(format!("using {} instead of {p}: {why}", backend.label())));
        }
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
            "starting host stream"
        );

        let pipeline = gst::Pipeline::with_name("mist-host");

        // ---- video branch ----
        let timing = Arc::new(HostTiming::default());
        let mut chain = video_source(&cfg.capture, settings.fps, gpu_d3d11)?;
        let source_tail = chain.last().cloned().expect("source chain is never empty");
        let capture_src = matches!(cfg.capture, CaptureSourceSpec::D3D11 { .. }).then(|| chain[0].clone());
        if let Some(src) = &capture_src {
            // A failing source sends EOS, which would end the encoder and
            // every viewer for good; it is restarted instead (see below).
            src.static_pad("src").expect("src pad").add_probe(
                gst::PadProbeType::EVENT_DOWNSTREAM,
                |_, info| match &info.data {
                    Some(gst::PadProbeData::Event(e)) if e.type_() == gst::EventType::Eos => gst::PadProbeReturn::Drop,
                    _ => gst::PadProbeReturn::Ok,
                },
            );
        }
        let queue = make("queue")?;
        queue.set_property_from_str("leaky", "downstream");
        queue.set_property("max-size-buffers", 1u32);
        queue.set_property("max-size-bytes", 0u32);
        queue.set_property("max-size-time", 0u64);
        chain.push(queue.clone());
        let raw_caps = if gpu_d3d11 {
            chain.push(make("d3d11convert")?);
            raw_video_caps(true)
        } else {
            let convert = make("videoconvertscale")?;
            try_set(&convert, "add-borders", "true");
            try_set(
                &convert,
                "n-threads",
                &std::thread::available_parallelism().map(|n| n.get().min(8)).unwrap_or(2).to_string(),
            );
            chain.push(convert);
            raw_video_caps(false)
        }
        .field("format", backend.input_format())
        .field("width", width as i32)
        .field("height", height as i32)
        .field("pixel-aspect-ratio", gst::Fraction::new(1, 1))
        .build();
        let raw_filter = make("capsfilter")?;
        raw_filter.set_property("caps", raw_caps);
        chain.push(raw_filter);
        let params = EncoderParams {
            width,
            height,
            fps: settings.fps,
            bitrate_kbps: bitrate,
            quality: settings.encoder_quality,
        };
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
        // Viewers come and go: frames keep flowing with nobody attached.
        let video_tee = make("tee")?;
        video_tee.set_property("allow-not-linked", true);
        chain.push(video_tee.clone());
        pipeline.add_many(&chain).map_err(MediaError::from_bool)?;
        gst::Element::link_many(&chain).map_err(|e| MediaError::Pipeline(format!("linking video chain: {e}")))?;

        Self::install_probes(&timing, &source_tail, &queue, &encoder, &es_filter);

        // ---- audio branch ----
        let (audio, audio_tee) = match &cfg.audio {
            Some(spec) if settings.audio.enabled => {
                let (a, tee) = AudioCapture::attach(&pipeline, spec.clone(), settings, events.clone())?;
                (Some(a), Some(tee))
            }
            _ => (None, None),
        };

        let lease = match &cfg.capture {
            CaptureSourceSpec::PipeWire { lease, .. } => Some(lease.clone()),
            _ => None,
        };
        let stream = Arc::new(HostStream {
            pipeline: pipeline.clone(),
            encoder,
            backend,
            timing,
            settings: StreamSettings { width, height, ..settings.clone() },
            width,
            height,
            video_tee,
            audio_tee,
            bitrate_kbps: AtomicU32::new(bitrate),
            viewer_bitrates: Mutex::new(HashMap::new()),
            viewer_events: Mutex::new(HashMap::new()),
            next_viewer: AtomicU64::new(1),
            stopping: Arc::new(AtomicBool::new(false)),
            audio,
            _capture_lease: lease,
            capture_src,
            capture_restarting: AtomicBool::new(false),
            _awake: crate::power::KeepAwake::new("Mist Remote Play: someone is playing on this PC"),
        });
        stream.watch_bus(events);

        pipeline.set_state(gst::State::Playing).map_err(|_| {
            MediaError::Pipeline("host pipeline failed to start (see logs for the failing element)".into())
        })?;
        if let Some(a) = &stream.audio {
            a.start();
        }
        Ok(stream)
    }

    /// Like [`crate::media::watch_bus`], but an error from inside a viewer's bin
    /// ends only that viewer; anything else ends the stream.
    fn watch_bus(self: &Arc<Self>, events: EventSender) {
        let bus = self.pipeline.bus().expect("pipeline has a bus");
        let stopping = self.stopping.clone();
        let weak = Arc::downgrade(self);
        std::thread::Builder::new()
            .name("mist-host-bus".into())
            .spawn(move || {
                while !stopping.load(Ordering::SeqCst) {
                    let Some(msg) = bus.timed_pop(gst::ClockTime::from_mseconds(200)) else { continue };
                    match msg.view() {
                        gst::MessageView::Error(e) => {
                            let src = e.src().map(|s| s.path_string().to_string()).unwrap_or_default();
                            let text = format!("{} ({src})", e.error());
                            tracing::debug!(debug = ?e.debug(), "pipeline error from {src}");
                            if let Some(s) = weak.upgrade() {
                                let from_capture =
                                    s.capture_src.as_ref().is_some_and(|c| e.src() == Some(c.upcast_ref()));
                                if from_capture && s.restart_capture(&text, &events) {
                                    continue;
                                }
                            }
                            let viewer = weak.upgrade().and_then(|s| {
                                s.viewer_events
                                    .lock()
                                    .iter()
                                    .find(|(id, _)| src.contains(&format!("/{}/", viewer_bin_name(**id))))
                                    .map(|(_, tx)| tx.clone())
                            });
                            match viewer {
                                Some(tx) => {
                                    let _ = tx.send(TransportEvent::Error(text));
                                }
                                None => {
                                    let _ = events.send(TransportEvent::Error(text));
                                    break;
                                }
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

    /// Restart the screen capture source after it failed (secure desktop,
    /// display mode change, GPU reset). Returns `false` once it has failed
    /// for too long, so the error ends the stream.
    fn restart_capture(self: &Arc<Self>, why: &str, events: &EventSender) -> bool {
        let Some(src) = self.capture_src.clone() else { return false };
        let failures = self.timing.capture_failures.fetch_add(1, Ordering::Relaxed);
        if failures >= CAPTURE_RETRY_LIMIT {
            return false;
        }
        if failures == 0 {
            tracing::warn!("screen capture stopped ({why}); restarting it");
            let _ = events.send(TransportEvent::Warning(
                "the screen can't be captured right now (locked, or a secure screen is showing); waiting for it".into(),
            ));
        }
        if self.capture_restarting.swap(true, Ordering::SeqCst) {
            return true;
        }
        let weak = Arc::downgrade(self);
        std::thread::Builder::new()
            .name("mist-capture-restart".into())
            .spawn(move || {
                std::thread::sleep(CAPTURE_RETRY_DELAY);
                let Some(s) = weak.upgrade() else { return };
                if !s.stopping.load(Ordering::SeqCst) {
                    let _ = src.set_state(gst::State::Null);
                    if src.sync_state_with_parent().is_err() {
                        tracing::debug!("capture source did not restart yet");
                    }
                    s.force_keyframe();
                }
                s.capture_restarting.store(false, Ordering::SeqCst);
            })
            .map(|_| true)
            .unwrap_or(false)
    }

    /// Attach a new viewer: payloaders and a `webrtcbin` fed from the tees.
    /// The offer and ICE candidates arrive on `events`; the next frame is a
    /// keyframe so the viewer's picture appears right away.
    pub fn add_viewer(self: &Arc<Self>, ice: &IceConfig, events: EventSender) -> Result<Arc<HostViewer>> {
        if self.stopping.load(Ordering::SeqCst) {
            return Err(MediaError::Pipeline("the host stream has stopped".into()));
        }
        let id = self.next_viewer.fetch_add(1, Ordering::Relaxed);
        let bin = gst::Bin::with_name(&viewer_bin_name(id));
        let peer = WebRtcPeer::new(&format!("webrtc-{id}"), ice, None, events.clone())?;
        let webrtc = peer.element().clone();
        webrtc.connect("on-new-transceiver", false, |values| {
            if let Ok(t) = values[1].get::<gst_webrtc::WebRTCRTPTransceiver>() {
                t.set_property("direction", gst_webrtc::WebRTCRTPTransceiverDirection::Sendonly);
                t.set_property("do-nack", true);
            }
            None
        });
        bin.add(&webrtc).map_err(MediaError::from_bool)?;

        let video_in = Self::viewer_branch(&bin, &webrtc, "video", video_payloader()?, video_rtp_caps())?;
        let audio_in = match &self.audio_tee {
            Some(_) => Some(Self::viewer_branch(&bin, &webrtc, "audio", audio_payloader()?, audio_rtp_caps())?),
            None => None,
        };

        self.viewer_events.lock().insert(id, events);
        // From here on, dropping `viewer` on error detaches whatever was attached.
        let mut viewer = HostViewer {
            id,
            stream: self.clone(),
            peer: peer.clone(),
            bin: bin.clone(),
            tee_pads: Vec::new(),
            detached: AtomicBool::new(false),
        };
        self.pipeline.add(&bin).map_err(MediaError::from_bool)?;
        for (tee, sink) in std::iter::once((&self.video_tee, video_in)).chain(self.audio_tee.iter().zip(audio_in)) {
            let pad = tee
                .request_pad_simple("src_%u")
                .ok_or_else(|| MediaError::Pipeline("tee refused another viewer".into()))?;
            viewer.tee_pads.push((tee.clone(), pad.clone()));
            pad.link(&sink).map_err(|e| MediaError::Pipeline(format!("linking viewer {id}: {e:?}")))?;
        }
        bin.sync_state_with_parent().map_err(MediaError::from_bool)?;
        peer.create_data_channels()?;
        peer.create_offer();
        self.force_keyframe();
        tracing::info!(viewer = id, viewers = self.viewer_events.lock().len(), "viewer attached");
        Ok(Arc::new(viewer))
    }

    /// queue -> payloader -> RTP caps -> webrtcbin inside `bin`; returns the
    /// ghost sink pad the tee links to.
    fn viewer_branch(
        bin: &gst::Bin,
        webrtc: &gst::Element,
        name: &str,
        pay: gst::Element,
        caps: gst::Caps,
    ) -> Result<gst::Pad> {
        // A viewer whose network stalls drops its own oldest data instead of
        // blocking the tee, and with it everyone else.
        let queue = make("queue")?;
        queue.set_property_from_str("leaky", "downstream");
        queue.set_property("max-size-buffers", 0u32);
        queue.set_property("max-size-bytes", 0u32);
        queue.set_property("max-size-time", 500_000_000u64);
        let filter = make("capsfilter")?;
        filter.set_property("caps", caps);
        bin.add_many([&queue, &pay, &filter]).map_err(MediaError::from_bool)?;
        gst::Element::link_many([&queue, &pay, &filter])
            .map_err(|e| MediaError::Pipeline(format!("linking viewer {name} branch: {e}")))?;
        filter.link(webrtc).map_err(|e| MediaError::Pipeline(format!("linking {name} to webrtcbin: {e}")))?;
        let target = queue.static_pad("sink").expect("queue has a sink pad");
        let ghost = gst::GhostPad::builder_with_target(&target).map_err(MediaError::from_bool)?.name(name).build();
        bin.add_pad(&ghost).map_err(MediaError::from_bool)?;
        Ok(ghost.upcast())
    }

    fn set_viewer_bitrate(&self, id: u64, kbps: u32) {
        self.viewer_bitrates.lock().insert(id, kbps);
        self.apply_bitrate();
    }

    /// Run the encoder at the lowest bitrate any viewer can take.
    fn apply_bitrate(&self) {
        let Some(kbps) = self.viewer_bitrates.lock().values().copied().min() else { return };
        let prev = self.bitrate_kbps.swap(kbps, Ordering::Relaxed);
        if prev != kbps {
            tracing::debug!(kbps, "encoder bitrate");
            self.backend.set_bitrate(&self.encoder, kbps);
        }
    }

    fn remove_viewer(&self, id: u64) {
        self.viewer_events.lock().remove(&id);
        self.viewer_bitrates.lock().remove(&id);
        self.apply_bitrate();
    }

    /// Settings of the shared stream (width/height are what is encoded).
    pub fn settings(&self) -> &StreamSettings {
        &self.settings
    }

    pub fn viewer_count(&self) -> usize {
        self.viewer_events.lock().len()
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
        //
        // The capture time is the instant the source sampled the frame (its
        // PTS on the pipeline clock), so the screen grab itself counts
        // towards latency; it falls back to "now" when that is unavailable.
        let t = timing.clone();
        let src = source_tail.clone();
        source_tail.static_pad("src").expect("src pad").add_probe(gst::PadProbeType::BUFFER, move |_, info| {
            if let Some(gst::PadProbeData::Buffer(buf)) = &mut info.data {
                let now = now_us();
                let sampled = sampled_at_us(&src, buf.pts()).filter(|&s| s <= now && now - s < 250_000);
                if let Some(s) = sampled {
                    t.grab_ms.lock().add((now - s) as f64 / 1000.0);
                }
                add_ref_ts(buf.make_mut(), &CAPTURE_REF, sampled.unwrap_or(now));
                t.capture_rate.lock().tick();
                if t.capture_failures.swap(0, Ordering::Relaxed) > 0 {
                    tracing::info!("screen capture is back");
                }
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
            let encoded = now_us();
            if let Some(t_in) = ref_ts(buf, &ENCODE_REF) {
                t.encode_ms.lock().add(encoded.saturating_sub(t_in) as f64 / 1000.0);
            }
            let Some(captured) = ref_ts(buf, &CAPTURE_REF) else { return gst::PadProbeReturn::Ok };
            let Ok(map) = buf.map_readable() else { return gst::PadProbeReturn::Ok };
            let times = sei::FrameTimes { capture_us: captured, encoded_us: Some(encoded) };
            let data = sei::insert_sei(map.as_slice(), &sei::build_sei_nal(times));
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
            capture_ms_avg: self.timing.grab_ms.lock().get().map(|v| v as f32),
            frames_dropped_before_encode: q_in.saturating_sub(q_out).saturating_sub(1),
        }
    }

    pub fn encoder_hardware(&self) -> bool {
        self.backend.hardware()
    }

    /// Stop capture and encoding, and with them every viewer still attached.
    pub fn stop(&self) {
        if self.stopping.swap(true, Ordering::SeqCst) {
            return;
        }
        if let Some(a) = &self.audio {
            a.stop();
        }
        let _ = self.pipeline.set_state(gst::State::Null);
        tracing::info!("host stream stopped");
    }
}

impl Drop for HostStream {
    fn drop(&mut self) {
        self.stop();
    }
}

fn video_payloader() -> Result<gst::Element> {
    let pay = make("rtph264pay")?;
    // SPS/PPS with every keyframe: viewers join mid-stream.
    pay.set_property("config-interval", -1i32);
    pay.set_property("pt", VIDEO_PT);
    pay.set_property("mtu", 1200u32);
    try_set(&pay, "aggregate-mode", "zero-latency");
    Ok(pay)
}

fn video_rtp_caps() -> gst::Caps {
    gst::Caps::builder("application/x-rtp")
        .field("media", "video")
        .field("encoding-name", "H264")
        .field("payload", VIDEO_PT as i32)
        .field("clock-rate", 90000i32)
        .build()
}

fn audio_payloader() -> Result<gst::Element> {
    let pay = make("rtpopuspay")?;
    pay.set_property("pt", AUDIO_PT);
    Ok(pay)
}

fn audio_rtp_caps() -> gst::Caps {
    gst::Caps::builder("application/x-rtp")
        .field("media", "audio")
        .field("encoding-name", "OPUS")
        .field("payload", AUDIO_PT as i32)
        .field("clock-rate", 48000i32)
        .build()
}

impl HostViewer {
    pub fn id(&self) -> u64 {
        self.id
    }

    /// This viewer's congestion-control target; the shared encoder follows
    /// the lowest target of all viewers.
    pub fn set_bitrate(&self, kbps: u32) {
        self.stream.set_viewer_bitrate(self.id, kbps);
    }

    pub fn bitrate_kbps(&self) -> u32 {
        self.stream.bitrate_kbps()
    }

    /// Keyframes go to every viewer (they share the encoder).
    pub fn force_keyframe(&self) {
        self.stream.force_keyframe();
    }

    pub fn host_stats(&self) -> HostStats {
        self.stream.host_stats()
    }

    pub fn encoder_hardware(&self) -> bool {
        self.stream.encoder_hardware()
    }

    /// Detach from the stream: unlink from the tees once no buffer is in
    /// flight on them, then shut the viewer's elements down. Blocks for up
    /// to [`DETACH_WAIT`]; call from a blocking context.
    pub fn stop(&self) {
        if self.detached.swap(true, Ordering::SeqCst) {
            return;
        }
        self.peer.close_channels();
        self.stream.remove_viewer(self.id);
        if self.stream.stopping.load(Ordering::SeqCst) {
            return; // the whole pipeline is being torn down
        }
        let (done_tx, done_rx) = mpsc::channel::<()>();
        for (tee, pad) in &self.tee_pads {
            let tee = tee.clone();
            let done = done_tx.clone();
            pad.add_probe(gst::PadProbeType::IDLE, move |pad, _| {
                if let Some(peer) = pad.peer() {
                    let _ = pad.unlink(&peer);
                }
                tee.release_request_pad(pad);
                let _ = done.send(());
                gst::PadProbeReturn::Remove
            });
        }
        drop(done_tx);
        for _ in 0..self.tee_pads.len() {
            if done_rx.recv_timeout(DETACH_WAIT).is_err() {
                tracing::warn!(viewer = self.id, "viewer branch did not go idle; detaching anyway");
                break;
            }
        }
        let _ = self.bin.set_state(gst::State::Null);
        let _ = self.stream.pipeline.remove(&self.bin);
        tracing::info!(viewer = self.id, viewers = self.stream.viewer_count(), "viewer detached");
    }
}

impl Drop for HostViewer {
    fn drop(&mut self) {
        self.stop();
    }
}

impl MediaTransport for HostViewer {
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
    /// Builds appsrc -> Opus -> tee in `pipeline`; returns the tee viewers
    /// attach to.
    fn attach(
        pipeline: &gst::Pipeline,
        spec: AudioSourceSpec,
        settings: &StreamSettings,
        events: EventSender,
    ) -> Result<(Self, gst::Element)> {
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
        let tee = make("tee")?;
        tee.set_property("allow-not-linked", true);
        let els: [&gst::Element; 7] = [appsrc.upcast_ref(), &queue, &convert, &resample, &channels, &opus, &tee];
        pipeline.add_many(els).map_err(MediaError::from_bool)?;
        gst::Element::link_many(els).map_err(|e| MediaError::Pipeline(format!("linking audio chain: {e}")))?;
        let capture = AudioCapture {
            spec,
            appsrc,
            capture: Arc::new(Mutex::new(None)),
            stopping: Arc::new(AtomicBool::new(false)),
            events,
        };
        Ok((capture, tee))
    }

    /// For app loopback: the root processes of the chosen apps that are
    /// running now (sorted; at most one when excluding, the Windows limit).
    /// `None` for other sources.
    fn app_targets(spec: &AudioSourceSpec) -> Option<Vec<u32>> {
        let AudioSourceSpec::AppLoopback { apps, exclude } = spec else { return None };
        #[cfg(windows)]
        {
            let procs = crate::audio::apps::processes().unwrap_or_default();
            let mut pids: Vec<u32> = apps.iter().flat_map(|a| crate::audio::apps::root_pids(&procs, a)).collect();
            pids.sort_unstable();
            pids.dedup();
            if *exclude {
                pids.truncate(1);
            }
            Some(pids)
        }
        #[cfg(not(windows))]
        {
            let _ = (apps, exclude);
            Some(Vec::new())
        }
    }

    /// Loopback of one process tree (`mode`: `include-process-tree` or
    /// `exclude-process-tree`).
    fn process_loopback(pid: u32, mode: &str) -> Result<gst::Element> {
        let s = make("wasapi2src")?;
        s.set_property("loopback", true);
        try_set(&s, "low-latency", "true");
        // Only offered where Windows supports process loopback.
        if s.find_property("loopback-mode").is_none() {
            return Err(MediaError::Pipeline(
                "sound from chosen apps needs Windows 11 (or Windows 10 build 20348 or newer)".into(),
            ));
        }
        s.set_property_from_str("loopback-mode", mode);
        s.set_property("loopback-target-pid", pid);
        Ok(s)
    }

    /// Adds the app-loopback sources to `p`; returns the element to link on.
    fn app_sources(p: &gst::Pipeline, pids: &[u32], exclude: bool) -> Result<gst::Element> {
        let single = |s: gst::Element| -> Result<gst::Element> {
            p.add(&s).map_err(MediaError::from_bool)?;
            Ok(s)
        };
        match (exclude, pids) {
            // The app isn't running: nothing to leave out.
            (true, []) => {
                let s = make("wasapi2src")?;
                s.set_property("loopback", true);
                try_set(&s, "low-latency", "true");
                single(s)
            }
            (true, [pid, ..]) => single(Self::process_loopback(*pid, "exclude-process-tree")?),
            // None of the apps is running: silence until one starts.
            (false, []) => {
                let s = make("audiotestsrc")?;
                s.set_property("is-live", true);
                s.set_property_from_str("wave", "silence");
                single(s)
            }
            (false, [pid]) => single(Self::process_loopback(*pid, "include-process-tree")?),
            (false, pids) => {
                let mixer = make("audiomixer")?;
                p.add(&mixer).map_err(MediaError::from_bool)?;
                for pid in pids {
                    let src = Self::process_loopback(*pid, "include-process-tree")?;
                    let convert = make("audioconvert")?;
                    let resample = make("audioresample")?;
                    let els = [&src, &convert, &resample];
                    p.add_many(els).map_err(MediaError::from_bool)?;
                    gst::Element::link_many([&src, &convert, &resample, &mixer])
                        .map_err(|e| MediaError::Pipeline(format!("linking app audio: {e}")))?;
                }
                Ok(mixer)
            }
        }
    }

    fn build_capture(
        spec: &AudioSourceSpec,
        targets: Option<&[u32]>,
        appsrc: gst_app::AppSrc,
    ) -> Result<gst::Pipeline> {
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
                p.add(&s).map_err(MediaError::from_bool)?;
                s
            }
            AudioSourceSpec::AppLoopback { exclude, .. } => Self::app_sources(&p, targets.unwrap_or(&[]), *exclude)?,
            AudioSourceSpec::TestTone { frequency_hz } => {
                let s = make("audiotestsrc")?;
                s.set_property("is-live", true);
                s.set_property("freq", *frequency_hz as f64);
                s.set_property("volume", 0.2f64);
                p.add(&s).map_err(MediaError::from_bool)?;
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
        p.add_many([&convert, &resample, &caps, sink.upcast_ref()]).map_err(MediaError::from_bool)?;
        gst::Element::link_many([&src, &convert, &resample, &caps, sink.upcast_ref()])
            .map_err(|e| MediaError::Pipeline(format!("linking audio capture: {e}")))?;
        Ok(p)
    }

    /// Start capture; restarts with backoff when the device fails, and
    /// right away when a chosen app starts or quits.
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
                let mut last_targets = None;
                // Opening process loopback fails now and then for no
                // lasting reason; such failures are retried quietly.
                let mut quiet_retries = 3;
                while !stopping.load(Ordering::SeqCst) {
                    let targets = Self::app_targets(&spec);
                    let changed = last_targets.as_ref() != Some(&targets);
                    last_targets = Some(targets.clone());
                    if let (true, AudioSourceSpec::AppLoopback { apps, exclude }, Some([])) =
                        (changed, &spec, targets.as_deref())
                    {
                        let names = apps.join(", ");
                        let _ = events.send(TransportEvent::Warning(if *exclude {
                            format!("{names} is not running, so all sound is streamed until it starts")
                        } else {
                            format!("{names}: not running; streaming silence until one starts")
                        }));
                    }
                    let result = Self::build_capture(&spec, targets.as_deref(), appsrc.clone()).and_then(|p| {
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
                    let mut ticks = 0u32;
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
                        // Every ~3 s: follow chosen apps that started, quit
                        // or restarted (new process IDs).
                        ticks += 1;
                        if targets.is_some() && ticks.is_multiple_of(15) && Self::app_targets(&spec) != targets {
                            tracing::info!("audio apps changed; restarting audio capture");
                            break;
                        }
                    }
                    let _ = p.set_state(gst::State::Null);
                    *slot.lock() = None;
                    if ticks >= 5 {
                        quiet_retries = 3;
                    }
                    if let Some(err) = failed {
                        if targets.is_some() && ticks <= 2 && quiet_retries > 0 {
                            quiet_retries -= 1;
                            tracing::info!("app audio capture did not open ({err}); trying again");
                            std::thread::sleep(Duration::from_millis(300));
                            continue;
                        }
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
    use super::{fit_within, raw_video_caps};
    use std::str::FromStr;

    /// Captures this PC's sound without one app (the first app with an
    /// audio session) for a second. Needs a sound device; run with
    /// `--ignored`.
    #[cfg(windows)]
    #[test]
    #[ignore]
    fn app_loopback_captures() {
        use super::{AudioCapture, AudioSourceSpec};
        use gst::prelude::*;
        crate::media::init().unwrap();
        let apps = crate::audio::apps::audio_apps().unwrap();
        println!("apps with sound: {apps:?}");
        for exclude in [true, false] {
            let names = apps.first().map(|a| vec![a.exe.clone()]).unwrap_or_default();
            let spec = AudioSourceSpec::AppLoopback { apps: names, exclude };
            let targets = AudioCapture::app_targets(&spec);
            println!("exclude={exclude} targets={targets:?}");
            // Like the capture thread: opening sometimes fails once.
            let mut last_error = None;
            for _ in 0..4 {
                let appsrc = gst_app::AppSrc::builder().build();
                let p = AudioCapture::build_capture(&spec, targets.as_deref(), appsrc).unwrap();
                p.set_state(gst::State::Playing).unwrap();
                last_error =
                    p.bus().unwrap().timed_pop_filtered(gst::ClockTime::from_seconds(1), &[gst::MessageType::Error]);
                let _ = p.set_state(gst::State::Null);
                if last_error.is_none() {
                    break;
                }
            }
            assert!(last_error.is_none(), "capture failed: {last_error:?}");
        }
    }

    #[test]
    fn raw_video_caps_match_what_sources_produce() {
        gst::init().unwrap();
        // The D3D11 screen capture source's output, as GStreamer writes it.
        let d3d11 =
            gst::Caps::from_str("video/x-raw(memory:D3D11Memory),format=BGRA,framerate=[0/1,2147483647/1]").unwrap();
        let system = gst::Caps::from_str("video/x-raw,format=BGRA,framerate=[0/1,2147483647/1]").unwrap();
        let rate = gst::Fraction::new(60, 1);
        let gpu = raw_video_caps(true).field("framerate", rate).build();
        let cpu = raw_video_caps(false).field("framerate", rate).build();
        assert!(gpu.can_intersect(&d3d11), "{gpu} vs {d3d11}");
        assert!(!gpu.can_intersect(&system));
        assert!(cpu.can_intersect(&system), "{cpu} vs {system}");
        assert!(!cpu.can_intersect(&d3d11));
    }

    #[test]
    fn fit_preserves_aspect_and_never_upscales() {
        assert_eq!(fit_within((1920, 1080), (1280, 720)), (1280, 720));
        assert_eq!(fit_within((2560, 1080), (1920, 1080)), (1920, 810));
        assert_eq!(fit_within((1280, 720), (1920, 1080)), (1280, 720));
        assert_eq!(fit_within((1366, 768), (1920, 1080)), (1366, 768));
    }
}
