//! Persistent configuration (JSON) and secrets (separate, permission-
//! restricted file). Paths can be overridden with `MIST_CONFIG_DIR`, which
//! also allows running several instances on one machine for testing.

use std::path::{Path, PathBuf};

use crate::controller::ControllerMapping;
use crate::input::InputBackendKind;
use crate::protocol::settings::StreamSettings;
use crate::protocol::signaling::IceServer;
use serde::{Deserialize, Serialize};

/// Default TCP port of the host's built-in signaling server.
pub const DEFAULT_SERVER_PORT: u16 = 47900;

pub fn config_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("MIST_CONFIG_DIR") {
        return PathBuf::from(dir);
    }
    directories::ProjectDirs::from("org", "Mist", "MistRemotePlay")
        .map(|d| d.config_dir().to_path_buf())
        .unwrap_or_else(|| PathBuf::from(".mist"))
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct NetworkConfig {
    pub signaling_url: String,
    /// PEM file with additional CA certificates for a self-hosted server.
    pub extra_ca_pem: Option<PathBuf>,
    /// Extra STUN/TURN servers used in addition to the server-provided ones.
    pub ice_servers: Vec<IceServer>,
    /// Force all media through TURN (testing, or to hide the host's IP).
    pub relay_only: bool,
    pub lan_discovery: bool,
}

impl Default for NetworkConfig {
    fn default() -> Self {
        NetworkConfig {
            signaling_url: "ws://127.0.0.1:8443".into(),
            extra_ca_pem: None,
            ice_servers: Vec::new(),
            relay_only: false,
            lan_discovery: true,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AudioChoice {
    /// Default playback device (loopback).
    Default,
    Device(String),
    /// Only these apps (executable names such as `game.exe`).
    Apps(Vec<String>),
    /// Everything except this app, e.g. `Discord.exe` so friends in a call
    /// don't hear themselves.
    Except(String),
    /// Synthetic tone (tests only).
    TestTone,
    Off,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ControllerChoice {
    Platform,
    Mock,
    Off,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct HostConfig {
    /// Display id from capture discovery; `None` = primary.
    pub display_id: Option<String>,
    /// Force a capture backend (`test_pattern` for tests).
    pub capture: Option<crate::capture::CaptureKind>,
    pub audio: AudioChoice,
    pub input_backend: InputBackendKind,
    pub controllers: ControllerChoice,
    /// Input permission when a session starts.
    pub input_allowed: bool,
    /// Upper limits the host will grant regardless of client requests.
    pub max_width: u32,
    pub max_height: u32,
    pub max_fps: u32,
    pub max_bitrate_kbps: u32,
    /// Preferred encoder element (`None` = automatic, hardware first).
    pub encoder: Option<String>,
    /// Approve session requests from already-paired devices without a
    /// prompt. Off by default; pairing itself always needs approval.
    pub trust_paired_sessions: bool,
    /// Run the signaling server inside the host (default), so other devices
    /// connect to this PC directly. Off: use `network.signaling_url`.
    pub builtin_server: bool,
    /// TCP port of the built-in server (forward it on the router to host
    /// over the internet).
    pub server_port: u16,
    /// How many people may be connected at once (1..=[`MAX_CLIENTS`]).
    pub max_clients: u8,
    /// Show a small notice on this PC's screen while someone is connected.
    /// It is excluded from screen capture, so viewers never see it.
    pub connection_overlay: bool,
    /// Open the built-in server's port on the router with UPnP while
    /// hosting, so no manual port forward is needed.
    pub upnp: bool,
    /// While streaming, switch this PC's display to the player's screen
    /// size (or the closest mode with the same shape), so the picture
    /// fills their monitor as if it were plugged in here. Restored when
    /// the last person leaves.
    pub match_client_display: bool,
}

/// Upper bound for [`HostConfig::max_clients`]: every viewer gets its own
/// copy of the stream, so the host's upload bandwidth runs out first.
pub const MAX_CLIENTS: u8 = 8;

impl Default for HostConfig {
    fn default() -> Self {
        HostConfig {
            display_id: None,
            capture: None,
            audio: AudioChoice::Default,
            input_backend: InputBackendKind::Auto,
            controllers: ControllerChoice::Platform,
            input_allowed: true,
            max_width: 3840,
            max_height: 2160,
            max_fps: 120,
            max_bitrate_kbps: 80_000,
            encoder: None,
            trust_paired_sessions: false,
            builtin_server: true,
            server_port: DEFAULT_SERVER_PORT,
            max_clients: MAX_CLIENTS,
            connection_overlay: true,
            upnp: true,
            match_client_display: true,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ClientConfig {
    pub stream: StreamSettings,
    pub decoder: Option<String>,
    pub jitter_buffer_ms: u32,
    pub fullscreen: bool,
    pub volume: f64,
    pub muted: bool,
    pub show_stats_overlay: bool,
    pub controllers_enabled: bool,
    pub controller_mapping: ControllerMapping,
    /// Lock the pointer to the window and send relative motion (games).
    pub capture_mouse: bool,
    /// How the streaming window draws video.
    pub renderer: Renderer,
}

/// Streaming-window video renderer.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Renderer {
    /// GPU video sink where supported (Direct3D 11 on Windows; X11 with
    /// OpenGL, also through XWayland, when measured to be fast), else
    /// software.
    #[default]
    Auto,
    /// GPU video sink whenever it works, even if slow (e.g. software
    /// OpenGL); for testing.
    Gpu,
    /// Always convert and scale on the CPU (softbuffer).
    Software,
}

impl Default for ClientConfig {
    fn default() -> Self {
        ClientConfig {
            stream: StreamSettings::default(),
            decoder: None,
            jitter_buffer_ms: 40,
            fullscreen: false,
            volume: 1.0,
            muted: false,
            show_stats_overlay: false,
            controllers_enabled: true,
            controller_mapping: ControllerMapping::default(),
            capture_mouse: false,
            renderer: Renderer::Auto,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AppConfig {
    pub device_name: String,
    pub network: NetworkConfig,
    pub host: HostConfig,
    pub client: ClientConfig,
    pub updates: UpdateConfig,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct UpdateConfig {
    /// Look for a newer release when the app starts.
    pub auto_check: bool,
}

impl Default for UpdateConfig {
    fn default() -> Self {
        UpdateConfig { auto_check: true }
    }
}

impl Default for AppConfig {
    fn default() -> Self {
        let name = gethostname::gethostname()
            .to_string_lossy()
            .chars()
            .filter(|c| !c.is_control())
            .take(64)
            .collect::<String>();
        AppConfig {
            device_name: if name.is_empty() { "Mist device".into() } else { name },
            network: NetworkConfig::default(),
            host: HostConfig::default(),
            client: ClientConfig::default(),
            updates: UpdateConfig::default(),
        }
    }
}

impl AppConfig {
    pub fn path(dir: &Path) -> PathBuf {
        dir.join("config.json")
    }

    pub fn load(dir: &Path) -> anyhow::Result<Self> {
        match std::fs::read(Self::path(dir)) {
            Ok(b) => Ok(serde_json::from_slice(&b)?),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(AppConfig::default()),
            Err(e) => Err(e.into()),
        }
    }

    pub fn save(&self, dir: &Path) -> anyhow::Result<()> {
        self.validate()?;
        std::fs::create_dir_all(dir)?;
        let tmp = dir.join("config.json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(self)?)?;
        std::fs::rename(tmp, Self::path(dir))?;
        Ok(())
    }

    pub fn validate(&self) -> anyhow::Result<()> {
        use crate::protocol::Validate;
        self.client.stream.validate().map_err(|e| anyhow::anyhow!("stream settings: {e}"))?;
        self.client.controller_mapping.validate().map_err(|e| anyhow::anyhow!("controller mapping: {e}"))?;
        for s in &self.network.ice_servers {
            s.validate().map_err(|e| anyhow::anyhow!("ICE server: {e}"))?;
        }
        crate::signaling::check_url(&self.network.signaling_url)?;
        let apps = match &self.host.audio {
            AudioChoice::Apps(apps) => apps.as_slice(),
            AudioChoice::Except(app) => std::slice::from_ref(app),
            _ => &[],
        };
        if matches!(self.host.audio, AudioChoice::Apps(_)) && !(1..=8).contains(&apps.len()) {
            anyhow::bail!("choose 1-8 apps whose sound is streamed");
        }
        if apps.iter().any(|a| a.trim().is_empty() || a.len() > 260 || a.contains(['/', '\\'])) {
            anyhow::bail!("audio apps are given by program file name, such as Discord.exe");
        }
        if self.device_name.trim().is_empty() || self.device_name.len() > 64 {
            anyhow::bail!("device name must be 1-64 characters");
        }
        if !(1..=MAX_CLIENTS).contains(&self.host.max_clients) {
            anyhow::bail!("people at once must be 1-{MAX_CLIENTS}");
        }
        Ok(())
    }
}

/// Secrets live in their own file with owner-only permissions.
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Secrets {
    pub signaling_access_token: Option<String>,
    /// Read-only GitHub token for checking and downloading updates from a
    /// private repository (see updater.rs).
    pub github_token: Option<String>,
}

impl std::fmt::Debug for Secrets {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Secrets(<redacted>)")
    }
}

impl Secrets {
    pub fn load(dir: &Path) -> anyhow::Result<Self> {
        match crate::secure_file::read(&dir.join("secrets.json")) {
            Ok(b) => Ok(serde_json::from_slice(&b)?),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Secrets::default()),
            Err(e) => Err(e.into()),
        }
    }

    pub fn save(&self, dir: &Path) -> anyhow::Result<()> {
        crate::secure_file::write(&dir.join("secrets.json"), &serde_json::to_vec(self)?)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_audio_choices() {
        // The JSON shapes the desktop UI writes.
        let except: AudioChoice = serde_json::from_str(r#"{"except":"Discord.exe"}"#).unwrap();
        assert_eq!(except, AudioChoice::Except("Discord.exe".into()));
        let apps: AudioChoice = serde_json::from_str(r#"{"apps":["game.exe","Spotify.exe"]}"#).unwrap();
        assert_eq!(apps, AudioChoice::Apps(vec!["game.exe".into(), "Spotify.exe".into()]));
        let mut c = AppConfig::default();
        c.host.audio = except;
        c.validate().unwrap();
        c.host.audio = AudioChoice::Apps(Vec::new());
        assert!(c.validate().is_err());
        c.host.audio = AudioChoice::Except(r"C:\Apps\Discord.exe".into());
        assert!(c.validate().is_err());
    }

    #[test]
    fn default_config_is_valid_and_roundtrips() {
        let dir = std::env::temp_dir().join(format!("mist-cfg-{}", std::process::id()));
        let cfg = AppConfig::default();
        cfg.save(&dir).unwrap();
        assert_eq!(AppConfig::load(&dir).unwrap(), cfg);
        let mut bad = cfg.clone();
        bad.network.signaling_url = "ws://10.0.0.1:8443".into();
        assert!(bad.save(&dir).is_err());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn secrets_debug_redacted() {
        let s = Secrets { signaling_access_token: Some("topsecret".into()), github_token: Some("ghp_secret".into()) };
        assert!(!format!("{s:?}").contains("topsecret"));
        assert!(!format!("{s:?}").contains("ghp_secret"));
    }
}
