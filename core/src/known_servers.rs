//! Certificates of signaling servers the user chose to trust although no CA
//! vouches for them (a host's built-in server uses a self-signed
//! certificate). Keyed by `host:port`; read from disk on every use so the
//! desktop app and the streaming client process always agree.
//!
//! Trusting a server here is low-stakes by design: the signaling server
//! never sees media or input, pairing needs the host's approval, and every
//! message between devices is signed with their paired keys.

use std::collections::BTreeMap;
use std::path::Path;

const FILE: &str = "known-servers.json";

/// `host:port` of a signaling URL, the key pins are stored under.
pub fn server_key(url: &str) -> Option<String> {
    let u = url::Url::parse(url).ok()?;
    Some(format!("{}:{}", u.host_str()?, u.port_or_known_default()?))
}

/// Turn what a person types as a host address into a signaling URL:
/// `192.168.1.40`, `myhost.example.org:47900` or a full `wss://` URL.
pub fn normalize_address(input: &str) -> anyhow::Result<String> {
    let t = input.trim().trim_end_matches('/');
    anyhow::ensure!(!t.is_empty(), "enter the host's address");
    let with_scheme = if t.contains("://") { t.to_owned() } else { format!("wss://{t}") };
    let mut u = url::Url::parse(&with_scheme).map_err(|e| anyhow::anyhow!("not a valid address: {e}"))?;
    anyhow::ensure!(matches!(u.scheme(), "ws" | "wss"), "the address must start with wss://");
    anyhow::ensure!(u.host_str().is_some_and(|h| !h.is_empty()), "the address has no host name");
    if u.port().is_none() && !t.contains("://") {
        let _ = u.set_port(Some(crate::config::DEFAULT_SERVER_PORT));
    }
    Ok(u.as_str().trim_end_matches('/').to_owned())
}

fn load(dir: &Path) -> BTreeMap<String, String> {
    std::fs::read(dir.join(FILE)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

/// The pinned SHA-256 (lowercase hex) for this server, if any.
pub fn pin_for(dir: &Path, url: &str) -> Option<String> {
    load(dir).get(&server_key(url)?).cloned()
}

/// Pin `fingerprint` for the server at `url` (replacing an older pin).
pub fn trust(dir: &Path, url: &str, fingerprint: &str) -> anyhow::Result<()> {
    let key = server_key(url).ok_or_else(|| anyhow::anyhow!("not a server URL: {url}"))?;
    let fp = fingerprint.chars().filter(|c| c.is_ascii_hexdigit()).collect::<String>().to_ascii_lowercase();
    anyhow::ensure!(fp.len() == 64, "a certificate fingerprint has 64 hex digits");
    let mut all = load(dir);
    all.insert(key, fp);
    std::fs::create_dir_all(dir)?;
    let tmp = dir.join(format!("{FILE}.tmp"));
    std::fs::write(&tmp, serde_json::to_vec_pretty(&all)?)?;
    std::fs::rename(tmp, dir.join(FILE))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_addresses() {
        assert_eq!(normalize_address("192.168.1.40").unwrap(), "wss://192.168.1.40:47900");
        assert_eq!(normalize_address(" host.example.org:5000 ").unwrap(), "wss://host.example.org:5000");
        assert_eq!(normalize_address("wss://signal.example.org").unwrap(), "wss://signal.example.org");
        assert_eq!(normalize_address("ws://127.0.0.1:8443/").unwrap(), "ws://127.0.0.1:8443");
        assert!(normalize_address("").is_err());
        assert!(normalize_address("https://x").is_err());
    }

    #[test]
    fn pins_by_host_and_port() {
        let dir = std::env::temp_dir().join(format!("mist-known-{}", std::process::id()));
        let fp = "ab".repeat(32);
        assert_eq!(pin_for(&dir, "wss://192.168.1.5:47900"), None);
        trust(&dir, "wss://192.168.1.5:47900", &fp.to_uppercase()).unwrap();
        assert_eq!(pin_for(&dir, "wss://192.168.1.5:47900/ws").as_deref(), Some(fp.as_str()));
        assert_eq!(pin_for(&dir, "wss://192.168.1.5:47901"), None);
        assert!(trust(&dir, "wss://x:1", "abc").is_err());
        // Spaces from the grouped display form are accepted.
        trust(&dir, "wss://h:1", &crate::signaling::format_fingerprint(&fp)).unwrap();
        assert_eq!(pin_for(&dir, "wss://h:1").as_deref(), Some(fp.as_str()));
        let _ = std::fs::remove_dir_all(dir);
    }
}
