//! Host -> client over a real webrtcbin peer connection on loopback,
//! exchanging SDP/ICE directly (no signaling server).

use std::collections::HashSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

use mist_audio_api::AudioSourceSpec;
use mist_capture_api::CaptureSourceSpec;
use mist_media_engine::{
    ClientMedia, ClientMediaConfig, DataChannelKind, FrameSink, HostMedia, HostMediaConfig, IceConfig, MediaTransport,
    TransportEvent, VideoFrame,
};
use mist_protocol::settings::StreamSettings;
use parking_lot::Mutex;
use tokio::sync::mpsc;

#[derive(Default)]
struct Collect {
    hashes: Mutex<Vec<u64>>,
    sizes: Mutex<HashSet<(u32, u32)>>,
    with_capture_ts: Mutex<u32>,
}

impl FrameSink for Collect {
    fn push(&self, frame: VideoFrame) {
        use std::hash::{Hash, Hasher};
        let h = frame
            .with_data(|d| {
                let mut s = std::collections::hash_map::DefaultHasher::new();
                d.hash(&mut s);
                s.finish()
            })
            .unwrap();
        self.hashes.lock().push(h);
        self.sizes.lock().insert((frame.width, frame.height));
        if frame.capture_host_us.is_some() {
            *self.with_capture_ts.lock() += 1;
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn host_streams_changing_video_to_client() {
    let _ = tracing_subscriber_init();
    let (host_tx, mut host_rx) = mpsc::unbounded_channel();
    let (client_tx, mut client_rx) = mpsc::unbounded_channel();
    let mut settings = StreamSettings::default();
    settings.width = 640;
    settings.height = 360;
    settings.fps = 30;
    let host = HostMedia::start(
        HostMediaConfig {
            capture: CaptureSourceSpec::TestPattern { width: 640, height: 360, pattern: "ball".into() },
            audio: Some(AudioSourceSpec::TestTone { frequency_hz: 440 }),
            settings,
            ice: IceConfig::default(),
        },
        host_tx,
    )
    .expect("host starts");
    let sink = Arc::new(Collect::default());
    let client = ClientMedia::start(ClientMediaConfig::default(), client_tx, sink.clone()).expect("client starts");

    let deadline = Instant::now() + Duration::from_secs(20);
    let mut opened = HashSet::new();
    let mut got_control = false;
    while Instant::now() < deadline {
        tokio::select! {
            Some(ev) = host_rx.recv() => match ev {
                TransportEvent::LocalSdp { kind, sdp } => client.set_remote_sdp(kind, &sdp).unwrap(),
                TransportEvent::LocalCandidate { mline_index, candidate } => client.add_remote_candidate(mline_index, &candidate).unwrap(),
                TransportEvent::ChannelOpen(k) => { opened.insert(k); if k == DataChannelKind::Control { host.send(k, "{\"hello\":1}").unwrap(); } }
                TransportEvent::Error(e) => panic!("host error: {e}"),
                _ => {}
            },
            Some(ev) = client_rx.recv() => match ev {
                TransportEvent::LocalSdp { kind, sdp } => host.set_remote_sdp(kind, &sdp).unwrap(),
                TransportEvent::LocalCandidate { mline_index, candidate } => host.add_remote_candidate(mline_index, &candidate).unwrap(),
                TransportEvent::ChannelMessage(DataChannelKind::Control, m) => { assert_eq!(m, "{\"hello\":1}"); got_control = true; }
                TransportEvent::Error(e) => panic!("client error: {e}"),
                _ => {}
            },
            _ = tokio::time::sleep(Duration::from_millis(50)) => {}
        }
        if sink.hashes.lock().len() >= 60 && got_control {
            break;
        }
    }
    let hashes = sink.hashes.lock().clone();
    assert!(hashes.len() >= 60, "received only {} frames", hashes.len());
    let distinct: HashSet<_> = hashes.iter().collect();
    assert!(distinct.len() > hashes.len() / 2, "video is static: {} distinct of {}", distinct.len(), hashes.len());
    assert!(sink.sizes.lock().contains(&(640, 360)));
    assert!(*sink.with_capture_ts.lock() > 0, "no SEI capture timestamps survived the trip");
    assert!(opened.contains(&DataChannelKind::Control) && opened.contains(&DataChannelKind::Input));
    assert!(got_control, "control channel message not delivered");

    let stats = host.request_stats().await.unwrap();
    let out = stats.outbound_video.as_ref().expect("outbound video stats");
    assert!(out.packets.unwrap_or(0) > 0);
    let cstats = client.request_stats().await.unwrap();
    assert!(cstats.inbound_video.as_ref().and_then(|s| s.packets).unwrap_or(0) > 0, "{cstats:?}");
    assert!(cstats.inbound_audio.as_ref().and_then(|s| s.packets).unwrap_or(0) > 0, "no audio packets: {cstats:?}");
    eprintln!("host stats: {:?}\nhost transport: {stats:?}\nclient transport: {cstats:?}", host.host_stats());
    client.stop();
    host.stop();
}

fn tracing_subscriber_init() -> Option<()> {
    None
}
