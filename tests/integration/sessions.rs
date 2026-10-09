//! Whole-system tests in one process: a real signaling server, the host
//! service and the client session runner talking over real sockets and
//! WebRTC, with a synthetic video/audio source and mock input backends.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use mist_app_core::client::session::{self, SessionOptions};
use mist_app_core::client::sinks::HeadlessSink;
use mist_app_core::client::{ClientCmd, ClientShared};
use mist_app_core::config::{AudioChoice, ControllerChoice};
use mist_app_core::events::{ClientEvent, ClientState, HostEvent};
use mist_app_core::host::{self, HostHandle};
use mist_app_core::AppContext;
use mist_capture_api::CaptureKind;
use mist_input_api::InputBackendKind;
use mist_media_engine::FrameSink;
use mist_protocol::input::{InputMessage, MouseButton};
use mist_protocol::peer::now_ms;
use mist_session_manager::trust::PairedDevice;
use mist_session_manager::{EndReason, SessionManager, SessionState};
use tokio::sync::{broadcast, mpsc};

async fn start_server() -> String {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let state = mist_signaling_server::AppState::new(mist_signaling_server::Config::default());
    tokio::spawn(async move { mist_signaling_server::serve(state, listener).await.unwrap() });
    tokio::time::sleep(Duration::from_millis(100)).await;
    format!("ws://127.0.0.1:{port}")
}

fn temp_dir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("mist-it-{tag}-{}-{}", std::process::id(), now_ms()));
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn context(tag: &str, url: &str) -> AppContext {
    let mut ctx = AppContext::load_from(temp_dir(tag)).unwrap();
    ctx.config.device_name = tag.into();
    ctx.config.network.signaling_url = url.into();
    ctx.config.network.lan_discovery = false;
    let h = &mut ctx.config.host;
    h.capture = Some(CaptureKind::TestPattern);
    h.audio = AudioChoice::TestTone;
    h.input_backend = InputBackendKind::Mock;
    h.controllers = ControllerChoice::Mock;
    let s = &mut ctx.config.client.stream;
    s.width = 640;
    s.height = 360;
    s.fps = 30;
    ctx.config.client.controllers_enabled = false;
    ctx
}

async fn wait_host(rx: &mut broadcast::Receiver<HostEvent>, secs: u64, mut pred: impl FnMut(&HostEvent) -> bool) -> HostEvent {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(secs);
    loop {
        match tokio::time::timeout_at(deadline, rx.recv()).await {
            Ok(Ok(ev)) if pred(&ev) => return ev,
            Ok(Ok(_)) | Ok(Err(broadcast::error::RecvError::Lagged(_))) => continue,
            Ok(Err(e)) => panic!("host events closed: {e}"),
            Err(_) => panic!("timed out waiting for host event"),
        }
    }
}

async fn wait_client(rx: &mut broadcast::Receiver<ClientEvent>, secs: u64, mut pred: impl FnMut(&ClientEvent) -> bool) -> ClientEvent {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(secs);
    loop {
        match tokio::time::timeout_at(deadline, rx.recv()).await {
            Ok(Ok(ev)) if pred(&ev) => return ev,
            Ok(Ok(_)) | Ok(Err(broadcast::error::RecvError::Lagged(_))) => continue,
            Ok(Err(e)) => panic!("client events closed: {e}"),
            Err(_) => panic!("timed out waiting for client event"),
        }
    }
}

/// Pair `client` with the running host through the real code flow.
async fn pair(host: &HostHandle, client: &AppContext) -> PairedDevice {
    let mut rx = host.subscribe();
    wait_until_online(host).await;
    host.generate_pairing_code();
    let HostEvent::PairingCode { code, .. } = wait_host(&mut rx, 10, |e| matches!(e, HostEvent::PairingCode { .. })).await else {
        unreachable!()
    };
    let c = client.clone();
    let pairing = tokio::spawn(async move { mist_app_core::client::pair(&c, &code).await });
    let HostEvent::PairingRequest { request_id, .. } =
        wait_host(&mut rx, 10, |e| matches!(e, HostEvent::PairingRequest { .. })).await
    else {
        unreachable!()
    };
    host.decide_pairing(&request_id, true).unwrap();
    pairing.await.unwrap().expect("pairing succeeds")
}

async fn wait_until_online(host: &HostHandle) {
    for _ in 0..100 {
        if host.snapshot().signaling_connected {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("host never connected to signaling");
}

struct RunningClient {
    events: broadcast::Receiver<ClientEvent>,
    shared: Arc<ClientShared>,
    sink: Arc<HeadlessSink>,
    cmd: mpsc::UnboundedSender<ClientCmd>,
    task: tokio::task::JoinHandle<anyhow::Result<String>>,
}

fn start_client(ctx: AppContext, host_id: mist_protocol::identity::DeviceId) -> RunningClient {
    let (cmd, cmd_rx) = mpsc::unbounded_channel();
    let shared = ClientShared::new(cmd.clone());
    let sink = HeadlessSink::new(shared.clone());
    let (tx, events) = broadcast::channel(512);
    let s2 = shared.clone();
    let sink2: Arc<dyn FrameSink> = sink.clone();
    let task = tokio::spawn(async move {
        session::run(ctx, SessionOptions { host: host_id, controllers: false }, s2, sink2, tx, cmd_rx).await
    });
    RunningClient { events, shared, sink, cmd, task }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn full_session_pairing_consent_video_input_and_cleanup() {
    let url = start_server().await;
    let host_ctx = context("host", &url);
    let client_ctx = context("client", &url);
    let host = host::start(host_ctx.clone());
    let mut host_rx = host.subscribe();

    let paired = pair(&host, &client_ctx).await;
    assert_eq!(paired.device_id, host_ctx.identity.device_id());
    assert!(host_ctx.trust.lock().get(&client_ctx.identity.device_id()).is_some(), "host stored the client");

    let mut client = start_client(client_ctx.clone(), paired.device_id.clone());
    // Explicit consent: the session waits until the host approves.
    let HostEvent::SessionRequest { session_id, .. } =
        wait_host(&mut host_rx, 15, |e| matches!(e, HostEvent::SessionRequest { .. })).await
    else {
        unreachable!()
    };
    assert_eq!(host.sessions()[0].state, SessionState::AwaitingApproval);
    assert_eq!(client.sink.report().frames, 0, "no media before consent");
    host.approve(&session_id).unwrap();
    wait_client(&mut client.events, 30, |e| matches!(e, ClientEvent::State { state: ClientState::Streaming, .. })).await;

    // Input travels client -> host dispatcher (mock backend).
    tokio::time::sleep(Duration::from_millis(500)).await;
    for msg in [
        InputMessage::MouseMoveAbs { x: 100, y: 100 },
        InputMessage::MouseButton { button: MouseButton::Left, down: true },
        InputMessage::MouseButton { button: MouseButton::Left, down: false },
        InputMessage::Key { usage: 0x04, down: true, repeat: false },
        InputMessage::Key { usage: 0x04, down: false, repeat: false },
    ] {
        client.shared.send_input(msg);
    }
    let applied = loop {
        let HostEvent::Stats(s) = wait_host(&mut host_rx, 10, |e| matches!(e, HostEvent::Stats(_))).await else { unreachable!() };
        if s.input.applied >= 5 {
            break s.input.applied;
        }
    };
    assert!(applied >= 5);

    // Live, changing video with capture timestamps.
    tokio::time::sleep(Duration::from_secs(2)).await;
    let report = client.sink.report();
    assert!(report.frames >= 30, "{report:?}");
    assert!(report.distinct_frames >= 10, "video is not changing: {report:?}");
    assert!(report.frames_with_capture_timestamp > 0);
    assert!(client.shared.recent_latency_ms().is_some(), "latency was not measured");

    // Client disconnects: host ends the session and cleans up.
    client.cmd.send(ClientCmd::Disconnect).unwrap();
    let ended = wait_host(&mut host_rx, 10, |e| matches!(e, HostEvent::Session { state: SessionState::Ended(_), .. })).await;
    let HostEvent::Session { state: SessionState::Ended(reason), .. } = ended else { unreachable!() };
    assert_eq!(reason, EndReason::ClientDisconnected);
    assert_eq!(client.task.await.unwrap().unwrap(), "disconnected");
    assert!(host.sessions().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn unpaired_device_is_rejected_without_prompt() {
    let url = start_server().await;
    let host_ctx = context("host2", &url);
    let stranger = context("stranger", &url);
    let host = host::start(host_ctx.clone());
    let mut host_rx = host.subscribe();
    wait_until_online(&host).await;
    // The stranger knows the host's public key (e.g. copied a config) but
    // the host never paired with it.
    stranger
        .trust
        .lock()
        .add(PairedDevice {
            device_id: host_ctx.identity.device_id(),
            name: "host2".into(),
            public_key: host_ctx.identity.public_key_b64(),
            paired_at_ms: now_ms(),
            last_connected_ms: None,
            signaling_url: Some(url.clone()),
        })
        .unwrap();
    let client = start_client(stranger, host_ctx.identity.device_id());
    let result = tokio::time::timeout(Duration::from_secs(20), client.task).await.unwrap().unwrap().unwrap();
    assert!(result.contains("not paired"), "{result}");
    // The host user was never asked.
    while let Ok(ev) = host_rx.try_recv() {
        assert!(!matches!(ev, HostEvent::SessionRequest { .. }), "unpaired device reached the consent prompt");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn declined_session_never_streams_and_revoked_input_is_reported() {
    let url = start_server().await;
    let host_ctx = context("host3", &url);
    let client_ctx = context("client3", &url);
    let host = host::start(host_ctx.clone());
    let mut host_rx = host.subscribe();
    let paired = pair(&host, &client_ctx).await;

    // 1. Host declines.
    let client = start_client(client_ctx.clone(), paired.device_id.clone());
    let HostEvent::SessionRequest { session_id, .. } =
        wait_host(&mut host_rx, 15, |e| matches!(e, HostEvent::SessionRequest { .. })).await
    else {
        unreachable!()
    };
    host.reject(&session_id, "not now").unwrap();
    let result = client.task.await.unwrap().unwrap();
    assert!(result.contains("rejected"), "{result}");
    assert_eq!(client.sink.report().frames, 0);

    // 2. Approved session; then the host revokes input mid-session.
    let mut client = start_client(client_ctx.clone(), paired.device_id.clone());
    let HostEvent::SessionRequest { session_id, .. } =
        wait_host(&mut host_rx, 15, |e| matches!(e, HostEvent::SessionRequest { .. })).await
    else {
        unreachable!()
    };
    host.approve(&session_id).unwrap();
    wait_client(&mut client.events, 30, |e| matches!(e, ClientEvent::State { state: ClientState::Streaming, .. })).await;
    host.set_input_allowed(false);
    wait_client(&mut client.events, 10, |e| matches!(e, ClientEvent::InputPermission { allowed: false })).await;
    // Input sent now is denied by the host.
    client.shared.send_input(InputMessage::Key { usage: 0x04, down: true, repeat: false });
    let denied = loop {
        let HostEvent::Stats(s) = wait_host(&mut host_rx, 10, |e| matches!(e, HostEvent::Stats(_))).await else { unreachable!() };
        if s.input.denied > 0 {
            break s.input.denied;
        }
    };
    assert!(denied >= 1);

    // 3. Emergency stop from the host.
    host.disconnect_all();
    let result = client.task.await.unwrap().unwrap();
    assert!(result.contains("host ended the session"), "{result}");
}
