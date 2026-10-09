//! Signaling client <-> server integration tests (real sockets, in-process).

use std::sync::Arc;
use std::time::Duration;

use mist_protocol::identity::DeviceIdentity;
use mist_protocol::pairing::PairingCode;
use mist_protocol::peer::{PeerMessage, SignedEnvelope};
use mist_protocol::signaling::{ClientToServer, ErrorCode, Role};
use mist_signaling_client::{connect, SignalingConfig, SignalingEvent};
use mist_signaling_server::{serve, AppState, Config};

async fn start_server(cfg: Config) -> String {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let tls = cfg.tls_cert.is_some();
    let state = AppState::new(cfg);
    tokio::spawn(async move { serve(state, listener).await.unwrap() });
    tokio::time::sleep(Duration::from_millis(100)).await;
    if tls {
        format!("wss://localhost:{}", addr.port())
    } else {
        format!("ws://127.0.0.1:{}", addr.port())
    }
}

fn cfg(url: &str, role: Role) -> SignalingConfig {
    SignalingConfig { url: url.into(), device_name: "test".into(), role, access_token: None, extra_ca_pem: None }
}

async fn next_event(rx: &mut tokio::sync::mpsc::Receiver<SignalingEvent>) -> SignalingEvent {
    tokio::time::timeout(Duration::from_secs(5), rx.recv()).await.expect("event within 5s").expect("channel open")
}

#[tokio::test]
async fn pairing_relay_and_presence() {
    let server_cfg = Config {
        stun_urls: vec!["stun:stun.example.org:3478".into()],
        turn_urls: vec!["turn:turn.example.org:3478?transport=udp".into()],
        turn_secret: Some("s3cret".into()),
        ..Default::default()
    };
    let url = start_server(server_cfg).await;

    let host_id = Arc::new(DeviceIdentity::generate());
    let client_id = Arc::new(DeviceIdentity::generate());
    let (host, mut host_rx) = connect(&cfg(&url, Role::Host), host_id.clone()).await.unwrap();
    let (client, mut client_rx) = connect(&cfg(&url, Role::Client), client_id.clone()).await.unwrap();

    // ICE config handed out, TURN with ephemeral credentials (never the secret).
    assert_eq!(client.ice_servers.len(), 2);
    let turn = client.ice_servers.iter().find(|s| s.is_turn()).unwrap();
    assert!(turn.username.as_ref().unwrap().ends_with(client_id.device_id().as_str()));
    assert_ne!(turn.credential.as_deref(), Some("s3cret"));

    let code = PairingCode::generate();
    host.send(ClientToServer::RegisterPairing { lookup: code.lookup(), ttl_secs: 120 }).await.unwrap();
    assert!(matches!(next_event(&mut host_rx).await, SignalingEvent::PairingRegistered { .. }));

    let proof = code.request_proof(&client_id.public_key_b64().0, "n");
    let env = SignedEnvelope::seal(
        &client_id,
        None,
        PeerMessage::PairRequest {
            client_name: "laptop".into(),
            client_public_key: client_id.public_key_b64(),
            nonce: "n".into(),
            proof,
        },
    );
    client.send(ClientToServer::PairRequest { lookup: code.lookup(), envelope: env.clone() }).await.unwrap();
    match next_event(&mut host_rx).await {
        SignalingEvent::PairRequest { lookup, from, from_public_key, envelope } => {
            assert_eq!(lookup, code.lookup());
            assert_eq!(from, client_id.device_id());
            assert_eq!(envelope, env);
            let body = envelope.open(&from_public_key, Some(&host_id.device_id())).unwrap();
            assert!(matches!(body.message, PeerMessage::PairRequest { .. }));
        }
        other => panic!("unexpected {other:?}"),
    }

    // Unknown pairing code.
    let bogus = PairingCode::generate();
    client.send(ClientToServer::PairRequest { lookup: bogus.lookup(), envelope: env.clone() }).await.unwrap();
    match next_event(&mut client_rx).await {
        SignalingEvent::ServerError { code, .. } => assert_eq!(code, ErrorCode::UnknownPairingCode),
        other => panic!("unexpected {other:?}"),
    }

    // Relay host -> client.
    let msg = SignedEnvelope::seal(
        &host_id,
        Some(client_id.device_id()),
        PeerMessage::EndSession { session_id: "a".repeat(32), reason: "test".into() },
    );
    host.send(ClientToServer::Relay { to: client_id.device_id(), envelope: msg.clone() }).await.unwrap();
    match next_event(&mut client_rx).await {
        SignalingEvent::Relay { from, envelope, .. } => {
            assert_eq!(from, host_id.device_id());
            assert_eq!(envelope, msg);
        }
        other => panic!("unexpected {other:?}"),
    }

    // Presence.
    let ghost = DeviceIdentity::generate().device_id();
    client.send(ClientToServer::QueryPresence { device_ids: vec![host_id.device_id(), ghost.clone()] }).await.unwrap();
    match next_event(&mut client_rx).await {
        SignalingEvent::Presence { online, offline } => {
            assert_eq!(online, vec![host_id.device_id()]);
            assert_eq!(offline, vec![ghost]);
        }
        other => panic!("unexpected {other:?}"),
    }

    // Relay to an offline device fails cleanly.
    client.send(ClientToServer::Relay { to: DeviceIdentity::generate().device_id(), envelope: msg }).await.unwrap();
    match next_event(&mut client_rx).await {
        SignalingEvent::ServerError { code, .. } => assert_eq!(code, ErrorCode::PeerOffline),
        other => panic!("unexpected {other:?}"),
    }
}

#[tokio::test]
async fn pairing_attempts_are_rate_limited() {
    let url = start_server(Config::default()).await;
    let host_id = Arc::new(DeviceIdentity::generate());
    let client_id = Arc::new(DeviceIdentity::generate());
    let (host, mut host_rx) = connect(&cfg(&url, Role::Host), host_id).await.unwrap();
    let code = PairingCode::generate();
    host.send(ClientToServer::RegisterPairing { lookup: code.lookup(), ttl_secs: 60 }).await.unwrap();
    next_event(&mut host_rx).await;
    let (client, mut client_rx) = connect(&cfg(&url, Role::Client), client_id.clone()).await.unwrap();
    let mut limited = false;
    for _ in 0..8 {
        let guess = PairingCode::generate();
        let env = SignedEnvelope::seal(
            &client_id,
            None,
            PeerMessage::PairRequest {
                client_name: "x".into(),
                client_public_key: client_id.public_key_b64(),
                nonce: "n".into(),
                proof: "00".into(),
            },
        );
        client.send(ClientToServer::PairRequest { lookup: guess.lookup(), envelope: env }).await.unwrap();
        if let SignalingEvent::ServerError { code: ErrorCode::RateLimited, .. } = next_event(&mut client_rx).await {
            limited = true;
        }
    }
    assert!(limited, "brute-force pairing attempts were not rate limited");
}

#[tokio::test]
async fn wrong_access_token_and_client_pairing_registration_rejected() {
    let server_cfg = Config { access_token: Some("letmein".into()), ..Default::default() };
    let url = start_server(server_cfg).await;
    let id = Arc::new(DeviceIdentity::generate());
    let mut c = cfg(&url, Role::Host);
    c.access_token = Some("wrong".into());
    assert!(connect(&c, id.clone()).await.is_err());
    c.access_token = None;
    assert!(connect(&c, id.clone()).await.is_err());
    c.access_token = Some("letmein".into());
    connect(&c, id.clone()).await.unwrap();

    // Clients may not register pairing codes.
    let mut cc = cfg(&url, Role::Client);
    cc.access_token = Some("letmein".into());
    let (client, mut rx) = connect(&cc, Arc::new(DeviceIdentity::generate())).await.unwrap();
    client
        .send(ClientToServer::RegisterPairing { lookup: PairingCode::generate().lookup(), ttl_secs: 60 })
        .await
        .unwrap();
    match next_event(&mut rx).await {
        SignalingEvent::ServerError { code, .. } => assert_eq!(code, ErrorCode::Unauthorized),
        other => panic!("unexpected {other:?}"),
    }
}

#[tokio::test]
async fn forged_identity_is_rejected() {
    use futures::{SinkExt, StreamExt};
    use mist_protocol::signaling::ServerToClient;
    use tokio_tungstenite::tungstenite::Message;
    let url = start_server(Config::default()).await;
    let (mut ws, _) = tokio_tungstenite::connect_async(format!("{url}/ws")).await.unwrap();
    let Some(Ok(Message::Text(t))) = ws.next().await else { panic!("no challenge") };
    let ServerToClient::Challenge { nonce, .. } = serde_json_from(&t) else { panic!("not a challenge") };
    let victim = DeviceIdentity::generate();
    let attacker = DeviceIdentity::generate();
    let hello = ClientToServer::Hello {
        protocol_version: mist_protocol::PROTOCOL_VERSION,
        device_id: victim.device_id(),
        public_key: victim.public_key_b64(),
        device_name: "evil".into(),
        role: Role::Host,
        access_token: None,
    };
    // Signed with the wrong key.
    let sig = attacker.sign(&mist_protocol::identity::server_auth_message(&nonce, &victim.device_id()));
    ws.send(Message::Text(mist_protocol::encode(&hello).into())).await.unwrap();
    ws.send(Message::Text(mist_protocol::encode(&ClientToServer::AuthResponse { signature: sig }).into()))
        .await
        .unwrap();
    let mut rejected = false;
    while let Some(Ok(m)) = ws.next().await {
        match m {
            Message::Text(t) => {
                if let ServerToClient::Error { code: ErrorCode::Unauthorized, .. } = serde_json_from(&t) {
                    rejected = true;
                }
            }
            Message::Close(_) => break,
            _ => {}
        }
    }
    assert!(rejected);
}

fn serde_json_from(t: &str) -> mist_protocol::signaling::ServerToClient {
    mist_protocol::decode(t.as_bytes(), 65536).unwrap()
}

#[tokio::test]
async fn tls_with_custom_ca() {
    let dir = std::env::temp_dir().join(format!("mist-tls-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let ca = rcgen::generate_simple_self_signed(vec!["localhost".into()]).unwrap();
    let cert = dir.join("cert.pem");
    let key = dir.join("key.pem");
    std::fs::write(&cert, ca.cert.pem()).unwrap();
    std::fs::write(&key, ca.key_pair.serialize_pem()).unwrap();
    let server_cfg = Config {
        tls_cert: Some(cert.to_string_lossy().into()),
        tls_key: Some(key.to_string_lossy().into()),
        ..Default::default()
    };
    let url = start_server(server_cfg).await;
    let id = Arc::new(DeviceIdentity::generate());
    // Untrusted certificate is refused...
    assert!(connect(&cfg(&url, Role::Host), id.clone()).await.is_err());
    // ...and accepted once its CA is configured.
    let mut c = cfg(&url, Role::Host);
    c.extra_ca_pem = Some(cert);
    connect(&c, id).await.unwrap();
    let _ = std::fs::remove_dir_all(dir);
}
