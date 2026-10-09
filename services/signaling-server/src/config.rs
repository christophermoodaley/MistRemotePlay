//! Configuration from environment variables (see docs/networking.md).

use std::net::SocketAddr;
use std::time::Duration;

use anyhow::{bail, Context};
use mist_protocol::signaling::IceServer;

#[derive(Clone, Debug)]
pub struct Config {
    pub bind: SocketAddr,
    pub tls_cert: Option<String>,
    pub tls_key: Option<String>,
    /// Permit plain ws:// on a non-loopback address (only behind a TLS proxy).
    pub allow_insecure: bool,
    /// Optional shared secret every device must present (private servers).
    pub access_token: Option<String>,
    pub stun_urls: Vec<String>,
    pub turn_urls: Vec<String>,
    pub turn_secret: Option<String>,
    pub turn_ttl: Duration,
    pub max_connections: usize,
    pub max_connections_per_ip: usize,
    pub connection_ttl: Duration,
    pub idle_timeout: Duration,
    pub auth_timeout: Duration,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            bind: "127.0.0.1:8443".parse().expect("static address"),
            tls_cert: None,
            tls_key: None,
            allow_insecure: false,
            access_token: None,
            stun_urls: Vec::new(),
            turn_urls: Vec::new(),
            turn_secret: None,
            turn_ttl: Duration::from_secs(3600),
            max_connections: 2000,
            max_connections_per_ip: 32,
            connection_ttl: Duration::from_secs(24 * 3600),
            idle_timeout: Duration::from_secs(90),
            auth_timeout: Duration::from_secs(10),
        }
    }
}

fn list(var: &str) -> Vec<String> {
    std::env::var(var)
        .map(|v| v.split(',').map(|s| s.trim().to_owned()).filter(|s| !s.is_empty()).collect())
        .unwrap_or_default()
}

fn opt(var: &str) -> Option<String> {
    std::env::var(var).ok().filter(|v| !v.is_empty())
}

fn secs(var: &str, default: Duration) -> anyhow::Result<Duration> {
    match opt(var) {
        Some(v) => Ok(Duration::from_secs(v.parse().with_context(|| format!("{var} must be a number of seconds"))?)),
        None => Ok(default),
    }
}

impl Config {
    pub fn from_env() -> anyhow::Result<Self> {
        let d = Config::default();
        let cfg = Config {
            bind: opt("MIST_SIGNAL_BIND").map(|b| b.parse()).transpose().context("MIST_SIGNAL_BIND")?.unwrap_or(d.bind),
            tls_cert: opt("MIST_TLS_CERT"),
            tls_key: opt("MIST_TLS_KEY"),
            allow_insecure: opt("MIST_ALLOW_INSECURE_WS").as_deref() == Some("1"),
            access_token: opt("MIST_ACCESS_TOKEN"),
            stun_urls: list("MIST_STUN_URLS"),
            turn_urls: list("MIST_TURN_URLS"),
            turn_secret: opt("MIST_TURN_SECRET"),
            turn_ttl: secs("MIST_TURN_TTL_SECS", d.turn_ttl)?,
            max_connections: opt("MIST_MAX_CONNECTIONS").map(|v| v.parse()).transpose()?.unwrap_or(d.max_connections),
            max_connections_per_ip: opt("MIST_MAX_CONNECTIONS_PER_IP")
                .map(|v| v.parse())
                .transpose()?
                .unwrap_or(d.max_connections_per_ip),
            connection_ttl: secs("MIST_CONNECTION_TTL_SECS", d.connection_ttl)?,
            idle_timeout: secs("MIST_IDLE_TIMEOUT_SECS", d.idle_timeout)?,
            auth_timeout: d.auth_timeout,
        };
        cfg.validate()?;
        Ok(cfg)
    }

    pub fn validate(&self) -> anyhow::Result<()> {
        if self.tls_cert.is_some() != self.tls_key.is_some() {
            bail!("MIST_TLS_CERT and MIST_TLS_KEY must be set together");
        }
        if self.tls_cert.is_none() && !self.bind.ip().is_loopback() && !self.allow_insecure {
            bail!(
                "refusing to serve plain ws:// on {}: set MIST_TLS_CERT/MIST_TLS_KEY, or set \
                 MIST_ALLOW_INSECURE_WS=1 only when a TLS-terminating reverse proxy is in front",
                self.bind
            );
        }
        if !self.turn_urls.is_empty() && self.turn_secret.is_none() {
            bail!("MIST_TURN_URLS requires MIST_TURN_SECRET (coturn use-auth-secret)");
        }
        for u in self.stun_urls.iter().chain(&self.turn_urls) {
            let probe = IceServer { urls: vec![u.clone()], username: None, credential: None };
            mist_protocol::Validate::validate(&probe).with_context(|| format!("invalid ICE url {u}"))?;
        }
        Ok(())
    }
}
