//! Host -> client over real webrtcbin peer connections on loopback,
//! exchanging SDP/ICE directly (no signaling server).

use std::collections::HashSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

use mist_core::audio::AudioSourceSpec;
use mist_core::capture::CaptureSourceSpec;
use mist_core::media::{
    ClientMedia, ClientMediaConfig, DataChannelKind, FrameSink, HostStream, HostStreamConfig, HostViewer, IceConfig,
    MediaTransport, TransportEvent, VideoFrame, VideoOutput,
};
use mist_core::protocol::settings::StreamSettings;
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
        if frame.times.capture_host_us.is_some() {
            *self.with_capture_ts.lock() += 1;
        }
    }
}

fn start_stream() -> (Arc<HostStream>, mpsc::UnboundedReceiver<TransportEvent>) {
    let (tx, rx) = mpsc::unbounded_channel();
    // Software encoding: the same on every machine (some NVENC setups fail
    // to open a session at stream start).
    let settings =
        StreamSettings { width: 640, height: 360, fps: 30, encoder: Some("x264enc".into()), ..Default::default() };
    let stream = HostStream::start(
        HostStreamConfig {
            capture: CaptureSourceSpec::TestPattern { width: 640, height: 360, pattern: "ball".into() },
            audio: Some(AudioSourceSpec::TestTone { frequency_hz: 440 }),
            settings,
        },
        tx,
    )
    .expect("host stream starts");
    (stream, rx)
}

/// One viewer of the host stream and the client receiving it.
struct Pair {
    viewer: Arc<HostViewer>,
    client: Arc<ClientMedia>,
    host_rx: mpsc::UnboundedReceiver<TransportEvent>,
    client_rx: mpsc::UnboundedReceiver<TransportEvent>,
    sink: Arc<Collect>,
    opened: HashSet<DataChannelKind>,
    got_control: bool,
}

impl Pair {
    fn connect(stream: &Arc<HostStream>) -> Pair {
        let (host_tx, host_rx) = mpsc::unbounded_channel();
        let (client_tx, client_rx) = mpsc::unbounded_channel();
        let viewer = stream.add_viewer(&IceConfig::default(), host_tx).expect("viewer attaches");
        let sink = Arc::new(Collect::default());
        let client = ClientMedia::start(ClientMediaConfig::default(), client_tx, VideoOutput::Frames(sink.clone()))
            .expect("client starts");
        Pair { viewer, client, host_rx, client_rx, sink, opened: HashSet::new(), got_control: false }
    }

    fn frames(&self) -> usize {
        self.sink.hashes.lock().len()
    }

    /// Relay pending SDP/ICE between host and client.
    fn pump(&mut self) {
        while let Ok(ev) = self.host_rx.try_recv() {
            match ev {
                TransportEvent::LocalSdp { kind, sdp } => self.client.set_remote_sdp(kind, &sdp).unwrap(),
                TransportEvent::LocalCandidate { mline_index, candidate } => {
                    self.client.add_remote_candidate(mline_index, &candidate).unwrap()
                }
                TransportEvent::ChannelOpen(k) => {
                    self.opened.insert(k);
                    if k == DataChannelKind::Control {
                        self.viewer.send(k, "{\"hello\":1}").unwrap();
                    }
                }
                TransportEvent::Error(e) => panic!("host error: {e}"),
                _ => {}
            }
        }
        while let Ok(ev) = self.client_rx.try_recv() {
            match ev {
                TransportEvent::LocalSdp { kind, sdp } => self.viewer.set_remote_sdp(kind, &sdp).unwrap(),
                TransportEvent::LocalCandidate { mline_index, candidate } => {
                    self.viewer.add_remote_candidate(mline_index, &candidate).unwrap()
                }
                TransportEvent::ChannelMessage(DataChannelKind::Control, m) => {
                    assert_eq!(m, "{\"hello\":1}");
                    self.got_control = true;
                }
                TransportEvent::Error(e) => panic!("client error: {e}"),
                _ => {}
            }
        }
    }
}

/// Pump every pair until `done` holds or `secs` pass; returns whether it held.
async fn pump_until(pairs: &mut [&mut Pair], secs: u64, done: impl Fn(&[&mut Pair]) -> bool) -> bool {
    let deadline = Instant::now() + Duration::from_secs(secs);
    while Instant::now() < deadline {
        for p in pairs.iter_mut() {
            p.pump();
        }
        if done(pairs) {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    false
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn host_streams_changing_video_to_client() {
    let (stream, _stream_rx) = start_stream();
    let mut p = Pair::connect(&stream);

    let ok = pump_until(&mut [&mut p], 20, |ps| ps[0].frames() >= 60 && ps[0].got_control).await;
    assert!(ok, "received only {} frames (control: {})", p.frames(), p.got_control);
    let hashes = p.sink.hashes.lock().clone();
    let distinct: HashSet<_> = hashes.iter().collect();
    assert!(distinct.len() > hashes.len() / 2, "video is static: {} distinct of {}", distinct.len(), hashes.len());
    assert!(p.sink.sizes.lock().contains(&(640, 360)));
    assert!(*p.sink.with_capture_ts.lock() > 0, "no SEI capture timestamps survived the trip");
    assert!(p.opened.contains(&DataChannelKind::Control) && p.opened.contains(&DataChannelKind::Input));

    let stats = p.viewer.request_stats().await.unwrap();
    let out = stats.outbound_video.as_ref().expect("outbound video stats");
    assert!(out.packets.unwrap_or(0) > 0);
    let cstats = p.client.request_stats().await.unwrap();
    assert!(cstats.inbound_video.as_ref().and_then(|s| s.packets).unwrap_or(0) > 0, "{cstats:?}");
    assert!(cstats.inbound_audio.as_ref().and_then(|s| s.packets).unwrap_or(0) > 0, "no audio packets: {cstats:?}");
    eprintln!("host stats: {:?}\nhost transport: {stats:?}\nclient transport: {cstats:?}", p.viewer.host_stats());

    // The statistics overlay is linked into the running video path and
    // removed again; frames must keep flowing in both states.
    for text in [Some("overlay on"), None, Some("on again"), None] {
        p.client.set_overlay_text(text);
        let before = p.frames();
        let ok = pump_until(&mut [&mut p], 5, |ps| ps[0].frames() >= before + 15).await;
        assert!(ok, "video stalled after overlay {text:?}: {before} -> {} frames", p.frames());
    }
    p.client.stop();
    p.viewer.stop();
    stream.stop();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn viewers_join_and_leave_a_running_stream() {
    let (stream, _stream_rx) = start_stream();
    let mut a = Pair::connect(&stream);
    let mut b = Pair::connect(&stream);
    let ok = pump_until(&mut [&mut a, &mut b], 20, |ps| ps.iter().all(|p| p.frames() >= 30)).await;
    assert!(ok, "both viewers stream: {} and {} frames", a.frames(), b.frames());
    assert_eq!(stream.viewer_count(), 2);

    // Each viewer's congestion control asks for a bitrate; the shared
    // encoder takes the lowest.
    a.viewer.set_bitrate(4000);
    b.viewer.set_bitrate(2500);
    assert_eq!(stream.bitrate_kbps(), 2500);

    // One leaves: the other keeps receiving, and the bitrate follows it.
    a.client.stop();
    a.viewer.stop();
    assert_eq!(stream.viewer_count(), 1);
    b.viewer.set_bitrate(6000);
    assert_eq!(stream.bitrate_kbps(), 6000);
    let before = b.frames();
    let ok = pump_until(&mut [&mut b], 10, |ps| ps[0].frames() >= before + 30).await;
    assert!(ok, "remaining viewer stalled after the other left: {before} -> {}", b.frames());

    // A newcomer joins mid-stream and gets a picture (keyframe on join).
    let mut c = Pair::connect(&stream);
    let ok = pump_until(&mut [&mut b, &mut c], 20, |ps| ps[1].frames() >= 30).await;
    assert!(ok, "late joiner received only {} frames", c.frames());
    assert!(c.sink.sizes.lock().contains(&(640, 360)));

    for p in [&b, &c] {
        p.client.stop();
        p.viewer.stop();
    }
    stream.stop();
}
