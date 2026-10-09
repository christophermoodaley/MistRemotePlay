//! Ephemeral TURN credentials (the "TURN REST API" scheme implemented by
//! coturn's `use-auth-secret`): username = "<expiry>:<device>", password =
//! base64(HMAC-SHA1(secret, username)). The shared secret never leaves the
//! server; credentials expire after the configured TTL.

use base64::{engine::general_purpose::STANDARD as B64, Engine};
use hmac::{Hmac, Mac};
use mist_protocol::identity::DeviceId;
use mist_protocol::signaling::IceServer;
use sha1::Sha1;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub fn credentials(secret: &str, device: &DeviceId, ttl: Duration, now: SystemTime) -> (String, String) {
    let expiry = now.duration_since(UNIX_EPOCH).unwrap_or_default().as_secs() + ttl.as_secs();
    let username = format!("{expiry}:{device}");
    let mut mac = Hmac::<Sha1>::new_from_slice(secret.as_bytes()).expect("HMAC accepts any key length");
    mac.update(username.as_bytes());
    (username, B64.encode(mac.finalize().into_bytes()))
}

pub fn ice_servers(cfg: &crate::config::Config, device: &DeviceId) -> Vec<IceServer> {
    let mut out = Vec::new();
    if !cfg.stun_urls.is_empty() {
        out.push(IceServer { urls: cfg.stun_urls.clone(), username: None, credential: None });
    }
    if let (false, Some(secret)) = (cfg.turn_urls.is_empty(), &cfg.turn_secret) {
        let (u, p) = credentials(secret, device, cfg.turn_ttl, SystemTime::now());
        out.push(IceServer { urls: cfg.turn_urls.clone(), username: Some(u), credential: Some(p) });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_coturn_reference_vector() {
        // Reference computed with: printf '%s' "1700003600:<id>" | openssl dgst -sha1 -hmac secret -binary | base64
        let id = DeviceId::parse("0123456789abcdef0123456789abcdef").unwrap();
        let (u, p) =
            credentials("secret", &id, Duration::from_secs(3600), UNIX_EPOCH + Duration::from_secs(1_700_000_000));
        assert_eq!(u, "1700003600:0123456789abcdef0123456789abcdef");
        assert_eq!(p, "N006sFHv8o9l0OwScZFBdeCTEOw=");
        let (_, p2) =
            credentials("other", &id, Duration::from_secs(3600), UNIX_EPOCH + Duration::from_secs(1_700_000_000));
        assert_ne!(p, p2);
    }
}
