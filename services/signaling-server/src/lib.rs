//! Mist signaling server.
//!
//! Responsibilities: authenticate devices (Ed25519 challenge), route signed
//! envelopes between online devices, route pairing requests by code lookup,
//! and hand out STUN/TURN configuration. It holds only in-memory, short-lived
//! state and never sees media, input, pairing codes or passwords.

pub mod config;
mod conn;
pub mod turn;

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

use axum::extract::{ConnectInfo, State, WebSocketUpgrade};
use axum::response::IntoResponse;
use axum::routing::get;
use axum::{Json, Router};
use mist_protocol::identity::DeviceId;
use mist_protocol::signaling::ServerToClient;
use parking_lot::Mutex;
use tokio::sync::mpsc;

pub use config::Config;

pub const SERVER_VERSION: &str = concat!("mist-signaling/", env!("CARGO_PKG_VERSION"));

/// Messages queued to a connection's writer task.
#[derive(Debug)]
pub(crate) enum Outbound {
    Msg(Box<ServerToClient>),
    Close(&'static str),
}

pub(crate) struct Device {
    pub conn_id: u64,
    pub tx: mpsc::Sender<Outbound>,
}

pub(crate) struct Pairing {
    pub host: DeviceId,
    pub expires: Instant,
}

#[derive(Default)]
pub struct Metrics {
    pub connections_total: AtomicU64,
    pub auth_failures: AtomicU64,
    pub relayed: AtomicU64,
    pub rate_limited: AtomicU64,
}

pub struct AppState {
    pub(crate) config: Config,
    pub(crate) devices: Mutex<HashMap<DeviceId, Device>>,
    pub(crate) pairings: Mutex<HashMap<String, Pairing>>,
    pub(crate) per_ip: Mutex<HashMap<IpAddr, usize>>,
    pub(crate) next_conn: AtomicU64,
    pub metrics: Metrics,
}

impl AppState {
    pub fn new(config: Config) -> Arc<Self> {
        Arc::new(AppState {
            config,
            devices: Mutex::new(HashMap::new()),
            pairings: Mutex::new(HashMap::new()),
            per_ip: Mutex::new(HashMap::new()),
            next_conn: AtomicU64::new(1),
            metrics: Metrics::default(),
        })
    }

    pub fn online_devices(&self) -> usize {
        self.devices.lock().len()
    }

    /// Drop expired pairing registrations.
    pub(crate) fn sweep(&self) {
        let now = Instant::now();
        self.pairings.lock().retain(|_, p| p.expires > now);
    }
}

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/healthz", get(|| async { "ok" }))
        .route("/readyz", get(readyz))
        .route("/ws", get(ws_upgrade))
        .with_state(state)
}

async fn readyz(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let m = &state.metrics;
    Json(serde_json::json!({
        "status": "ready",
        "version": SERVER_VERSION,
        "online_devices": state.online_devices(),
        "active_pairings": state.pairings.lock().len(),
        "connections_total": m.connections_total.load(Ordering::Relaxed),
        "auth_failures": m.auth_failures.load(Ordering::Relaxed),
        "relayed_messages": m.relayed.load(Ordering::Relaxed),
        "rate_limited": m.rate_limited.load(Ordering::Relaxed),
        "turn_configured": state.config.turn_secret.is_some() && !state.config.turn_urls.is_empty(),
    }))
}

async fn ws_upgrade(
    ws: WebSocketUpgrade,
    State(state): State<Arc<AppState>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
) -> axum::response::Response {
    let ip = addr.ip();
    {
        let total: usize = state.per_ip.lock().values().sum();
        let mut per_ip = state.per_ip.lock();
        let n = per_ip.entry(ip).or_insert(0);
        if total >= state.config.max_connections || *n >= state.config.max_connections_per_ip {
            state.metrics.rate_limited.fetch_add(1, Ordering::Relaxed);
            tracing::warn!(%ip, "connection limit reached");
            return (axum::http::StatusCode::TOO_MANY_REQUESTS, "too many connections").into_response();
        }
        *n += 1;
    }
    ws.max_message_size(mist_protocol::MAX_SIGNALING_MESSAGE_BYTES)
        .max_frame_size(mist_protocol::MAX_SIGNALING_MESSAGE_BYTES)
        .on_upgrade(move |socket| async move {
            conn::handle(state.clone(), socket, addr).await;
            let mut per_ip = state.per_ip.lock();
            if let Some(n) = per_ip.get_mut(&ip) {
                *n -= 1;
                if *n == 0 {
                    per_ip.remove(&ip);
                }
            }
        })
}

/// Serve wss:// with an in-memory certificate and key (PEM), for servers
/// embedded in another program that keeps its key outside plain files.
pub async fn serve_with_pem(
    state: Arc<AppState>,
    listener: std::net::TcpListener,
    cert_pem: Vec<u8>,
    key_pem: Vec<u8>,
) -> anyhow::Result<()> {
    let app = router(state.clone()).into_make_service_with_connect_info::<SocketAddr>();
    spawn_sweeper(state);
    listener.set_nonblocking(true)?;
    let _ = rustls::crypto::ring::default_provider().install_default();
    let tls = axum_server::tls_rustls::RustlsConfig::from_pem(cert_pem, key_pem).await?;
    tracing::info!(addr = %listener.local_addr()?, "serving wss:// (embedded)");
    axum_server::from_tcp_rustls(listener, tls).serve(app).await?;
    Ok(())
}

fn spawn_sweeper(state: Arc<AppState>) {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(std::time::Duration::from_secs(30));
        loop {
            tick.tick().await;
            state.sweep();
        }
    });
}

/// Serve on an already-bound listener (used by `main` and tests).
pub async fn serve(state: Arc<AppState>, listener: std::net::TcpListener) -> anyhow::Result<()> {
    let app = router(state.clone()).into_make_service_with_connect_info::<SocketAddr>();
    let sweeper = state.clone();
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(std::time::Duration::from_secs(30));
        loop {
            tick.tick().await;
            sweeper.sweep();
        }
    });
    listener.set_nonblocking(true)?;
    match (&state.config.tls_cert, &state.config.tls_key) {
        (Some(cert), Some(key)) => {
            let _ = rustls::crypto::ring::default_provider().install_default();
            let tls = axum_server::tls_rustls::RustlsConfig::from_pem_file(cert, key).await?;
            tracing::info!(addr = %listener.local_addr()?, "serving wss://");
            axum_server::from_tcp_rustls(listener, tls).serve(app).await?;
        }
        _ => {
            tracing::warn!(addr = %listener.local_addr()?, "serving plain ws:// (use TLS or a TLS proxy in production)");
            axum_server::from_tcp(listener).serve(app).await?;
        }
    }
    Ok(())
}
