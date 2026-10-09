//! H.264 encoder/decoder discovery.
//!
//! Each candidate is checked twice at runtime: the GStreamer element must be
//! registered, and a short functional test must succeed (hardware elements
//! are often registered even when no usable device/driver exists).

use std::sync::OnceLock;

use gst::glib;
use gst::prelude::*;
use mist_protocol::settings::EncoderQuality;
use serde::Serialize;

use crate::{MediaError, Result};

/// Memory the encoder prefers as input. System memory always works; the
/// GPU paths avoid a download/upload when capture is on the same device.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum InputMemory {
    System,
    Va,
    D3D11,
}

#[derive(Clone, Debug, Serialize)]
pub struct CodecInfo {
    pub element: String,
    pub label: String,
    pub hardware: bool,
    pub api: String,
    /// Element registered in GStreamer.
    pub present: bool,
    /// Functional probe succeeded.
    pub available: bool,
    pub error: Option<String>,
}

#[derive(Clone, Debug)]
pub struct EncoderParams {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub bitrate_kbps: u32,
    pub quality: EncoderQuality,
}

/// Interface implemented by every encoder candidate.
pub trait EncoderBackend: Send + Sync {
    fn element_name(&self) -> &'static str;
    fn label(&self) -> &'static str;
    fn hardware(&self) -> bool;
    fn api(&self) -> &'static str;
    fn input_memory(&self) -> InputMemory;
    /// Raw format the encoder is fed in system memory.
    fn input_format(&self) -> &'static str;
    fn create(&self, params: &EncoderParams) -> Result<gst::Element>;
    fn set_bitrate(&self, encoder: &gst::Element, kbps: u32);
}

/// Interface implemented by every decoder candidate.
pub trait DecoderBackend: Send + Sync {
    fn element_name(&self) -> &'static str;
    fn label(&self) -> &'static str;
    fn hardware(&self) -> bool;
    fn create(&self) -> Result<gst::Element>;
}

type Configure = fn(&gst::Element, &EncoderParams);

pub struct GstEncoder {
    element: &'static str,
    label: &'static str,
    hardware: bool,
    api: &'static str,
    memory: InputMemory,
    format: &'static str,
    bitrate_prop: &'static str,
    /// Multiplier from kbit/s to the element's bitrate unit.
    bitrate_scale: u32,
    configure: Configure,
}

/// Set a property only if the element (in this GStreamer version) has it.
pub(crate) fn try_set(el: &gst::Element, name: &str, value: &str) {
    let Some(pspec) = el.find_property(name) else {
        tracing::debug!(element = %el.name(), property = name, "property not supported by this GStreamer version");
        return;
    };
    match glib::Value::deserialize_with_pspec(value, &pspec) {
        Ok(v) => el.set_property_from_value(name, &v),
        Err(_) => tracing::warn!(element = %el.name(), property = name, value, "property value not accepted"),
    }
}

fn gop(p: &EncoderParams) -> String {
    // A keyframe every 2 s bounds recovery time after unrecoverable loss;
    // clients also request keyframes (PLI) on demand.
    (p.fps * 2).to_string()
}

fn cfg_x264(el: &gst::Element, p: &EncoderParams) {
    try_set(el, "tune", "zerolatency");
    try_set(
        el,
        "speed-preset",
        match p.quality {
            EncoderQuality::Fastest => "ultrafast",
            EncoderQuality::Balanced => "superfast",
            EncoderQuality::Quality => "veryfast",
        },
    );
    try_set(el, "key-int-max", &gop(p));
    try_set(el, "bframes", "0");
    // Small VBV keeps frame sizes even so a single frame does not burst
    // far beyond the link capacity.
    try_set(el, "vbv-buf-capacity", &(2000 / p.fps.max(1)).max(33).to_string());
    try_set(el, "rc-lookahead", "0");
}

fn cfg_openh264(el: &gst::Element, p: &EncoderParams) {
    try_set(el, "usage-type", "screen");
    try_set(el, "rate-control", "bitrate");
    try_set(el, "gop-size", &gop(p));
    try_set(
        el,
        "complexity",
        match p.quality {
            EncoderQuality::Fastest => "low",
            EncoderQuality::Balanced => "medium",
            EncoderQuality::Quality => "high",
        },
    );
}

fn cfg_nvenc(el: &gst::Element, p: &EncoderParams) {
    try_set(el, "rc-mode", "cbr");
    try_set(el, "gop-size", &gop(p));
    try_set(el, "bframes", "0");
    try_set(el, "zerolatency", "true");
    try_set(el, "preset", "low-latency-hq");
    try_set(el, "tune", "ultra-low-latency");
}

fn cfg_qsv(el: &gst::Element, p: &EncoderParams) {
    try_set(el, "rate-control", "cbr");
    try_set(el, "gop-size", &gop(p));
    try_set(el, "b-frames", "0");
    try_set(el, "low-latency", "true");
    try_set(el, "target-usage", if p.quality == EncoderQuality::Quality { "4" } else { "7" });
}

fn cfg_amf(el: &gst::Element, p: &EncoderParams) {
    try_set(el, "rate-control", "cbr");
    try_set(el, "usage", "ultra-low-latency");
    try_set(el, "gop-size", &gop(p));
    try_set(el, "b-frames", "0");
}

fn cfg_mf(el: &gst::Element, p: &EncoderParams) {
    try_set(el, "rc-mode", "cbr");
    try_set(el, "low-latency", "true");
    try_set(el, "gop-size", &gop(p));
    try_set(el, "bframes", "0");
}

fn cfg_va(el: &gst::Element, p: &EncoderParams) {
    try_set(el, "rate-control", "cbr");
    try_set(el, "key-int-max", &gop(p));
    try_set(el, "b-frames", "0");
    try_set(el, "target-usage", if p.quality == EncoderQuality::Quality { "4" } else { "7" });
}

fn cfg_vaapi(el: &gst::Element, p: &EncoderParams) {
    try_set(el, "rate-control", "cbr");
    try_set(el, "keyframe-period", &gop(p));
    try_set(el, "max-bframes", "0");
    try_set(el, "tune", "low-power");
}

// One row of the static encoder table; positional arguments keep the table compact.
#[allow(clippy::too_many_arguments)]
const fn enc(
    element: &'static str,
    label: &'static str,
    hardware: bool,
    api: &'static str,
    memory: InputMemory,
    format: &'static str,
    bitrate_scale: u32,
    configure: Configure,
) -> GstEncoder {
    GstEncoder { element, label, hardware, api, memory, format, bitrate_prop: "bitrate", bitrate_scale, configure }
}

/// Candidates in preference order: hardware first, software last.
pub static H264_ENCODERS: &[GstEncoder] = &[
    enc("nvh264enc", "NVIDIA NVENC (CUDA)", true, "nvenc", InputMemory::System, "NV12", 1, cfg_nvenc),
    enc("nvd3d11h264enc", "NVIDIA NVENC (D3D11)", true, "nvenc", InputMemory::D3D11, "NV12", 1, cfg_nvenc),
    enc("qsvh264enc", "Intel Quick Sync", true, "qsv", InputMemory::D3D11, "NV12", 1, cfg_qsv),
    enc("amfh264enc", "AMD AMF", true, "amf", InputMemory::D3D11, "NV12", 1, cfg_amf),
    enc("vah264lpenc", "VA-API (low power)", true, "va", InputMemory::Va, "NV12", 1, cfg_va),
    enc("vah264enc", "VA-API", true, "va", InputMemory::Va, "NV12", 1, cfg_va),
    enc("vaapih264enc", "VA-API (legacy gstreamer-vaapi)", true, "vaapi", InputMemory::System, "NV12", 1, cfg_vaapi),
    enc("mfh264enc", "Media Foundation", true, "mediafoundation", InputMemory::D3D11, "NV12", 1, cfg_mf),
    enc("x264enc", "x264 (software)", false, "software", InputMemory::System, "I420", 1, cfg_x264),
    enc("openh264enc", "OpenH264 (software)", false, "software", InputMemory::System, "I420", 1000, cfg_openh264),
];

impl EncoderBackend for GstEncoder {
    fn element_name(&self) -> &'static str {
        self.element
    }
    fn label(&self) -> &'static str {
        self.label
    }
    fn hardware(&self) -> bool {
        self.hardware
    }
    fn api(&self) -> &'static str {
        self.api
    }
    fn input_memory(&self) -> InputMemory {
        self.memory
    }
    fn input_format(&self) -> &'static str {
        self.format
    }

    fn create(&self, params: &EncoderParams) -> Result<gst::Element> {
        let el = gst::ElementFactory::make(self.element)
            .build()
            .map_err(|_| MediaError::MissingElement(self.element.into()))?;
        (self.configure)(&el, params);
        self.set_bitrate(&el, params.bitrate_kbps);
        Ok(el)
    }

    fn set_bitrate(&self, encoder: &gst::Element, kbps: u32) {
        try_set(encoder, self.bitrate_prop, &(kbps.saturating_mul(self.bitrate_scale)).to_string());
    }
}

pub struct GstDecoder {
    element: &'static str,
    label: &'static str,
    hardware: bool,
}

pub static H264_DECODERS: &[GstDecoder] = &[
    GstDecoder { element: "nvh264dec", label: "NVIDIA NVDEC", hardware: true },
    GstDecoder { element: "d3d11h264dec", label: "Direct3D 11 (DXVA)", hardware: true },
    GstDecoder { element: "d3d12h264dec", label: "Direct3D 12", hardware: true },
    GstDecoder { element: "vah264dec", label: "VA-API", hardware: true },
    GstDecoder { element: "vaapih264dec", label: "VA-API (legacy)", hardware: true },
    GstDecoder { element: "avdec_h264", label: "FFmpeg (software)", hardware: false },
    GstDecoder { element: "openh264dec", label: "OpenH264 (software)", hardware: false },
];

impl DecoderBackend for GstDecoder {
    fn element_name(&self) -> &'static str {
        self.element
    }
    fn label(&self) -> &'static str {
        self.label
    }
    fn hardware(&self) -> bool {
        self.hardware
    }
    fn create(&self) -> Result<gst::Element> {
        let el = gst::ElementFactory::make(self.element)
            .build()
            .map_err(|_| MediaError::MissingElement(self.element.into()))?;
        // Low-latency hints where supported.
        try_set(&el, "max-threads", "0");
        Ok(el)
    }
}

fn present(name: &str) -> bool {
    gst::ElementFactory::find(name).is_some()
}

/// Run a short pipeline to completion; returns an error string on failure.
fn run_probe(pipeline: &gst::Pipeline) -> std::result::Result<(), String> {
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
    res
}

fn probe_encoder(e: &GstEncoder) -> CodecInfo {
    let mut info = CodecInfo {
        element: e.element.into(),
        label: e.label.into(),
        hardware: e.hardware,
        api: e.api.into(),
        present: present(e.element),
        available: false,
        error: None,
    };
    if !info.present {
        info.error = Some("GStreamer element not installed".into());
        return info;
    }
    let result = (|| -> std::result::Result<(), String> {
        let pipeline = gst::Pipeline::new();
        let src = gst::ElementFactory::make("videotestsrc")
            .property("num-buffers", 5i32)
            .build()
            .map_err(|e| e.to_string())?;
        let conv = gst::ElementFactory::make("videoconvert").build().map_err(|e| e.to_string())?;
        let caps = gst::Caps::builder("video/x-raw")
            .field("format", e.format)
            .field("width", 640i32)
            .field("height", 360i32)
            .field("framerate", gst::Fraction::new(30, 1))
            .build();
        let filter =
            gst::ElementFactory::make("capsfilter").property("caps", &caps).build().map_err(|e| e.to_string())?;
        let params =
            EncoderParams { width: 640, height: 360, fps: 30, bitrate_kbps: 1000, quality: EncoderQuality::Fastest };
        let encoder = e.create(&params).map_err(|e| e.to_string())?;
        let sink = gst::ElementFactory::make("fakesink").build().map_err(|e| e.to_string())?;
        pipeline.add_many([&src, &conv, &filter, &encoder, &sink]).map_err(|e| e.to_string())?;
        gst::Element::link_many([&src, &conv, &filter, &encoder, &sink]).map_err(|e| e.to_string())?;
        run_probe(&pipeline)
    })();
    match result {
        Ok(()) => info.available = true,
        Err(err) => info.error = Some(err),
    }
    info
}

fn probe_decoder(d: &GstDecoder, test_encoder: Option<&GstEncoder>) -> CodecInfo {
    let mut info = CodecInfo {
        element: d.element.into(),
        label: d.label.into(),
        hardware: d.hardware,
        api: if d.hardware { "hardware".into() } else { "software".into() },
        present: present(d.element),
        available: false,
        error: None,
    };
    if !info.present {
        info.error = Some("GStreamer element not installed".into());
        return info;
    }
    let Some(enc) = test_encoder else {
        info.error = Some("no working encoder to generate a probe stream".into());
        return info;
    };
    let result = (|| -> std::result::Result<(), String> {
        let pipeline = gst::Pipeline::new();
        let src = gst::ElementFactory::make("videotestsrc")
            .property("num-buffers", 10i32)
            .build()
            .map_err(|e| e.to_string())?;
        let conv = gst::ElementFactory::make("videoconvert").build().map_err(|e| e.to_string())?;
        let caps = gst::Caps::builder("video/x-raw")
            .field("format", enc.format)
            .field("width", 320i32)
            .field("height", 240i32)
            .build();
        let filter =
            gst::ElementFactory::make("capsfilter").property("caps", &caps).build().map_err(|e| e.to_string())?;
        let params =
            EncoderParams { width: 320, height: 240, fps: 30, bitrate_kbps: 500, quality: EncoderQuality::Fastest };
        let encoder = enc.create(&params).map_err(|e| e.to_string())?;
        let parse = gst::ElementFactory::make("h264parse").build().map_err(|e| e.to_string())?;
        let decoder = d.create().map_err(|e| e.to_string())?;
        let sink = gst::ElementFactory::make("fakesink").build().map_err(|e| e.to_string())?;
        let els = [&src, &conv, &filter, &encoder, &parse, &decoder, &sink];
        pipeline.add_many(els).map_err(|e| e.to_string())?;
        gst::Element::link_many(els).map_err(|e| e.to_string())?;
        run_probe(&pipeline)
    })();
    match result {
        Ok(()) => info.available = true,
        Err(err) => info.error = Some(err),
    }
    info
}

static ENCODER_PROBE: OnceLock<Vec<CodecInfo>> = OnceLock::new();
static DECODER_PROBE: OnceLock<Vec<CodecInfo>> = OnceLock::new();

/// Probe all encoder candidates (cached for the process lifetime).
pub fn probe_encoders() -> &'static [CodecInfo] {
    ENCODER_PROBE.get_or_init(|| {
        if let Err(e) = crate::init() {
            tracing::error!("{e}");
            return Vec::new();
        }
        let start = std::time::Instant::now();
        let out: Vec<CodecInfo> = H264_ENCODERS.iter().map(probe_encoder).collect();
        for i in &out {
            tracing::info!(element = %i.element, present = i.present, available = i.available, error = ?i.error, "encoder probe");
        }
        tracing::debug!("encoder probe took {:?}", start.elapsed());
        out
    })
}

pub fn probe_decoders() -> &'static [CodecInfo] {
    DECODER_PROBE.get_or_init(|| {
        if let Err(e) = crate::init() {
            tracing::error!("{e}");
            return Vec::new();
        }
        let encoders = probe_encoders();
        let test_enc = H264_ENCODERS
            .iter()
            .filter(|e| !e.hardware)
            .find(|e| encoders.iter().any(|i| i.element == e.element && i.available));
        let out: Vec<CodecInfo> = H264_DECODERS.iter().map(|d| probe_decoder(d, test_enc)).collect();
        for i in &out {
            tracing::info!(element = %i.element, present = i.present, available = i.available, error = ?i.error, "decoder probe");
        }
        out
    })
}

/// Pick an encoder: the preferred one if it works, else the best available.
pub fn select_encoder(preferred: Option<&str>) -> Result<&'static GstEncoder> {
    let probe = probe_encoders();
    let ok = |name: &str| probe.iter().any(|i| i.element == name && i.available);
    if let Some(p) = preferred {
        if let Some(e) = H264_ENCODERS.iter().find(|e| e.element == p) {
            if ok(p) {
                return Ok(e);
            }
            tracing::warn!("preferred encoder {p} is not available; falling back");
        } else {
            return Err(MediaError::Encoder(format!("unknown encoder `{p}`")));
        }
    }
    H264_ENCODERS.iter().find(|e| ok(e.element)).ok_or_else(|| {
        MediaError::Encoder("no working H.264 encoder found (install x264 or openh264 GStreamer plugins)".into())
    })
}

pub fn select_decoder(preferred: Option<&str>) -> Result<&'static GstDecoder> {
    let probe = probe_decoders();
    let ok = |name: &str| probe.iter().any(|i| i.element == name && i.available);
    if let Some(p) = preferred {
        if let Some(d) = H264_DECODERS.iter().find(|d| d.element == p && ok(p)) {
            return Ok(d);
        }
        tracing::warn!("preferred decoder {p} is not available; falling back");
    }
    H264_DECODERS
        .iter()
        .find(|d| ok(d.element))
        .ok_or_else(|| MediaError::Decoder("no working H.264 decoder found (install gst-libav or openh264)".into()))
}
