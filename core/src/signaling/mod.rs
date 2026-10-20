//! Signaling client: connects over WebSocket/TLS, answers the server's
//! Ed25519 challenge, and exposes typed events. Reconnection is decided by
//! the caller (see `mist-session-manager::reconnect`).

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use crate::protocol::identity::{server_auth_message, DeviceId, DeviceIdentity, PublicKeyB64};
use crate::protocol::peer::SignedEnvelope;
use crate::protocol::signaling::{ClientToServer, ErrorCode, IceServer, Role, ServerToClient};
use crate::protocol::{decode, encode, MAX_SIGNALING_MESSAGE_BYTES, PROTOCOL_VERSION};
use futures::{SinkExt, StreamExt};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;
use tokio_tungstenite::tungstenite::Message;

#[derive(Clone, Debug)]
pub struct SignalingConfig {
    pub url: String,
    pub device_name: String,
    pub role: Role,
    pub access_token: Option<String>,
    /// Extra PEM CA certificate(s) to trust (self-hosted servers).
    pub extra_ca_pem: Option<PathBuf>,
    /// SHA-256 of a server certificate to accept even though no CA vouches
    /// for it (the self-signed certificate of a host's built-in server,
    /// trusted on first use). Lowercase hex; see [`cert_fingerprint`].
    pub pinned_cert_sha256: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum SignalingError {
    #[error("invalid signaling URL: {0}")]
    BadUrl(String),
    #[error("cannot reach signaling server: {0}")]
    Connect(String),
    #[error("TLS configuration error: {0}")]
    Tls(String),
    /// The server presented a certificate that neither a CA nor a pin
    /// vouches for. The caller may show the fingerprint and pin it.
    #[error("the signaling server's certificate is not trusted yet (fingerprint {})", format_fingerprint(.fingerprint))]
    UntrustedCertificate { fingerprint: String },
    #[error("signaling server rejected us: {0}")]
    Rejected(String),
    #[error("protocol error: {0}")]
    Protocol(String),
    #[error("signaling connection closed")]
    Closed,
}

pub type Result<T> = std::result::Result<T, SignalingError>;

#[derive(Debug)]
pub enum SignalingEvent {
    PairRequest { lookup: String, from: DeviceId, from_public_key: PublicKeyB64, envelope: SignedEnvelope },
    Relay { from: DeviceId, from_public_key: PublicKeyB64, envelope: SignedEnvelope },
    PairingRegistered { lookup: String, expires_in_secs: u32 },
    Presence { online: Vec<DeviceId>, offline: Vec<DeviceId> },
    ServerError { code: ErrorCode, message: String },
    Disconnected { reason: String },
}

/// Handle used to send messages; cloneable.
#[derive(Clone)]
pub struct SignalingClient {
    tx: mpsc::Sender<ClientToServer>,
    pub ice_servers: Vec<IceServer>,
}

impl SignalingClient {
    pub async fn send(&self, msg: ClientToServer) -> Result<()> {
        self.tx.send(msg).await.map_err(|_| SignalingError::Closed)
    }

    pub fn is_closed(&self) -> bool {
        self.tx.is_closed()
    }
}

/// Only `wss://` is accepted, except `ws://` to a loopback address.
pub fn check_url(url: &str) -> Result<url::Url> {
    let u = url::Url::parse(url).map_err(|e| SignalingError::BadUrl(e.to_string()))?;
    match u.scheme() {
        "wss" => Ok(u),
        "ws" => {
            let loopback = match u.host() {
                Some(url::Host::Domain(d)) => d == "localhost",
                Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
                Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
                None => false,
            };
            if loopback {
                Ok(u)
            } else {
                Err(SignalingError::BadUrl("unencrypted ws:// is only allowed to localhost; use wss://".into()))
            }
        }
        s => Err(SignalingError::BadUrl(format!("unsupported scheme `{s}`"))),
    }
}

/// SHA-256 of a DER certificate, lowercase hex.
pub fn cert_fingerprint(der: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(Sha256::digest(der))
}

/// Groups of four upper-case hex digits, for people to compare.
pub fn format_fingerprint(hex: &str) -> String {
    hex.to_ascii_uppercase()
        .as_bytes()
        .chunks(4)
        .map(|c| String::from_utf8_lossy(c).into_owned())
        .collect::<Vec<_>>()
        .join(" ")
}

/// Accepts what the CA store accepts, or exactly the pinned certificate.
/// Remembers the certificate it refused so the caller can offer to pin it.
#[derive(Debug)]
struct PinningVerifier {
    webpki: Option<Arc<rustls::client::WebPkiServerVerifier>>,
    pin: Option<String>,
    refused: Arc<std::sync::Mutex<Option<String>>>,
    provider: Arc<rustls::crypto::CryptoProvider>,
}

impl rustls::client::danger::ServerCertVerifier for PinningVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &rustls::pki_types::CertificateDer<'_>,
        intermediates: &[rustls::pki_types::CertificateDer<'_>],
        server_name: &rustls::pki_types::ServerName<'_>,
        ocsp: &[u8],
        now: rustls::pki_types::UnixTime,
    ) -> std::result::Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        let fp = cert_fingerprint(end_entity.as_ref());
        if self.pin.as_deref() == Some(fp.as_str()) {
            return Ok(rustls::client::danger::ServerCertVerified::assertion());
        }
        let webpki = match &self.webpki {
            Some(v) => v.verify_server_cert(end_entity, intermediates, server_name, ocsp, now),
            None => Err(rustls::Error::General("no trusted CA certificates".into())),
        };
        if webpki.is_err() {
            *self.refused.lock().unwrap_or_else(|e| e.into_inner()) = Some(fp);
        }
        webpki
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &rustls::pki_types::CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> std::result::Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(message, cert, dss, &self.provider.signature_verification_algorithms)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &rustls::pki_types::CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> std::result::Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &self.provider.signature_verification_algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.provider.signature_verification_algorithms.supported_schemes()
    }
}

fn tls_config(
    extra_ca: Option<&PathBuf>,
    pin: Option<&str>,
    refused: Arc<std::sync::Mutex<Option<String>>>,
) -> Result<Arc<rustls::ClientConfig>> {
    let mut roots = rustls::RootCertStore::empty();
    let native = rustls_native_certs::load_native_certs();
    for e in &native.errors {
        tracing::debug!("native certificate load error: {e}");
    }
    for cert in native.certs {
        let _ = roots.add(cert);
    }
    if let Some(path) = extra_ca {
        let pem = std::fs::read(path).map_err(|e| SignalingError::Tls(format!("{}: {e}", path.display())))?;
        let mut added = 0;
        for cert in rustls_pemfile::certs(&mut pem.as_slice()) {
            let cert = cert.map_err(|e| SignalingError::Tls(e.to_string()))?;
            roots.add(cert).map_err(|e| SignalingError::Tls(e.to_string()))?;
            added += 1;
        }
        if added == 0 {
            return Err(SignalingError::Tls(format!("no certificates in {}", path.display())));
        }
    }
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let webpki = if roots.is_empty() {
        None
    } else {
        Some(
            rustls::client::WebPkiServerVerifier::builder_with_provider(Arc::new(roots), provider.clone())
                .build()
                .map_err(|e| SignalingError::Tls(e.to_string()))?,
        )
    };
    let verifier =
        PinningVerifier { webpki, pin: pin.map(|p| p.to_ascii_lowercase()), refused, provider: provider.clone() };
    let cfg = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|e| SignalingError::Tls(e.to_string()))?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(verifier))
        .with_no_client_auth();
    Ok(Arc::new(cfg))
}

/// Connect, authenticate and spawn the I/O task.
pub async fn connect(
    cfg: &SignalingConfig,
    identity: Arc<DeviceIdentity>,
) -> Result<(SignalingClient, mpsc::Receiver<SignalingEvent>)> {
    let url = check_url(&cfg.url)?;
    let ws_url = url.as_str().trim_end_matches('/').to_owned();
    let ws_url = if url.path() == "/" || url.path().is_empty() { format!("{ws_url}/ws") } else { ws_url };
    let refused = Arc::new(std::sync::Mutex::new(None));
    let connector = if url.scheme() == "wss" {
        Some(tokio_tungstenite::Connector::Rustls(tls_config(
            cfg.extra_ca_pem.as_ref(),
            cfg.pinned_cert_sha256.as_deref(),
            refused.clone(),
        )?))
    } else {
        None
    };
    let ws_cfg = WebSocketConfig::default()
        .max_message_size(Some(MAX_SIGNALING_MESSAGE_BYTES))
        .max_frame_size(Some(MAX_SIGNALING_MESSAGE_BYTES));
    let (ws, _) = tokio::time::timeout(
        Duration::from_secs(10),
        tokio_tungstenite::connect_async_tls_with_config(ws_url.as_str(), Some(ws_cfg), false, connector),
    )
    .await
    .map_err(|_| SignalingError::Connect("timed out".into()))?
    .map_err(|e| match refused.lock().unwrap_or_else(|e| e.into_inner()).take() {
        Some(fingerprint) => SignalingError::UntrustedCertificate { fingerprint },
        None => SignalingError::Connect(e.to_string()),
    })?;
    let (mut sink, mut stream) = ws.split();

    async fn recv(
        stream: &mut (impl futures::Stream<Item = std::result::Result<Message, tokio_tungstenite::tungstenite::Error>>
                  + Unpin),
    ) -> Result<ServerToClient> {
        loop {
            match stream.next().await {
                Some(Ok(Message::Text(t))) => {
                    return decode(t.as_bytes(), MAX_SIGNALING_MESSAGE_BYTES)
                        .map_err(|e| SignalingError::Protocol(e.to_string()))
                }
                Some(Ok(Message::Close(f))) => {
                    return Err(SignalingError::Rejected(f.map(|f| f.reason.to_string()).unwrap_or_default()))
                }
                Some(Ok(_)) => continue,
                Some(Err(e)) => return Err(SignalingError::Connect(e.to_string())),
                None => return Err(SignalingError::Closed),
            }
        }
    }

    let ServerToClient::Challenge { nonce, server_version } = recv(&mut stream).await? else {
        return Err(SignalingError::Protocol("expected challenge".into()));
    };
    tracing::debug!(%server_version, "signaling challenge received");
    let device_id = identity.device_id();
    let hello = ClientToServer::Hello {
        protocol_version: PROTOCOL_VERSION,
        device_id: device_id.clone(),
        public_key: identity.public_key_b64(),
        device_name: cfg.device_name.clone(),
        role: cfg.role,
        access_token: cfg.access_token.clone(),
    };
    let signature = identity.sign(&server_auth_message(&nonce, &device_id));
    for m in [hello, ClientToServer::AuthResponse { signature }] {
        sink.send(Message::Text(encode(&m).into())).await.map_err(|e| SignalingError::Connect(e.to_string()))?;
    }
    let ice_servers = match recv(&mut stream).await? {
        ServerToClient::Welcome { ice_servers, .. } => ice_servers,
        ServerToClient::Error { code, message } => {
            return Err(SignalingError::Rejected(format!("{code:?}: {message}")))
        }
        other => return Err(SignalingError::Protocol(format!("expected welcome, got {other:?}"))),
    };
    tracing::info!(url = %cfg.url, ice_servers = ice_servers.len(), "connected to signaling server");

    let (out_tx, mut out_rx) = mpsc::channel::<ClientToServer>(64);
    let (ev_tx, ev_rx) = mpsc::channel::<SignalingEvent>(64);
    tokio::spawn(async move {
        let mut keepalive = tokio::time::interval(Duration::from_secs(20));
        keepalive.tick().await;
        let reason = loop {
            tokio::select! {
                out = out_rx.recv() => match out {
                    Some(m) => {
                        if let Err(e) = sink.send(Message::Text(encode(&m).into())).await {
                            break format!("send failed: {e}");
                        }
                    }
                    None => {
                        let _ = sink.send(Message::Close(None)).await;
                        break "closed by application".into();
                    }
                },
                _ = keepalive.tick() => {
                    if let Err(e) = sink.send(Message::Text(encode(&ClientToServer::Ping {}).into())).await {
                        break format!("keepalive failed: {e}");
                    }
                }
                inc = recv(&mut stream) => match inc {
                    Ok(ServerToClient::Pong {}) => {}
                    Ok(ServerToClient::PairRequest { lookup, from, from_public_key, envelope }) => {
                        let _ = ev_tx.send(SignalingEvent::PairRequest { lookup, from, from_public_key, envelope }).await;
                    }
                    Ok(ServerToClient::Relay { from, from_public_key, envelope }) => {
                        let _ = ev_tx.send(SignalingEvent::Relay { from, from_public_key, envelope }).await;
                    }
                    Ok(ServerToClient::PairingRegistered { lookup, expires_in_secs }) => {
                        let _ = ev_tx.send(SignalingEvent::PairingRegistered { lookup, expires_in_secs }).await;
                    }
                    Ok(ServerToClient::Presence { online, offline }) => {
                        let _ = ev_tx.send(SignalingEvent::Presence { online, offline }).await;
                    }
                    Ok(ServerToClient::Error { code, message }) => {
                        tracing::warn!(?code, %message, "signaling server error");
                        let _ = ev_tx.send(SignalingEvent::ServerError { code, message }).await;
                    }
                    Ok(other) => tracing::warn!("unexpected signaling message: {other:?}"),
                    Err(SignalingError::Protocol(e)) => tracing::warn!("ignoring invalid signaling message: {e}"),
                    Err(e) => break e.to_string(),
                }
            }
        };
        tracing::info!("signaling connection ended: {reason}");
        let _ = ev_tx.send(SignalingEvent::Disconnected { reason }).await;
    });
    Ok((SignalingClient { tx: out_tx, ice_servers }, ev_rx))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_policy() {
        assert!(check_url("wss://signal.example.org").is_ok());
        assert!(check_url("ws://127.0.0.1:8443").is_ok());
        assert!(check_url("ws://localhost:8443").is_ok());
        assert!(check_url("ws://[::1]:8443").is_ok());
        assert!(check_url("ws://192.168.1.10:8443").is_err());
        assert!(check_url("http://example.org").is_err());
        assert!(check_url("not a url").is_err());
    }
}
