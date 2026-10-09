//! Application core shared by the CLI and the desktop app.
//!
//! The UI never touches capture, injection or the network directly: it
//! drives a [`host::HostHandle`] or a client session through typed calls and
//! receives typed events.

pub mod client;
pub mod config;
pub mod discovery;
pub mod events;
pub mod host;
pub mod identity_store;
pub mod link;
pub mod report;
pub mod secure_file;

use std::path::PathBuf;
use std::sync::Arc;

use mist_protocol::identity::DeviceIdentity;
use mist_session_manager::trust::TrustStore;
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
}
