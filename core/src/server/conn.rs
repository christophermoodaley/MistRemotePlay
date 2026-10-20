//! One WebSocket connection: authentication, then message routing.

use std::net::SocketAddr;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Instant;

use crate::protocol::identity::{random_hex, server_auth_message, verify_signature, DeviceId, PublicKeyB64};
use crate::protocol::ratelimit::TokenBucket;
use crate::protocol::signaling::{ClientToServer, ErrorCode, Role, ServerToClient};
use crate::protocol::{decode, encode, PROTOCOL_VERSION};
use axum::extract::ws::{CloseFrame, Message, WebSocket};
use futures::{SinkExt, StreamExt};
use tokio::sync::mpsc;
use tokio::time::{sleep_until, timeout};

use crate::server::{AppState, Device, Outbound, Pairing};

const MAX_PAIRINGS_PER_HOST: usize = 4;
const MAX_INVALID_MESSAGES: u32 = 10;

struct Authed {
    device_id: DeviceId,
    public_key: PublicKeyB64,
    role: Role,
}

fn err(code: ErrorCode, message: impl Into<String>) -> Outbound {
    Outbound::Msg(Box::new(ServerToClient::Error { code, message: message.into() }))
}

fn msg(m: ServerToClient) -> Outbound {
    Outbound::Msg(Box::new(m))
}

async fn next_text(stream: &mut futures::stream::SplitStream<WebSocket>) -> Option<Result<String, &'static str>> {
    loop {
        match stream.next().await? {
            Ok(Message::Text(t)) => return Some(Ok(t.to_string())),
            Ok(Message::Binary(_)) => return Some(Err("binary frames are not accepted")),
            Ok(Message::Close(_)) | Err(_) => return None,
            Ok(Message::Ping(_)) | Ok(Message::Pong(_)) => continue,
        }
    }
}

async fn authenticate(
    state: &AppState,
    stream: &mut futures::stream::SplitStream<WebSocket>,
    tx: &mpsc::Sender<Outbound>,
    nonce: &str,
) -> Result<Authed, String> {
    let text = next_text(stream).await.ok_or("closed before hello")??;
    let hello: ClientToServer =
        decode(text.as_bytes(), crate::protocol::MAX_SIGNALING_MESSAGE_BYTES).map_err(|e| e.to_string())?;
    let ClientToServer::Hello { protocol_version, device_id, public_key, device_name: _, role, access_token } = hello
    else {
        return Err("expected hello".into());
    };
    if protocol_version != PROTOCOL_VERSION {
        let _ = tx.send(err(ErrorCode::UnsupportedVersion, format!("server speaks protocol {PROTOCOL_VERSION}"))).await;
        return Err(format!("unsupported protocol version {protocol_version}"));
    }
    if let Some(expected) = &state.config.access_token {
        let ok = access_token.as_deref().map(|t| {
            use subtle_eq::ct_eq;
            ct_eq(t.as_bytes(), expected.as_bytes())
        });
        if ok != Some(true) {
            return Err("missing or wrong access token".into());
        }
    }
    let text = next_text(stream).await.ok_or("closed before auth response")??;
    let resp: ClientToServer = decode(text.as_bytes(), 4096).map_err(|e| e.to_string())?;
    let ClientToServer::AuthResponse { signature } = resp else { return Err("expected auth_response".into()) };
    let key = public_key.to_key().map_err(|e| e.to_string())?;
    verify_signature(&key, &server_auth_message(nonce, &device_id), &signature).map_err(|e| e.to_string())?;
    Ok(Authed { device_id, public_key, role })
}

/// Constant-time comparison without pulling another dependency in.
mod subtle_eq {
    pub fn ct_eq(a: &[u8], b: &[u8]) -> bool {
        if a.len() != b.len() {
            return false;
        }
        a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
    }
}

pub(crate) async fn handle(state: Arc<AppState>, socket: WebSocket, addr: SocketAddr) {
    state.metrics.connections_total.fetch_add(1, Ordering::Relaxed);
    let conn_id = state.next_conn.fetch_add(1, Ordering::Relaxed);
    let (mut sink, mut stream) = socket.split();
    let (tx, mut rx) = mpsc::channel::<Outbound>(128);
    let writer = tokio::spawn(async move {
        while let Some(out) = rx.recv().await {
            match out {
                Outbound::Msg(m) => {
                    if sink.send(Message::Text(encode(&*m).into())).await.is_err() {
                        break;
                    }
                }
                Outbound::Close(reason) => {
                    let _ = sink.send(Message::Close(Some(CloseFrame { code: 4000, reason: reason.into() }))).await;
                    break;
                }
            }
        }
    });

    let nonce = random_hex(32);
    let _ = tx
        .send(msg(ServerToClient::Challenge {
            nonce: nonce.clone(),
            server_version: crate::server::SERVER_VERSION.into(),
        }))
        .await;
    let authed = match timeout(state.config.auth_timeout, authenticate(&state, &mut stream, &tx, &nonce)).await {
        Ok(Ok(a)) => a,
        Ok(Err(e)) => {
            state.metrics.auth_failures.fetch_add(1, Ordering::Relaxed);
            tracing::info!(conn = conn_id, ip = %addr.ip(), "authentication failed: {e}");
            let _ = tx.send(err(ErrorCode::Unauthorized, "authentication failed")).await;
            let _ = tx.send(Outbound::Close("unauthorized")).await;
            drop(tx);
            let _ = writer.await;
            return;
        }
        Err(_) => {
            state.metrics.auth_failures.fetch_add(1, Ordering::Relaxed);
            let _ = tx.send(Outbound::Close("authentication timeout")).await;
            drop(tx);
            let _ = writer.await;
            return;
        }
    };
    let device_id = authed.device_id.clone();
    tracing::info!(conn = conn_id, device = %device_id, role = ?authed.role, ip = %addr.ip(), "device authenticated");

    // Register; a reconnecting device replaces its stale connection.
    if let Some(old) = state.devices.lock().insert(device_id.clone(), Device { conn_id, tx: tx.clone() }) {
        let _ = old.tx.try_send(Outbound::Close("replaced by a newer connection"));
    }
    let _ = tx
        .send(msg(ServerToClient::Welcome {
            ice_servers: crate::server::turn::ice_servers(&state.config, &device_id),
            connection_ttl_secs: state.config.connection_ttl.as_secs() as u32,
        }))
        .await;

    let deadline = tokio::time::Instant::now() + state.config.connection_ttl;
    let mut general = TokenBucket::new(30.0, 120.0);
    let mut pair_bucket = TokenBucket::new(5.0 / 60.0, 5.0);
    let mut invalid = 0u32;
    loop {
        let next = tokio::select! {
            r = timeout(state.config.idle_timeout, next_text(&mut stream)) => r,
            _ = sleep_until(deadline) => {
                let _ = tx.send(Outbound::Close("connection lifetime expired; reconnect")).await;
                break;
            }
        };
        let text = match next {
            Err(_) => {
                let _ = tx.send(Outbound::Close("idle timeout")).await;
                break;
            }
            Ok(None) => break,
            Ok(Some(Err(e))) => {
                let _ = tx.send(err(ErrorCode::BadRequest, e)).await;
                invalid += 1;
                if invalid > MAX_INVALID_MESSAGES {
                    break;
                }
                continue;
            }
            Ok(Some(Ok(t))) => t,
        };
        if !general.try_take() {
            state.metrics.rate_limited.fetch_add(1, Ordering::Relaxed);
            let _ = tx.send(err(ErrorCode::RateLimited, "slow down")).await;
            continue;
        }
        let m: ClientToServer = match decode(text.as_bytes(), crate::protocol::MAX_SIGNALING_MESSAGE_BYTES) {
            Ok(m) => m,
            Err(e) => {
                invalid += 1;
                let _ = tx.send(err(ErrorCode::BadRequest, e.to_string())).await;
                if invalid > MAX_INVALID_MESSAGES {
                    tracing::info!(conn = conn_id, device = %device_id, "too many invalid messages; closing");
                    break;
                }
                continue;
            }
        };
        if let Some(reply) = route(&state, &authed, m, &mut pair_bucket) {
            let _ = tx.send(reply).await;
        }
    }

    // Cleanup.
    {
        let mut devices = state.devices.lock();
        if devices.get(&device_id).map(|d| d.conn_id) == Some(conn_id) {
            devices.remove(&device_id);
        }
    }
    if authed.role == Role::Host {
        let still_online = state.devices.lock().contains_key(&device_id);
        if !still_online {
            state.pairings.lock().retain(|_, p| p.host != device_id);
        }
    }
    tracing::info!(conn = conn_id, device = %device_id, "device disconnected");
    drop(tx);
    let _ = writer.await;
}

fn route(state: &AppState, me: &Authed, m: ClientToServer, pair_bucket: &mut TokenBucket) -> Option<Outbound> {
    match m {
        ClientToServer::Hello { .. } | ClientToServer::AuthResponse { .. } => {
            Some(err(ErrorCode::BadRequest, "already authenticated"))
        }
        ClientToServer::Ping {} => Some(msg(ServerToClient::Pong {})),
        ClientToServer::RegisterPairing { lookup, ttl_secs } => {
            if me.role != Role::Host {
                return Some(err(ErrorCode::Unauthorized, "only hosts register pairing codes"));
            }
            let mut pairings = state.pairings.lock();
            let now = Instant::now();
            pairings.retain(|_, p| p.expires > now);
            let mine = pairings.values().filter(|p| p.host == me.device_id).count();
            if mine >= MAX_PAIRINGS_PER_HOST && !pairings.contains_key(&lookup) {
                return Some(err(ErrorCode::RateLimited, "too many active pairing codes"));
            }
            if let Some(existing) = pairings.get(&lookup) {
                if existing.host != me.device_id {
                    return Some(err(ErrorCode::BadRequest, "pairing code collision; generate a new one"));
                }
            }
            pairings.insert(
                lookup.clone(),
                Pairing { host: me.device_id.clone(), expires: now + std::time::Duration::from_secs(ttl_secs as u64) },
            );
            Some(msg(ServerToClient::PairingRegistered { lookup, expires_in_secs: ttl_secs }))
        }
        ClientToServer::CancelPairing { lookup } => {
            let mut pairings = state.pairings.lock();
            if pairings.get(&lookup).map(|p| p.host == me.device_id) == Some(true) {
                pairings.remove(&lookup);
            }
            None
        }
        ClientToServer::PairRequest { lookup, envelope } => {
            if !pair_bucket.try_take() {
                state.metrics.rate_limited.fetch_add(1, Ordering::Relaxed);
                return Some(err(ErrorCode::RateLimited, "too many pairing attempts; wait a minute"));
            }
            let host = {
                let pairings = state.pairings.lock();
                pairings.get(&lookup).filter(|p| p.expires > Instant::now()).map(|p| p.host.clone())
            };
            let Some(host) = host else {
                return Some(err(ErrorCode::UnknownPairingCode, "pairing code unknown or expired"));
            };
            deliver(
                state,
                &host,
                ServerToClient::PairRequest {
                    lookup,
                    from: me.device_id.clone(),
                    from_public_key: me.public_key.clone(),
                    envelope,
                },
            )
        }
        ClientToServer::Relay { to, envelope } => deliver(
            state,
            &to,
            ServerToClient::Relay { from: me.device_id.clone(), from_public_key: me.public_key.clone(), envelope },
        ),
        ClientToServer::QueryPresence { device_ids } => {
            let devices = state.devices.lock();
            let (online, offline) = device_ids.into_iter().partition(|d| devices.contains_key(d));
            Some(msg(ServerToClient::Presence { online, offline }))
        }
    }
}

fn deliver(state: &AppState, to: &DeviceId, m: ServerToClient) -> Option<Outbound> {
    let devices = state.devices.lock();
    let Some(dev) = devices.get(to) else {
        return Some(err(ErrorCode::PeerOffline, "peer is not connected"));
    };
    match dev.tx.try_send(msg(m)) {
        Ok(()) => {
            state.metrics.relayed.fetch_add(1, Ordering::Relaxed);
            None
        }
        Err(_) => Some(err(ErrorCode::PeerOffline, "peer is not keeping up")),
    }
}
