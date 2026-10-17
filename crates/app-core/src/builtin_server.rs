//! The signaling server built into the host, so nothing else has to be
//! installed: while hosting, this PC accepts signaling connections on
//! `server_port` (TLS with a self-signed certificate that clients pin on
//! first use) and offers public STUN servers for NAT traversal.
//!
//! The server only relays signed messages between devices; it never sees
//! media or input (see docs/security.md).

use std::net::{IpAddr, Ipv4Addr, SocketAddr};

use serde::Serialize;

use crate::AppContext;

/// Default STUN servers handed to devices using the built-in server.
/// webrtcbin uses only the first one. Google's comes first: Cloudflare's
/// never answered libnice's binding requests (measured on Windows: no
/// server-reflexive candidates, so peers behind NAT could not connect over
/// the internet), although it answers plain RFC 5389 requests.
pub const DEFAULT_STUN: &[&str] = &["stun:stun.l.google.com:19302", "stun:stun.cloudflare.com:3478"];

const CERT_FILE: &str = "server-cert.pem";
const KEY_FILE: &str = "server-key.pem";

/// What other devices need to reach this host's server.
#[derive(Clone, Debug, Default, Serialize, PartialEq, Eq)]
pub struct ServerInfo {
    pub port: u16,
    /// SHA-256 of the server certificate, lowercase hex.
    pub fingerprint: String,
    /// `ip:port` on each local network this PC is connected to.
    pub lan_addresses: Vec<String>,
}

pub struct BuiltinServer {
    pub info: ServerInfo,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for BuiltinServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl BuiltinServer {
    /// URL the host itself uses to reach the server.
    pub fn local_url(&self) -> String {
        format!("wss://127.0.0.1:{}", self.info.port)
    }
}

/// Load this PC's server certificate, creating it on first use. The key is
/// kept in owner-only storage (DPAPI-encrypted on Windows).
pub fn load_or_create_cert(ctx: &AppContext) -> anyhow::Result<(Vec<u8>, Vec<u8>, String)> {
    let cert_path = ctx.dir.join(CERT_FILE);
    let key_path = ctx.dir.join(KEY_FILE);
    if let (Ok(cert), Ok(key)) = (std::fs::read(&cert_path), crate::secure_file::read(&key_path)) {
        if let Some(der) = first_cert_der(&cert) {
            return Ok((cert, key, mist_signaling_client::cert_fingerprint(&der)));
        }
    }
    let name = format!("mist-{}", &ctx.identity.device_id().to_string()[..12]);
    let generated = rcgen::generate_simple_self_signed(vec![name, "localhost".into()])?;
    let cert = generated.cert.pem().into_bytes();
    let key = generated.key_pair.serialize_pem().into_bytes();
    crate::secure_file::write(&key_path, &key)?;
    std::fs::write(&cert_path, &cert)?;
    Ok((cert, key, mist_signaling_client::cert_fingerprint(generated.cert.der())))
}

fn first_cert_der(pem: &[u8]) -> Option<Vec<u8>> {
    rustls_pemfile::certs(&mut &pem[..]).next()?.ok().map(|c| c.to_vec())
}

/// IPv4 addresses of this PC on local networks (no loopback, no link-local).
pub fn lan_ips() -> Vec<Ipv4Addr> {
    let mut ips: Vec<Ipv4Addr> = if_addrs::get_if_addrs()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|i| match i.ip() {
            IpAddr::V4(v4) if !v4.is_loopback() && !v4.is_link_local() && !v4.is_unspecified() => Some(v4),
            _ => None,
        })
        .collect();
    // Private (home network) addresses first.
    ips.sort_by_key(|ip| (!ip.is_private(), *ip));
    ips.dedup();
    ips
}

/// Start the server on all interfaces. Fails if the port is taken.
pub fn start(ctx: &AppContext) -> anyhow::Result<BuiltinServer> {
    let port = ctx.config.host.server_port;
    let (cert, key, fingerprint) = load_or_create_cert(ctx)?;
    let listener = std::net::TcpListener::bind(SocketAddr::from((Ipv4Addr::UNSPECIFIED, port))).map_err(|e| {
        anyhow::anyhow!("cannot use port {port} for the built-in server ({e}); is another Mist host running? Change it under Settings → Host")
    })?;
    let config = mist_signaling_server::Config {
        bind: listener.local_addr()?,
        stun_urls: DEFAULT_STUN.iter().map(|s| s.to_string()).collect(),
        ..Default::default()
    };
    let state = mist_signaling_server::AppState::new(config);
    let task = tokio::spawn(async move {
        if let Err(e) = mist_signaling_server::serve_with_pem(state, listener, cert, key).await {
            tracing::error!("built-in signaling server stopped: {e}");
        }
    });
    let lan_addresses = lan_ips().into_iter().map(|ip| format!("{ip}:{port}")).collect();
    tracing::info!(port, %fingerprint, "built-in signaling server listening");
    Ok(BuiltinServer { info: ServerInfo { port, fingerprint, lan_addresses }, task })
}
