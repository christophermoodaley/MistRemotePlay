//! Whole-system tests in one process: a real signaling server, the host
//! service and the client session runner talking over real sockets and
//! WebRTC, with a synthetic video/audio source and mock input backends.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use mist_core::capture::CaptureKind;
use mist_core::client::session::{self, SessionOptions};
use mist_core::client::sinks::HeadlessSink;
use mist_core::client::{ClientCmd, ClientShared};
use mist_core::config::{AudioChoice, ControllerChoice};
use mist_core::events::{ClientEvent, ClientState, HostEvent};
use mist_core::host::{self, HostHandle};
use mist_core::input::InputBackendKind;
use mist_core::media::FrameSink;
use mist_core::protocol::input::{GamepadState, InputMessage, MouseButton};
use mist_core::protocol::peer::now_ms;
use mist_core::sessions::trust::PairedDevice;
use mist_core::sessions::{EndReason, SessionManager, SessionRole, SessionState};
use mist_core::AppContext;
use tokio::sync::{broadcast, mpsc};

async fn start_server() -> String {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let state = mist_core::server::AppState::new(mist_core::server::Config::default());
    tokio::spawn(async move { mist_core::server::serve(state, listener).await.unwrap() });
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
    // Software encoding behaves the same on every machine.
    h.encoder = Some("x264enc".into());
    let s = &mut ctx.config.client.stream;
    s.width = 640;
    s.height = 360;
    s.fps = 30;
    ctx.config.client.controllers_enabled = false;
    ctx
}

async fn wait_host(
    rx: &mut broadcast::Receiver<HostEvent>,
    secs: u64,
    mut pred: impl FnMut(&HostEvent) -> bool,
) -> HostEvent {
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

async fn wait_client(
    rx: &mut broadcast::Receiver<ClientEvent>,
    secs: u64,
    mut pred: impl FnMut(&ClientEvent) -> bool,
) -> ClientEvent {
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
    let HostEvent::PairingCode { code, .. } =
        wait_host(&mut rx, 10, |e| matches!(e, HostEvent::PairingCode { .. })).await
    else {
        unreachable!()
    };
    let c = client.clone();
    let pairing = tokio::spawn(async move { mist_core::client::pair(&c, &code).await });
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

fn start_client(ctx: AppContext, host_id: mist_core::protocol::identity::DeviceId) -> RunningClient {
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
    wait_client(&mut client.events, 30, |e| matches!(e, ClientEvent::State { state: ClientState::Streaming, .. }))
        .await;

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
    let applied = tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            let HostEvent::Stats(s) = wait_host(&mut host_rx, 10, |e| matches!(e, HostEvent::Stats(_))).await else {
                unreachable!()
            };
            if s.input.applied >= 5 {
                break s.input.applied;
            }
        }
    })
    .await
    .expect("host never applied the input");
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
    let ended =
        wait_host(&mut host_rx, 10, |e| matches!(e, HostEvent::Session { state: SessionState::Ended(_), .. })).await;
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
    let result = tokio::time::timeout(Duration::from_secs(15), client.task)
        .await
        .expect("client did not end after rejection")
        .unwrap()
        .unwrap();
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
    wait_client(&mut client.events, 30, |e| matches!(e, ClientEvent::State { state: ClientState::Streaming, .. }))
        .await;
    host.set_input_allowed(false);
    wait_client(&mut client.events, 10, |e| matches!(e, ClientEvent::InputPermission { allowed: false })).await;
    // Input sent now is denied by the host. `Streaming` does not guarantee
    // the input channel is open yet, and input sent before that is dropped,
    // so keep sending until the host reports a denial.
    let denied = tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            client.shared.send_input(InputMessage::Key { usage: 0x04, down: true, repeat: false });
            let HostEvent::Stats(s) = wait_host(&mut host_rx, 10, |e| matches!(e, HostEvent::Stats(_))).await else {
                unreachable!()
            };
            if s.input.denied > 0 {
                break s.input.denied;
            }
        }
    })
    .await
    .expect("host never reported denied input");
    assert!(denied >= 1);

    // 3. Emergency stop from the host.
    host.disconnect_all();
    let result = tokio::time::timeout(Duration::from_secs(15), client.task)
        .await
        .expect("client did not end after the host disconnected everyone")
        .unwrap()
        .unwrap();
    assert!(result.contains("host ended the session"), "{result}");
}

/// Start a client and approve its session in the given role; returns once
/// it streams.
async fn join(
    host: &HostHandle,
    host_rx: &mut broadcast::Receiver<HostEvent>,
    ctx: &AppContext,
    host_id: &mist_core::protocol::identity::DeviceId,
    role: SessionRole,
) -> (RunningClient, String) {
    let mut client = start_client(ctx.clone(), host_id.clone());
    let name = ctx.config.device_name.clone();
    let HostEvent::SessionRequest { session_id, .. } =
        wait_host(host_rx, 15, |e| matches!(e, HostEvent::SessionRequest { client_name, .. } if *client_name == name))
            .await
    else {
        unreachable!()
    };
    host.approve_as(&session_id, role).unwrap();
    wait_client(&mut client.events, 30, |e| matches!(e, ClientEvent::State { state: ClientState::Streaming, .. }))
        .await;
    (client, session_id)
}

/// Keep sending `msg` until the host's stats for `session_id` satisfy `done`.
async fn until_host_stats(
    host_rx: &mut broadcast::Receiver<HostEvent>,
    client: &RunningClient,
    session_id: &str,
    msg: InputMessage,
    mut done: impl FnMut(&mist_core::events::HostStatsEvent) -> bool,
) {
    tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            client.shared.send_input(msg.clone());
            let HostEvent::Stats(s) =
                wait_host(host_rx, 10, |e| matches!(e, HostEvent::Stats(s) if s.session_id == session_id)).await
            else {
                unreachable!()
            };
            if done(&s) {
                break;
            }
        }
    })
    .await
    .expect("host stats never reached the expected state");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn several_people_play_and_watch_one_stream() {
    let url = start_server().await;
    let host_ctx = context("host4", &url);
    let host = host::start(host_ctx.clone());
    let mut host_rx = host.subscribe();
    let ctxs: Vec<AppContext> = ["alice", "bob", "carol"].iter().map(|n| context(n, &url)).collect();
    let mut host_id = None;
    for c in &ctxs {
        host_id = Some(pair(&host, c).await.device_id);
    }
    let host_id = host_id.unwrap();

    let (alice, alice_s) = join(&host, &mut host_rx, &ctxs[0], &host_id, SessionRole::Play).await;
    let (bob, bob_s) = join(&host, &mut host_rx, &ctxs[1], &host_id, SessionRole::Play).await;
    let (mut carol, carol_s) = join(&host, &mut host_rx, &ctxs[2], &host_id, SessionRole::Watch).await;

    let sessions = host.sessions();
    assert_eq!(sessions.len(), 3);
    let info = |id: &str| host.sessions().into_iter().find(|s| s.session_id == id).unwrap();
    // The first player gets keyboard and mouse; later players do not.
    assert!(info(&alice_s).keyboard_mouse);
    assert!(!info(&bob_s).keyboard_mouse);
    assert_eq!(info(&carol_s).role, SessionRole::Watch);
    assert!(!info(&carol_s).input_allowed);

    // Everyone sees the same, changing video.
    tokio::time::sleep(Duration::from_secs(2)).await;
    for c in [&alice, &bob, &carol] {
        let r = c.sink.report();
        assert!(r.frames >= 30 && r.distinct_frames >= 10, "{r:?}");
    }

    // Both players number their controller 0; the host gives them
    // different virtual controllers.
    let pad = |seq| InputMessage::Gamepad { index: 0, seq, state: GamepadState::default() };
    until_host_stats(&mut host_rx, &alice, &alice_s, pad(1), |s| s.controllers == vec![0]).await;
    until_host_stats(&mut host_rx, &bob, &bob_s, pad(1), |s| s.controllers == vec![1]).await;
    // Bob's keyboard and Carol's everything are refused.
    let key = InputMessage::Key { usage: 0x04, down: true, repeat: false };
    until_host_stats(&mut host_rx, &bob, &bob_s, key.clone(), |s| s.input.denied > 0).await;
    until_host_stats(&mut host_rx, &carol, &carol_s, pad(2), |s| s.input.denied > 0 && s.controllers.is_empty()).await;

    // Alice leaves; Bob and Carol keep watching the running stream.
    alice.cmd.send(ClientCmd::Disconnect).unwrap();
    wait_host(
        &mut host_rx,
        10,
        |e| matches!(e, HostEvent::Session { session_id, state: SessionState::Ended(_), .. } if *session_id == alice_s),
    )
    .await;
    let before = (bob.sink.report().frames, carol.sink.report().frames);
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert!(bob.sink.report().frames >= before.0 + 30, "bob's video stalled after alice left");
    assert!(carol.sink.report().frames >= before.1 + 30, "carol's video stalled after alice left");
    assert_eq!(host.sessions().len(), 2);

    // Carol is promoted to player; her client is told input is on.
    host.set_role(&carol_s, SessionRole::Play).unwrap();
    wait_client(&mut carol.events, 10, |e| matches!(e, ClientEvent::InputPermission { allowed: true })).await;
    until_host_stats(&mut host_rx, &carol, &carol_s, pad(3), |s| s.controllers == vec![0]).await;

    host.disconnect_all();
    for c in [bob, carol] {
        let r = tokio::time::timeout(Duration::from_secs(15), c.task).await.unwrap().unwrap().unwrap();
        assert!(r.contains("host ended the session"), "{r}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_full_host_turns_further_people_away() {
    let url = start_server().await;
    let mut host_ctx = context("host5", &url);
    host_ctx.config.host.max_clients = 1;
    let host = host::start(host_ctx.clone());
    let mut host_rx = host.subscribe();
    let first = context("first", &url);
    let second = context("second", &url);
    let host_id = pair(&host, &first).await.device_id;
    pair(&host, &second).await;
    let (_first, _) = join(&host, &mut host_rx, &first, &host_id, SessionRole::Play).await;
    let late = start_client(second, host_id);
    let result = tokio::time::timeout(Duration::from_secs(20), late.task).await.unwrap().unwrap().unwrap();
    assert!(result.contains("the host is full"), "{result}");
    host.disconnect_all();
}
