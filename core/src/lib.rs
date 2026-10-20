//! Mist Remote Play engine, shared by the desktop app and the `mist` CLI.
//!
//! The UI never touches capture, injection or the network directly: it
//! drives a [`host::HostHandle`] or a client session through typed calls and
//! receives typed events.
//!
//! Building blocks: [`protocol`] (wire formats, identity, pairing),
//! [`capture`], [`audio`], [`media`] (GStreamer + WebRTC), [`input`],
//! [`controller`], [`sessions`] (sessions, trust, bitrate), [`signaling`]
//! (client) and [`server`] (signaling server). Services built on them:
//! [`host`], [`client`] and the rest below.

pub mod audio;
pub mod capture;
pub mod controller;
pub mod diagnostics;
pub mod input;
pub mod media;
pub mod protocol;
pub mod server;
pub mod sessions;
pub mod signaling;

pub mod builtin_server;
pub mod client;
pub mod config;
pub mod discovery;
pub mod events;
pub mod host;
pub mod identity_store;
pub mod known_servers;
pub mod link;
pub mod reachability;
pub mod report;
pub mod secure_file;
pub mod updater;
pub mod upnp;

use std::path::PathBuf;
use std::sync::Arc;

use crate::protocol::identity::DeviceIdentity;
use crate::sessions::trust::TrustStore;
use parking_lot::Mutex;

pub use config::{AppConfig, Secrets};

/// Everything a host or client needs: configuration, identity, trust.
#[derive(Clone)]
pub struct AppContext {
    pub dir: PathBuf,
    pub config: AppConfig,
    pub secrets: Secrets,
    pub identity: Arc<DeviceIdentity>,
    pub trust: Arc<Mutex<TrustStore>>,
}

impl AppContext {
    pub fn load() -> anyhow::Result<Self> {
        Self::load_from(config::config_dir())
    }

    pub fn load_from(dir: PathBuf) -> anyhow::Result<Self> {
        std::fs::create_dir_all(&dir)?;
        let config = AppConfig::load(&dir)?;
        let secrets = Secrets::load(&dir)?;
        let identity = identity_store::load_or_create(&dir)?;
        let trust = TrustStore::load(&dir.join("paired-devices.json"))?;
        Ok(AppContext { dir, config, secrets, identity, trust: Arc::new(Mutex::new(trust)) })
    }

    pub fn save_config(&self) -> anyhow::Result<()> {
        self.config.save(&self.dir)
    }

    /// Connection settings for the signaling server at `url`, including a
    /// certificate pin the user accepted earlier.
    pub fn signaling_config(
        &self,
        url: &str,
        role: crate::protocol::signaling::Role,
    ) -> crate::signaling::SignalingConfig {
        crate::signaling::SignalingConfig {
            url: url.to_owned(),
            device_name: self.config.device_name.clone(),
            role,
            access_token: self.secrets.signaling_access_token.clone(),
            extra_ca_pem: self.config.network.extra_ca_pem.clone(),
            pinned_cert_sha256: known_servers::pin_for(&self.dir, url),
        }
    }
}
