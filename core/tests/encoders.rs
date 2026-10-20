//! Runs the real host stream (screen capture -> encoder, no network) with
//! each available encoder and reports which ones keep encoding. Needs a
//! display and a GPU: `cargo test -p mist-core --test encoders -- --ignored --nocapture`.

use std::time::{Duration, Instant};

use mist_core::capture::{detect_backends, CaptureBackend};
use mist_core::media::{probe_encoders, HostStream, HostStreamConfig, TransportEvent};
use mist_core::protocol::settings::StreamSettings;
use tokio::sync::mpsc;

fn primary_display() -> (Box<dyn CaptureBackend>, String) {
    let backend = detect_backends().into_iter().next().expect("a capture backend");
    let displays = backend.displays().expect("displays");
    let d = displays.iter().find(|d| d.primary).or(displays.first()).expect("a display").id.clone();
    (backend, d)
}

#[test]
#[ignore]
fn encoders_encode_the_screen() {
    let (backend, display) = primary_display();
    let mut failed = Vec::new();
    // NVENC first: Media Foundation running before it in this process would
    // make Mist swap NVENC for Media Foundation (see `nvenc_usable`).
    let mut encoders: Vec<_> = probe_encoders().iter().filter(|e| e.available).collect();
    encoders.sort_by_key(|e| e.api != "nvenc");
    for info in encoders {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let settings = StreamSettings {
            width: 1920,
            height: 1080,
            fps: 60,
            encoder: Some(info.element.clone()),
            ..Default::default()
        };
        let capture = backend.open(&display).expect("open display");
        let result = HostStream::start(HostStreamConfig { capture, audio: None, settings }, tx);
        let mut errors = Vec::new();
        match &result {
            Ok(_) => {
                let deadline = Instant::now() + Duration::from_secs(4);
                while Instant::now() < deadline {
                    while let Ok(ev) = rx.try_recv() {
                        match ev {
                            TransportEvent::Error(e) => errors.push(e),
                            TransportEvent::Warning(w) => println!("  {}: warning: {w}", info.element),
                            _ => {}
                        }
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
            }
            Err(e) => errors.push(e.to_string()),
        }
        let stats = result.as_ref().ok().map(|s| s.host_stats());
        if let Ok(s) = result {
            s.stop();
        }
        println!("{}: errors={errors:?} stats={stats:?}", info.element);
        if !errors.is_empty() || stats.as_ref().is_some_and(|s| s.encoder != info.element || s.capture_fps < 30.0) {
            failed.push(info.element.clone());
        }
    }
    assert!(failed.is_empty(), "encoders that failed: {failed:?}");
}

fn run(desc: &str) -> Result<(), String> {
    let p = gst::parse::launch(desc).map_err(|e| e.to_string())?;
    let p = p.downcast::<gst::Pipeline>().unwrap();
    use gst::prelude::*;
    p.set_state(gst::State::Playing).map_err(|e| e.to_string())?;
    let msg = p
        .bus()
        .unwrap()
        .timed_pop_filtered(gst::ClockTime::from_seconds(10), &[gst::MessageType::Eos, gst::MessageType::Error]);
    let _ = p.set_state(gst::State::Null);
    match msg.as_ref().map(|m| m.view()) {
        Some(gst::MessageView::Eos(_)) => Ok(()),
        Some(gst::MessageView::Error(e)) => Err(e.error().to_string()),
        _ => Err("timeout".into()),
    }
}

#[test]
#[ignore]
fn nvenc_after_other_encoders() {
    gst::init().unwrap();
    let pipe = |enc: &str| {
        format!("videotestsrc num-buffers=30 ! video/x-raw,format=NV12,width=640,height=360 ! {enc} ! fakesink")
    };
    let order: Vec<String> = std::env::var("MIST_ENC_ORDER")
        .unwrap_or("nvh264enc,mfh264enc,nvh264enc".into())
        .split(',')
        .map(str::to_owned)
        .collect();
    for enc in order {
        println!("{enc}: {:?}", run(&pipe(&enc)));
    }
}
