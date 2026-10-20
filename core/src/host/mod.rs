//! Host service: pairing, session consent, media and input for up to
//! `host.max_clients` people at once.
//!
//! Everyone watches the same stream: one capture and encoder
//! ([`HostStream`]) feeds a [`HostViewer`] (peer connection) per session. The
//! host decides per person whether they play (gamepads, and keyboard/mouse
//! if allowed) or only watch. The stream starts with the first approved
//! session and stops when the last one leaves.
//!
//! All state lives in a single task (`HostLoop`); UIs talk to it through a
//! [`HostHandle`] (commands + broadcast events + a read-only snapshot).

mod input_worker;

use std::collections::HashMap;
use std::sync::{Arc, Weak};
use std::time::{Duration, Instant};

use crate::audio::AudioSourceSpec;
use crate::capture::CaptureKind;
use crate::input::{ClientId, InputPermissions, TargetRect};
use crate::media::{
    ConnectionState, DataChannelKind, HostStream, HostStreamConfig, HostViewer, IceConfig, MediaTransport,
    TransportEvent, TransportStats,
};
use crate::protocol::control::{ClientStats, ControlMessage};
use crate::protocol::identity::{random_hex, DeviceId, PublicKeyB64};
use crate::protocol::pairing::PairingCode;
use crate::protocol::peer::{now_ms, PeerMessage, ReplayGuard, SdpKind, SignedEnvelope};
use crate::protocol::settings::StreamSettings;
use crate::protocol::signaling::{ClientToServer, Role};
use crate::protocol::{decode, encode, Validate, MAX_DATA_CHANNEL_MESSAGE_BYTES};
use crate::sessions::bitrate::{BitrateController, Decision, NetworkSample};
use crate::sessions::pairing::{PairingCheck, PairingCodes, DEFAULT_CODE_TTL};
use crate::sessions::session::DEFAULT_MAX_SESSION;
use crate::sessions::trust::PairedDevice;
use crate::sessions::{EndReason, Session, SessionError, SessionInfo, SessionManager, SessionRole, SessionState};
use crate::signaling::SignalingEvent;
use parking_lot::Mutex;
use serde::Serialize;
use tokio::sync::{broadcast, mpsc, oneshot};

use crate::config::AudioChoice;
use crate::events::{HostEvent, HostStatsEvent};
use crate::link::{self, LinkEvent, LinkHandle};
use crate::AppContext;
use input_worker::{InputWorker, WorkerStats};

const PAIRING_APPROVAL_TIMEOUT: Duration = Duration::from_secs(120);
const RESUME_WINDOW: Duration = Duration::from_secs(60);
const RECONNECT_GRACE: Duration = Duration::from_secs(15);
/// The client pings every second; this much silence means the path is
/// broken even if ICE has not noticed yet.
const PEER_SILENCE: Duration = Duration::from_secs(3);

#[derive(Debug)]
enum HostCmd {
    GeneratePairingCode,
    CancelPairing,
    DecidePairing { request_id: String, approve: bool },
    DecideSession { session_id: String, approve: bool, role: SessionRole, reason: String },
    EndSession { session_id: String },
    SetRole { session_id: String, role: SessionRole },
    SetKeyboardMouse { session_id: String, allowed: bool },
    SetInputAllowed(bool),
    DisconnectAll,
    Revoke(DeviceId),
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct HostSnapshot {
    pub device_id: String,
    pub device_name: String,
    pub signaling_connected: bool,
    pub signaling_detail: String,
    pub pairing_code: Option<String>,
    pub pairing_expires_unix_ms: Option<u64>,
    pub pending_pairings: Vec<PendingPairingInfo>,
    pub sessions: Vec<SessionInfo>,
    /// Host-wide remote input switch.
    pub input_allowed: bool,
    /// How many people may be connected at once.
    pub max_clients: u8,
    pub paired_devices: Vec<PairedDevice>,
    /// The built-in signaling server, when this host runs one.
    pub server: Option<crate::builtin_server::ServerInfo>,
}

#[derive(Clone, Debug, Serialize)]
pub struct PendingPairingInfo {
    pub request_id: String,
    pub client_name: String,
    pub fingerprint: String,
}

/// Cloneable control surface for UIs.
#[derive(Clone)]
pub struct HostHandle {
    cmd: mpsc::UnboundedSender<HostCmd>,
    events: broadcast::Sender<HostEvent>,
    snapshot: Arc<Mutex<HostSnapshot>>,
    _alive: Arc<oneshot::Sender<()>>,
    _server: Option<Arc<crate::builtin_server::BuiltinServer>>,
}

impl HostHandle {
    pub fn subscribe(&self) -> broadcast::Receiver<HostEvent> {
        self.events.subscribe()
    }
    pub fn snapshot(&self) -> HostSnapshot {
        self.snapshot.lock().clone()
    }
    pub fn generate_pairing_code(&self) {
        let _ = self.cmd.send(HostCmd::GeneratePairingCode);
    }
    pub fn cancel_pairing(&self) {
        let _ = self.cmd.send(HostCmd::CancelPairing);
    }
    pub fn decide_pairing(&self, request_id: &str, approve: bool) -> Result<(), SessionError> {
        if !self.snapshot.lock().pending_pairings.iter().any(|p| p.request_id == request_id) {
            return Err(SessionError::NotFound);
        }
        let _ = self.cmd.send(HostCmd::DecidePairing { request_id: request_id.into(), approve });
        Ok(())
    }
    pub fn revoke_device(&self, id: DeviceId) {
        let _ = self.cmd.send(HostCmd::Revoke(id));
    }
    fn has_session(&self, id: &str) -> bool {
        self.snapshot.lock().sessions.iter().any(|s| s.session_id == id)
    }
    fn send_for(&self, session_id: &str, cmd: HostCmd) -> Result<(), SessionError> {
        if !self.has_session(session_id) {
            return Err(SessionError::NotFound);
        }
        let _ = self.cmd.send(cmd);
        Ok(())
    }
}

impl SessionManager for HostHandle {
    fn sessions(&self) -> Vec<SessionInfo> {
        self.snapshot.lock().sessions.clone()
    }
    fn approve_as(&self, session_id: &str, role: SessionRole) -> Result<(), SessionError> {
        self.send_for(
            session_id,
            HostCmd::DecideSession { session_id: session_id.into(), approve: true, role, reason: String::new() },
        )
    }
    fn reject(&self, session_id: &str, reason: &str) -> Result<(), SessionError> {
        self.send_for(
            session_id,
            HostCmd::DecideSession {
                session_id: session_id.into(),
                approve: false,
                role: SessionRole::Watch,
                reason: reason.into(),
            },
        )
    }
    fn end(&self, session_id: &str) -> Result<(), SessionError> {
        self.send_for(session_id, HostCmd::EndSession { session_id: session_id.into() })
    }
    fn set_role(&self, session_id: &str, role: SessionRole) -> Result<(), SessionError> {
        self.send_for(session_id, HostCmd::SetRole { session_id: session_id.into(), role })
    }
    fn set_keyboard_mouse(&self, session_id: &str, allowed: bool) -> Result<(), SessionError> {
        self.send_for(session_id, HostCmd::SetKeyboardMouse { session_id: session_id.into(), allowed })
    }
    fn set_input_allowed(&self, allowed: bool) {
        let _ = self.cmd.send(HostCmd::SetInputAllowed(allowed));
    }
    fn input_allowed(&self) -> bool {
        self.snapshot.lock().input_allowed
    }
    fn disconnect_all(&self) {
        let _ = self.cmd.send(HostCmd::DisconnectAll);
    }
}

struct PendingPairing {
    client_id: DeviceId,
    client_pk: PublicKeyB64,
    client_name: String,
    nonce: String,
    code: PairingCode,
    created: Instant,
}

enum Internal {
    StreamStarted {
        generation: u64,
        result: Result<(Arc<HostStream>, TargetRect), String>,
    },
    /// Errors of the shared stream (end every session).
    Stream(TransportEvent),
    /// Events of one session's peer connection.
    Media {
        session_id: String,
        viewer: u64,
        ev: TransportEvent,
    },
    Stats {
        session_id: String,
        transport: Box<TransportStats>,
    },
}

/// The shared capture/encode, started with the first approved session.
enum StreamState {
    Idle,
    /// Capture + pipeline setup may block (portal dialog, encoder probe);
    /// sessions approved meanwhile wait here.
    Starting {
        generation: u64,
        waiting: Vec<String>,
    },
    Running {
        stream: Arc<HostStream>,
        input: Option<InputWorker>,
    },
}

/// Who receives a game's force feedback for a client's pads.
type RumbleRoutes = Arc<Mutex<HashMap<ClientId, Weak<HostViewer>>>>;

struct ActiveSession {
    session: Session,
    client_id: DeviceId,
    client_name: String,
    /// What the client asked for, clamped to the host's limits.
    settings: StreamSettings,
    role: SessionRole,
    keyboard_mouse: bool,
    /// This session's identity towards the input dispatcher.
    input_key: ClientId,
    viewer: Option<Arc<HostViewer>>,
    bitrate: BitrateController,
    client_stats: Option<ClientStats>,
    prev_client_dropped: u64,
    disconnected_since: Option<Instant>,
    last_heard: Option<Instant>,
    early_candidates: Vec<(u32, String)>,
}

impl ActiveSession {
    fn permissions(&self) -> InputPermissions {
        let play = self.role == SessionRole::Play;
        InputPermissions { gamepads: play, keyboard_mouse: play && self.keyboard_mouse }
    }
}

/// A session that failed recently and may be resumed without a new prompt.
struct FailedSession {
    client: DeviceId,
    at: Instant,
    role: SessionRole,
    keyboard_mouse: bool,
}

struct HostLoop {
    ctx: AppContext,
    link: LinkHandle,
    events: broadcast::Sender<HostEvent>,
    snapshot: Arc<Mutex<HostSnapshot>>,
    internal_tx: mpsc::UnboundedSender<Internal>,
    codes: PairingCodes,
    code_lookup: Option<String>,
    /// A code sent for registration but not yet acknowledged by the server:
    /// (lookup, display form, sent at). It is shown only once registered,
    /// so a client can never try it before the server knows it.
    code_unconfirmed: Option<(String, String, Instant)>,
    pending_pairings: HashMap<String, PendingPairing>,
    replay: ReplayGuard,
    sessions: HashMap<String, ActiveSession>,
    stream: StreamState,
    stream_generation: u64,
    rumble_routes: RumbleRoutes,
    /// Input counters and controller slots, refreshed every second.
    input_stats: Option<WorkerStats>,
    next_input_key: ClientId,
    recently_failed: HashMap<String, FailedSession>,
    input_allowed: bool,
}

const CODE_CONFIRM_WAIT: Duration = Duration::from_secs(3);

/// Start hosting the way the configuration says: with the built-in
/// signaling server (default) or through `network.signaling_url`.
pub fn start_hosting(ctx: AppContext) -> anyhow::Result<HostHandle> {
    if ctx.config.host.builtin_server {
        start_with_builtin_server(ctx)
    } else {
        Ok(start(ctx))
    }
}

/// Start the built-in signaling server, then host through it. The host
/// reaches its own server over loopback, pinning its own certificate.
pub fn start_with_builtin_server(mut ctx: AppContext) -> anyhow::Result<HostHandle> {
    let mut server = crate::builtin_server::start(&ctx)?;
    let url = server.local_url();
    crate::known_servers::trust(&ctx.dir, &url, &server.info.fingerprint)?;
    ctx.config.network.signaling_url = url;
    let upnp = ctx.config.host.upnp;
    let mut handle = start(ctx);
    handle.snapshot.lock().server = Some(server.info.clone());
    if upnp {
        let (snapshot, events) = (handle.snapshot.clone(), handle.events.clone());
        server.port_mapper = Some(crate::upnp::start(server.info.port, move |mapping| {
            if let Some(s) = snapshot.lock().server.as_mut() {
                s.port_mapping = mapping.clone();
            }
            let _ = events.send(HostEvent::PortMapping { mapping });
        }));
    }
    handle._server = Some(Arc::new(server));
    Ok(handle)
}

/// Start hosting: connects to signaling and waits for pairing/session
/// requests. Encoders are probed in the background so the first session
/// starts quickly.
pub fn start(ctx: AppContext) -> HostHandle {
    let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
    let (events, _) = broadcast::channel(256);
    let (alive_tx, alive_rx) = oneshot::channel();
    let snapshot = Arc::new(Mutex::new(HostSnapshot {
        device_id: ctx.identity.device_id().to_string(),
        device_name: ctx.config.device_name.clone(),
        input_allowed: ctx.config.host.input_allowed,
        max_clients: ctx.config.host.max_clients,
        paired_devices: ctx.trust.lock().list(),
        ..Default::default()
    }));
    let handle = HostHandle {
        cmd: cmd_tx,
        events: events.clone(),
        snapshot: snapshot.clone(),
        _alive: Arc::new(alive_tx),
        _server: None,
    };

    tokio::task::spawn_blocking(|| {
        let _ = crate::media::probe_encoders();
    });

    let sig_cfg = ctx.signaling_config(&ctx.config.network.signaling_url, Role::Host);
    let (link, link_rx) = link::spawn(sig_cfg, ctx.identity.clone());
    let (internal_tx, internal_rx) = mpsc::unbounded_channel();
    let input_allowed = ctx.config.host.input_allowed;
    let state = HostLoop {
        ctx,
        link,
        events,
        snapshot,
        internal_tx,
        codes: PairingCodes::default(),
        code_lookup: None,
        code_unconfirmed: None,
        pending_pairings: HashMap::new(),
        replay: ReplayGuard::default(),
        sessions: HashMap::new(),
        stream: StreamState::Idle,
        stream_generation: 0,
        rumble_routes: Arc::new(Mutex::new(HashMap::new())),
        input_stats: None,
        next_input_key: 1,
        recently_failed: HashMap::new(),
        input_allowed,
    };
    tokio::spawn(state.run(cmd_rx, link_rx, internal_rx, alive_rx));
    handle
}

fn audio_spec(choice: &AudioChoice) -> Option<AudioSourceSpec> {
    match choice {
        AudioChoice::Off => None,
        AudioChoice::TestTone => Some(AudioSourceSpec::TestTone { frequency_hz: 440 }),
        AudioChoice::Default | AudioChoice::Device(_) => {
            let device = match choice {
                AudioChoice::Device(d) => Some(d.clone()),
                _ => None,
            };
            Some(AudioSourceSpec::WasapiLoopback { device })
        }
    }
}

/// Stop a viewer off the async runtime (detaching waits for its pads).
fn stop_viewer(viewer: Arc<HostViewer>) {
    tokio::task::spawn_blocking(move || viewer.stop());
}

impl HostLoop {
    async fn run(
        mut self,
        mut cmd_rx: mpsc::UnboundedReceiver<HostCmd>,
        mut link_rx: mpsc::Receiver<LinkEvent>,
        mut internal_rx: mpsc::UnboundedReceiver<Internal>,
        mut alive_rx: oneshot::Receiver<()>,
    ) {
        let mut tick = tokio::time::interval(Duration::from_secs(1));
        loop {
            tokio::select! {
                Some(cmd) = cmd_rx.recv() => self.on_command(cmd).await,
                Some(ev) = link_rx.recv() => self.on_link(ev).await,
                Some(ev) = internal_rx.recv() => self.on_internal(ev).await,
                _ = tick.tick() => self.on_tick().await,
                _ = &mut alive_rx => {
                    tracing::info!("host handle dropped; shutting down");
                    self.end_all(EndReason::HostStopped).await;
                    return;
                }
            }
        }
    }

    /// Publish an event. The snapshot is refreshed first so a UI reacting
    /// to the event immediately sees consistent state.
    fn emit(&self, ev: HostEvent) {
        self.update_snapshot();
        let _ = self.events.send(ev);
    }

    fn update_snapshot(&self) {
        let mut s = self.snapshot.lock();
        s.signaling_connected = self.link.connected();
        s.input_allowed = self.input_allowed;
        s.max_clients = self.ctx.config.host.max_clients;
        s.pairing_code = self.codes.current().map(|(c, _)| c.display());
        s.pairing_expires_unix_ms = self
            .codes
            .current()
            .map(|(_, exp)| now_ms() + exp.saturating_duration_since(Instant::now()).as_millis() as u64);
        s.pending_pairings = self
            .pending_pairings
            .iter()
            .map(|(id, p)| PendingPairingInfo {
                request_id: id.clone(),
                client_name: p.client_name.clone(),
                fingerprint: p.client_id.fingerprint(),
            })
            .collect();
        let mut sessions: Vec<SessionInfo> = self
            .sessions
            .values()
            .map(|a| SessionInfo {
                session_id: a.session.id.clone(),
                client_id: a.client_id.clone(),
                client_name: a.client_name.clone(),
                state: a.session.state().clone(),
                input_allowed: self.input_allowed && a.role == SessionRole::Play,
                role: a.role,
                keyboard_mouse: a.keyboard_mouse,
                controllers: self
                    .input_stats
                    .as_ref()
                    .and_then(|w| w.clients.get(&a.input_key))
                    .map(|(_, pads)| pads.clone())
                    .unwrap_or_default(),
            })
            .collect();
        sessions.sort_by_key(|s| s.client_name.to_lowercase());
        s.sessions = sessions;
        s.paired_devices = self.ctx.trust.lock().list();
    }

    async fn relay(&self, to: &DeviceId, msg: PeerMessage) {
        let env = SignedEnvelope::seal(&self.ctx.identity, Some(to.clone()), msg);
        if let Err(e) = self.link.send(ClientToServer::Relay { to: to.clone(), envelope: env }).await {
            tracing::warn!("cannot reach client through signaling: {e}");
        }
    }

    fn input(&self) -> Option<&InputWorker> {
        match &self.stream {
            StreamState::Running { input, .. } => input.as_ref(),
            _ => None,
        }
    }

    // ---------------------------------------------------------------- commands

    async fn on_command(&mut self, cmd: HostCmd) {
        match cmd {
            HostCmd::GeneratePairingCode => {
                if let Some(old) = self.code_lookup.take() {
                    let _ = self.link.send(ClientToServer::CancelPairing { lookup: old }).await;
                }
                let (code, lookup, _) = self.codes.generate(DEFAULT_CODE_TTL);
                self.code_lookup = Some(lookup.clone());
                let reg = ClientToServer::RegisterPairing {
                    lookup: lookup.clone(),
                    ttl_secs: DEFAULT_CODE_TTL.as_secs() as u32,
                };
                // Shown when the server confirms (PairingRegistered). When
                // signaling is not connected yet, the code is registered on
                // connect (see LinkEvent::Connected).
                if let Err(e) = self.link.send(reg).await {
                    tracing::debug!("pairing code registration deferred: {e}");
                }
                self.code_unconfirmed = Some((lookup, code.display(), Instant::now()));
            }
            HostCmd::CancelPairing => {
                self.code_unconfirmed = None;
                self.codes.cancel();
                if let Some(old) = self.code_lookup.take() {
                    let _ = self.link.send(ClientToServer::CancelPairing { lookup: old }).await;
                }
            }
            HostCmd::DecidePairing { request_id, approve } => self.decide_pairing(&request_id, approve).await,
            HostCmd::DecideSession { session_id, approve, role, reason } => {
                let waiting = self
                    .sessions
                    .get(&session_id)
                    .map(|a| *a.session.state() == SessionState::AwaitingApproval)
                    .unwrap_or(false);
                if !waiting {
                    self.emit(HostEvent::Warning { message: "that session is no longer waiting for approval".into() });
                } else if approve {
                    let kbm = role == SessionRole::Play && !self.someone_has_keyboard_mouse();
                    self.accept_session(&session_id, role, kbm).await;
                } else {
                    let client = self.sessions[&session_id].client_id.clone();
                    let reason = if reason.is_empty() { "declined by the host".to_string() } else { reason };
                    self.relay(&client, PeerMessage::SessionRejected { session_id: session_id.clone(), reason }).await;
                    self.finish_session(&session_id, EndReason::RejectedByHost);
                }
            }
            HostCmd::EndSession { session_id } => {
                if self.sessions.contains_key(&session_id) {
                    self.end_session(&session_id, EndReason::KickedByHost).await;
                }
            }
            HostCmd::SetRole { session_id, role } => {
                let kbm_default = role == SessionRole::Play && !self.someone_has_keyboard_mouse();
                if let Some(a) = self.sessions.get_mut(&session_id) {
                    if a.role != role {
                        a.role = role;
                        // Promoted players get keyboard/mouse only if nobody has it.
                        a.keyboard_mouse = role == SessionRole::Play && (a.keyboard_mouse || kbm_default);
                        self.apply_permissions(&session_id);
                    }
                }
            }
            HostCmd::SetKeyboardMouse { session_id, allowed } => {
                if let Some(a) = self.sessions.get_mut(&session_id) {
                    if a.keyboard_mouse != allowed {
                        a.keyboard_mouse = allowed;
                        self.apply_permissions(&session_id);
                    }
                }
            }
            HostCmd::SetInputAllowed(allowed) => {
                self.input_allowed = allowed;
                if let Some(i) = self.input() {
                    i.set_allowed(allowed);
                }
                let ids: Vec<String> = self.sessions.keys().cloned().collect();
                for id in ids {
                    self.send_input_permission(&id);
                }
                tracing::info!(allowed, "remote input permission changed");
                self.emit(HostEvent::InputPermission { allowed });
            }
            HostCmd::DisconnectAll => self.end_all(EndReason::KickedByHost).await,
            HostCmd::Revoke(id) => {
                let removed = self.ctx.trust.lock().remove(&id);
                if let Err(e) = removed {
                    self.emit(HostEvent::Error { message: format!("could not update trust store: {e}") });
                }
                let theirs: Vec<String> =
                    self.sessions.iter().filter(|(_, a)| a.client_id == id).map(|(s, _)| s.clone()).collect();
                for s in theirs {
                    self.end_session(&s, EndReason::KickedByHost).await;
                }
            }
        }
        self.update_snapshot();
    }

    fn someone_has_keyboard_mouse(&self) -> bool {
        self.sessions.values().any(|a| a.role == SessionRole::Play && a.keyboard_mouse && a.session.is_authorized())
    }

    /// Push a session's role/keyboard-mouse change to the input dispatcher
    /// and the client, and tell the UIs.
    fn apply_permissions(&mut self, session_id: &str) {
        let Some(a) = self.sessions.get(session_id) else { return };
        if let Some(i) = self.input() {
            i.set_permissions(a.input_key, a.permissions());
        }
        tracing::info!(client = %a.client_name, role = ?a.role, keyboard_mouse = a.keyboard_mouse, "permissions changed");
        let ev = HostEvent::Permissions {
            session_id: session_id.to_string(),
            client_name: a.client_name.clone(),
            role: a.role,
            keyboard_mouse: a.keyboard_mouse,
        };
        self.send_input_permission(session_id);
        self.emit(ev);
    }

    /// Tell a client whether its input counts. Watchers are told "no", so
    /// their app shows that input is off (existing message; older clients
    /// understand it).
    fn send_input_permission(&self, session_id: &str) {
        let Some(a) = self.sessions.get(session_id) else { return };
        let allowed = self.input_allowed && a.role == SessionRole::Play;
        if let Some(v) = &a.viewer {
            let _ = v.send(DataChannelKind::Control, &encode(&ControlMessage::InputPermission { allowed }));
        }
    }

    async fn decide_pairing(&mut self, request_id: &str, approve: bool) {
        let Some(p) = self.pending_pairings.remove(request_id) else { return };
        if !approve {
            self.relay(&p.client_id, PeerMessage::PairRejected { reason: "declined by the host".into() }).await;
            self.emit(HostEvent::PairingRejected { client_name: p.client_name, reason: "declined".into() });
            return;
        }
        let host_pk = self.ctx.identity.public_key_b64();
        let proof = p.code.accept_proof(&host_pk.0, &p.client_pk.0, &p.nonce);
        let device = PairedDevice {
            device_id: p.client_id.clone(),
            name: p.client_name.clone(),
            public_key: p.client_pk.clone(),
            paired_at_ms: now_ms(),
            last_connected_ms: None,
            signaling_url: None,
        };
        if let Err(e) = self.ctx.trust.lock().add(device) {
            self.emit(HostEvent::Error { message: format!("could not save paired device: {e}") });
            return;
        }
        self.relay(
            &p.client_id,
            PeerMessage::PairAccepted {
                host_name: self.ctx.config.device_name.clone(),
                host_public_key: host_pk,
                proof,
            },
        )
        .await;
        tracing::info!(client = %p.client_id, name = %p.client_name, "device paired");
        self.emit(HostEvent::Paired { client_name: p.client_name, device_id: p.client_id.to_string() });
    }

    // --------------------------------------------------------------- signaling

    async fn on_link(&mut self, ev: LinkEvent) {
        match ev {
            LinkEvent::Connected { ice_servers } => {
                let detail = format!(
                    "connected to {} ({} ICE servers)",
                    self.ctx.config.network.signaling_url,
                    ice_servers.len()
                );
                self.snapshot.lock().signaling_detail = detail.clone();
                self.emit(HostEvent::Signaling { connected: true, detail });
                // Re-register an active pairing code after a reconnect.
                if let (Some(lookup), Some((_, exp))) = (&self.code_lookup, self.codes.current()) {
                    let ttl = exp.saturating_duration_since(Instant::now()).as_secs().max(30) as u32;
                    let _ =
                        self.link.send(ClientToServer::RegisterPairing { lookup: lookup.clone(), ttl_secs: ttl }).await;
                }
            }
            LinkEvent::Disconnected { reason, retry_in } => {
                let detail = match retry_in {
                    Some(d) => format!("{reason}; retrying in {:.1}s", d.as_secs_f32()),
                    None => reason,
                };
                self.snapshot.lock().signaling_detail = detail.clone();
                self.emit(HostEvent::Signaling { connected: false, detail });
            }
            LinkEvent::Signal(SignalingEvent::PairRequest { lookup, from, from_public_key, envelope }) => {
                self.on_pair_request(lookup, from, from_public_key, envelope).await
            }
            LinkEvent::Signal(SignalingEvent::Relay { from, from_public_key, envelope }) => {
                self.on_relay(from, from_public_key, envelope).await
            }
            LinkEvent::Signal(SignalingEvent::ServerError { code, message }) => {
                tracing::debug!(?code, %message, "signaling error");
            }
            LinkEvent::Signal(SignalingEvent::PairingRegistered { lookup, .. }) => {
                if self.code_unconfirmed.as_ref().is_some_and(|(l, _, _)| *l == lookup) {
                    self.show_pairing_code();
                }
            }
            LinkEvent::Signal(_) => {}
        }
        self.update_snapshot();
    }

    async fn on_pair_request(&mut self, lookup: String, from: DeviceId, from_pk: PublicKeyB64, env: SignedEnvelope) {
        let me = self.ctx.identity.device_id();
        let body = match env.open(&from_pk, Some(&me)).and_then(|b| self.replay.check(&b).map(|_| b)) {
            Ok(b) => b,
            Err(e) => {
                tracing::warn!(%from, "rejected pairing envelope: {e}");
                return;
            }
        };
        let PeerMessage::PairRequest { client_name, client_public_key, nonce, proof } = body.message else { return };
        if client_public_key != from_pk {
            tracing::warn!(%from, "pairing request key does not match the authenticated key");
            return;
        }
        match self.codes.check(&lookup, &client_public_key.0, &nonce, &proof) {
            PairingCheck::Valid(code) => {
                if self.code_lookup.as_deref() == Some(lookup.as_str()) {
                    self.code_lookup = None;
                    let _ = self.link.send(ClientToServer::CancelPairing { lookup }).await;
                }
                let request_id = random_hex(8);
                tracing::info!(%from, name = %client_name, "pairing request awaiting host approval");
                let fingerprint = from.fingerprint();
                self.pending_pairings.insert(
                    request_id.clone(),
                    PendingPairing {
                        client_id: from,
                        client_pk: client_public_key,
                        client_name: client_name.clone(),
                        nonce,
                        code,
                        created: Instant::now(),
                    },
                );
                self.emit(HostEvent::PairingRequest { request_id, client_name, fingerprint });
            }
            PairingCheck::BadProof => {
                self.relay(&from, PeerMessage::PairRejected { reason: "wrong pairing code".into() }).await;
            }
            PairingCheck::UnknownOrExpired => {
                self.relay(&from, PeerMessage::PairRejected { reason: "pairing code expired".into() }).await;
            }
        }
    }

    async fn on_relay(&mut self, from: DeviceId, from_pk: PublicKeyB64, env: SignedEnvelope) {
        let me = self.ctx.identity.device_id();
        let body = match env.open(&from_pk, Some(&me)).and_then(|b| self.replay.check(&b).map(|_| b)) {
            Ok(b) => b,
            Err(e) => {
                tracing::warn!(%from, "rejected envelope: {e}");
                return;
            }
        };
        let trusted = self.ctx.trust.lock().verify(&from, &from_pk).map(|d| d.cloned());
        let device = match trusted {
            Ok(Some(d)) => d,
            Ok(None) => {
                if let PeerMessage::SessionRequest { session_id, .. } = body.message {
                    tracing::info!(%from, "session request from unpaired device rejected");
                    self.relay(
                        &from,
                        PeerMessage::SessionRejected {
                            session_id,
                            reason: "this device is not paired with the host".into(),
                        },
                    )
                    .await;
                }
                return;
            }
            Err(e) => {
                tracing::warn!("{e}");
                return;
            }
        };
        match body.message {
            PeerMessage::SessionRequest { session_id, settings, capabilities, resume } => {
                self.on_session_request(device, session_id, settings, capabilities.platform, resume).await
            }
            PeerMessage::Sdp { session_id, kind: SdpKind::Answer, sdp } => {
                if let Some(a) = self.session_for(&session_id, &from) {
                    if let Some(v) = &a.viewer {
                        if let Err(e) = v.set_remote_sdp(SdpKind::Answer, &sdp) {
                            tracing::warn!("bad answer: {e}");
                        }
                    }
                }
            }
            PeerMessage::IceCandidate { session_id, mline_index, candidate } => {
                if let Some(a) = self.session_for(&session_id, &from) {
                    match &a.viewer {
                        Some(v) => {
                            let _ = v.add_remote_candidate(mline_index, &candidate);
                        }
                        None => a.early_candidates.push((mline_index, candidate)),
                    }
                }
            }
            PeerMessage::EndSession { session_id, reason } => {
                if self.session_for(&session_id, &from).is_some() {
                    tracing::info!(%reason, "client ended the session");
                    self.end_session(&session_id, EndReason::ClientDisconnected).await;
                }
            }
            other => tracing::debug!("ignoring peer message {other:?}"),
        }
    }

    fn session_for(&mut self, session_id: &str, from: &DeviceId) -> Option<&mut ActiveSession> {
        self.sessions.get_mut(session_id).filter(|a| &a.client_id == from)
    }

    async fn on_session_request(
        &mut self,
        device: PairedDevice,
        session_id: String,
        requested: StreamSettings,
        platform: String,
        resume: bool,
    ) {
        if let Some(a) = self.sessions.get(&session_id) {
            if a.client_id == device.device_id {
                if resume && a.session.is_authorized() {
                    // The client abandoned its broken peer connection; build
                    // a new one for the same, already approved session.
                    self.restart_media(&session_id).await;
                } else {
                    tracing::debug!("duplicate session request ignored");
                }
            }
            return;
        }
        // A device asking for a new session has given up on its old one.
        let stale: Vec<String> =
            self.sessions.iter().filter(|(_, a)| a.client_id == device.device_id).map(|(s, _)| s.clone()).collect();
        for s in stale {
            tracing::info!(client = %device.name, "replacing the device's previous session");
            self.end_session(&s, EndReason::ClientDisconnected).await;
        }
        let max = self.ctx.config.host.max_clients as usize;
        if self.sessions.len() >= max {
            let reason = format!("the host is full ({max} people are connected)");
            self.relay(&device.device_id, PeerMessage::SessionRejected { session_id, reason }).await;
            return;
        }
        let h = &self.ctx.config.host;
        let mut settings = requested.clamp_to(h.max_width, h.max_height, h.max_fps);
        settings.max_bitrate_kbps = settings.max_bitrate_kbps.min(h.max_bitrate_kbps);
        settings.min_bitrate_kbps = settings.min_bitrate_kbps.min(settings.max_bitrate_kbps);
        settings.encoder = h.encoder.clone();
        if let Err(e) = settings.validate() {
            self.relay(
                &device.device_id,
                PeerMessage::SessionRejected { session_id, reason: format!("unsupported settings: {e}") },
            )
            .await;
            return;
        }
        let mut session = Session::new(session_id.clone());
        let _ = session.transition(SessionState::AwaitingApproval);
        let bitrate =
            BitrateController::new(settings.min_bitrate_kbps, settings.max_bitrate_kbps, settings.start_bitrate_kbps());
        let input_key = self.next_input_key;
        self.next_input_key += 1;
        self.sessions.insert(
            session_id.clone(),
            ActiveSession {
                session,
                client_id: device.device_id.clone(),
                client_name: device.name.clone(),
                settings: settings.clone(),
                role: SessionRole::Play,
                keyboard_mouse: false,
                input_key,
                viewer: None,
                bitrate,
                client_stats: None,
                prev_client_dropped: 0,
                disconnected_since: None,
                last_heard: None,
                early_candidates: Vec::new(),
            },
        );
        let resumable = self
            .recently_failed
            .remove(&session_id)
            .filter(|f| resume && f.client == device.device_id && f.at.elapsed() < RESUME_WINDOW);
        tracing::info!(%session_id, client = %device.name, %platform, resume = resumable.is_some(), "session requested");
        if let Some(f) = resumable {
            tracing::info!("resuming previously approved session after a connection failure");
            self.accept_session(&session_id, f.role, f.keyboard_mouse).await;
        } else if self.ctx.config.host.trust_paired_sessions {
            tracing::info!("session auto-approved by host policy (trust_paired_sessions)");
            let kbm = !self.someone_has_keyboard_mouse();
            self.accept_session(&session_id, SessionRole::Play, kbm).await;
        } else {
            self.emit(HostEvent::SessionRequest {
                session_id: session_id.clone(),
                client_name: device.name.clone(),
                fingerprint: device.device_id.fingerprint(),
                settings,
            });
            self.emit_session_state(&session_id);
        }
    }

    fn emit_session_state(&self, session_id: &str) {
        if let Some(a) = self.sessions.get(session_id) {
            self.emit(HostEvent::Session {
                session_id: a.session.id.clone(),
                client_name: a.client_name.clone(),
                state: a.session.state().clone(),
            });
        }
    }

    async fn accept_session(&mut self, session_id: &str, role: SessionRole, keyboard_mouse: bool) {
        let Some(a) = self.sessions.get_mut(session_id) else { return };
        if a.session.transition(SessionState::Negotiating).is_err() {
            return;
        }
        a.role = role;
        a.keyboard_mouse = role == SessionRole::Play && keyboard_mouse;
        tracing::info!(client = %a.client_name, ?role, keyboard_mouse = a.keyboard_mouse, "session approved");
        self.start_media(session_id).await;
    }

    /// Replace the peer connection of a session (resume).
    async fn restart_media(&mut self, session_id: &str) {
        let Some(a) = self.sessions.get_mut(session_id) else { return };
        tracing::info!(session = %a.session.id, "client is re-establishing the connection; restarting media");
        if let Some(v) = a.viewer.take() {
            self.rumble_routes.lock().remove(&a.input_key);
            stop_viewer(v);
        }
        a.last_heard = None;
        a.disconnected_since = None;
        a.early_candidates.clear();
        if *a.session.state() == SessionState::Connected {
            let _ = a.session.transition(SessionState::Reconnecting);
        }
        if *a.session.state() == SessionState::Reconnecting {
            let _ = a.session.transition(SessionState::Negotiating);
        }
        let key = a.input_key;
        if let Some(i) = self.input() {
            i.release_client(key);
        }
        self.start_media(session_id).await;
    }

    /// Accept the session towards the client and give it a viewer of the
    /// shared stream, starting the stream if this is the first viewer.
    async fn start_media(&mut self, session_id: &str) {
        let Some(a) = self.sessions.get(session_id) else { return };
        let client = a.client_id.clone();
        // Joiners get the stream that is already running.
        let settings = match &self.stream {
            StreamState::Running { stream, .. } => stream.settings().clone(),
            _ => a.settings.clone(),
        };
        self.relay(
            &client,
            PeerMessage::SessionAccepted {
                session_id: session_id.to_string(),
                expires_at_ms: now_ms() + DEFAULT_MAX_SESSION.as_millis() as u64,
                settings: settings.clone(),
                input_allowed: self.input_allowed && a.role == SessionRole::Play,
            },
        )
        .await;
        self.emit_session_state(session_id);

        if let StreamState::Starting { waiting, .. } = &mut self.stream {
            waiting.push(session_id.to_string());
        } else if matches!(self.stream, StreamState::Running { .. }) {
            self.attach_viewer(session_id);
        } else {
            self.start_stream(session_id.to_string(), settings);
        }
    }

    fn start_stream(&mut self, first: String, settings: StreamSettings) {
        self.stream_generation += 1;
        let generation = self.stream_generation;
        self.stream = StreamState::Starting { generation, waiting: vec![first] };
        let host_cfg = self.ctx.config.host.clone();
        let tx = self.internal_tx.clone();
        tokio::task::spawn_blocking(move || {
            let result = (|| -> anyhow::Result<_> {
                let backend = match host_cfg.capture {
                    Some(kind) => crate::capture::backend_for(kind)?,
                    None => crate::capture::detect_backends()
                        .into_iter()
                        .next()
                        .ok_or_else(|| anyhow::anyhow!("no screen capture backend available in this session"))?,
                };
                let displays = backend.displays()?;
                let display_id = host_cfg.display_id.clone().unwrap_or_default();
                let display = displays
                    .iter()
                    .find(|d| d.id == display_id)
                    .or_else(|| displays.iter().find(|d| d.primary))
                    .or(displays.first())
                    .cloned()
                    .ok_or_else(|| anyhow::anyhow!("no displays found"))?;
                let capture = backend.open(&display.id)?;
                let target = TargetRect { x: display.x, y: display.y, width: display.width, height: display.height };
                let (etx, mut erx) = mpsc::unbounded_channel();
                let stream =
                    HostStream::start(HostStreamConfig { capture, audio: audio_spec(&host_cfg.audio), settings }, etx)?;
                let fwd = tx.clone();
                tokio::spawn(async move {
                    while let Some(ev) = erx.recv().await {
                        if fwd.send(Internal::Stream(ev)).is_err() {
                            break;
                        }
                    }
                });
                Ok((stream, target))
            })();
            let _ = tx.send(Internal::StreamStarted { generation, result: result.map_err(|e| e.to_string()) });
        });
    }

    /// Give an approved session its own peer connection on the running
    /// stream and register it with the input dispatcher.
    fn attach_viewer(&mut self, session_id: &str) {
        let StreamState::Running { stream, input } = &self.stream else { return };
        let Some(a) = self.sessions.get_mut(session_id) else { return };
        let mut ice = IceConfig { servers: self.link.ice_servers(), relay_only: self.ctx.config.network.relay_only };
        ice.servers.extend(self.ctx.config.network.ice_servers.iter().cloned());
        let (etx, mut erx) = mpsc::unbounded_channel();
        let viewer = match stream.add_viewer(&ice, etx) {
            Ok(v) => v,
            Err(e) => {
                let msg = format!("cannot start streaming to {}: {e}", a.client_name);
                tracing::error!("{msg}");
                let id = session_id.to_string();
                let tx = self.internal_tx.clone();
                // Ends the session through the normal media error path.
                let _ = tx.send(Internal::Media { session_id: id, viewer: 0, ev: TransportEvent::Error(msg) });
                return;
            }
        };
        let viewer_id = viewer.id();
        let tx = self.internal_tx.clone();
        let id = session_id.to_string();
        tokio::spawn(async move {
            while let Some(ev) = erx.recv().await {
                if tx.send(Internal::Media { session_id: id.clone(), viewer: viewer_id, ev }).is_err() {
                    break;
                }
            }
        });
        for (m, c) in a.early_candidates.drain(..) {
            let _ = viewer.add_remote_candidate(m, &c);
        }
        if let Some(i) = input {
            i.add_client(a.input_key, a.permissions());
        }
        self.rumble_routes.lock().insert(a.input_key, Arc::downgrade(&viewer));
        a.viewer = Some(viewer);
    }

    async fn on_internal(&mut self, ev: Internal) {
        match ev {
            Internal::StreamStarted { generation, result } => self.on_stream_started(generation, result).await,
            Internal::Stream(TransportEvent::Error(e)) => {
                tracing::error!("stream error: {e}");
                self.emit(HostEvent::Error { message: e.clone() });
                self.end_all(EndReason::Error(e)).await;
            }
            Internal::Stream(TransportEvent::Warning(w)) => {
                tracing::warn!("{w}");
                self.emit(HostEvent::Warning { message: w });
            }
            Internal::Stream(_) => {}
            Internal::Media { session_id, viewer, ev } => self.on_media(&session_id, viewer, ev).await,
            Internal::Stats { session_id, transport } => self.on_stats(&session_id, *transport).await,
        }
        self.update_snapshot();
    }

    async fn on_stream_started(&mut self, generation: u64, result: Result<(Arc<HostStream>, TargetRect), String>) {
        let waiting = match &mut self.stream {
            StreamState::Starting { generation: g, waiting } if *g == generation => std::mem::take(waiting),
            _ => {
                // Everyone left before the stream came up.
                if let Ok((stream, _)) = result {
                    tokio::task::spawn_blocking(move || stream.stop());
                }
                return;
            }
        };
        let waiting: Vec<String> = waiting.into_iter().filter(|s| self.sessions.contains_key(s)).collect();
        match result {
            Ok((stream, _)) if waiting.is_empty() => {
                self.stream = StreamState::Idle;
                tokio::task::spawn_blocking(move || stream.stop());
            }
            Ok((stream, target)) => {
                // Force feedback from games goes back to the owning client's controller.
                let routes = self.rumble_routes.clone();
                let on_rumble = move |client: ClientId, r: crate::controller::Rumble| {
                    let viewer = routes.lock().get(&client).and_then(Weak::upgrade);
                    if let Some(v) = viewer {
                        let msg = ControlMessage::Rumble {
                            index: r.index,
                            strong: r.strong,
                            weak: r.weak,
                            duration_ms: r.duration_ms,
                        };
                        let _ = v.send(DataChannelKind::Control, &encode(&msg));
                    }
                };
                let input = match InputWorker::start(&self.ctx.config.host, target, self.input_allowed, on_rumble) {
                    Ok(w) => Some(w),
                    Err(e) => {
                        let msg = format!("remote input unavailable: {e}");
                        tracing::warn!("{msg}");
                        self.emit(HostEvent::Warning { message: msg });
                        None
                    }
                };
                self.stream = StreamState::Running { stream, input };
                for s in waiting {
                    self.attach_viewer(&s);
                }
            }
            Err(e) => {
                self.stream = StreamState::Idle;
                tracing::error!("cannot start streaming: {e}");
                self.emit(HostEvent::Error { message: format!("cannot start streaming: {e}") });
                for s in waiting {
                    self.end_session(&s, EndReason::Error(e.clone())).await;
                }
            }
        }
    }

    // ------------------------------------------------------------------- media

    async fn on_media(&mut self, session_id: &str, viewer_id: u64, ev: TransportEvent) {
        let Some(a) = self.sessions.get_mut(session_id) else { return };
        // Events from a replaced peer connection (after a resume) are stale.
        let current = a.viewer.as_ref().map(|v| v.id());
        if viewer_id != 0 && current != Some(viewer_id) {
            return;
        }
        let client = a.client_id.clone();
        if let TransportEvent::ChannelMessage(..) = ev {
            a.last_heard = Some(Instant::now());
            if a.disconnected_since.take().is_some() && a.session.transition(SessionState::Connected).is_ok() {
                tracing::info!(client = %a.client_name, "peer is reachable again");
                self.emit_session_state(session_id);
            }
        }
        let Some(a) = self.sessions.get_mut(session_id) else { return };
        let sid = session_id.to_string();
        match ev {
            TransportEvent::LocalSdp { kind, sdp } => {
                self.relay(&client, PeerMessage::Sdp { session_id: sid, kind, sdp }).await;
            }
            TransportEvent::LocalCandidate { mline_index, candidate } => {
                self.relay(&client, PeerMessage::IceCandidate { session_id: sid, mline_index, candidate }).await;
            }
            TransportEvent::PeerState(ConnectionState::Connected) => {
                a.disconnected_since = None;
                if a.session.transition(SessionState::Connected).is_ok() {
                    let _ = self.ctx.trust.lock().touch(&client, now_ms());
                    tracing::info!(session_id, client = %a.client_name, "peer connected; streaming");
                    // The keyframe sent at attach time went out before the
                    // connection was up; give the newcomer a fresh one.
                    if let Some(v) = &a.viewer {
                        v.force_keyframe();
                    }
                    self.emit_session_state(session_id);
                }
            }
            TransportEvent::PeerState(ConnectionState::Disconnected) => {
                if self.interrupted(session_id, "peer connection interrupted") {
                    self.emit_session_state(session_id);
                }
            }
            TransportEvent::PeerState(ConnectionState::Failed) => {
                self.remember_failed(session_id);
                self.end_session(session_id, EndReason::ConnectionFailed("ICE/DTLS connection failed".into())).await;
            }
            TransportEvent::ChannelOpen(DataChannelKind::Control) => self.send_input_permission(session_id),
            TransportEvent::ChannelMessage(DataChannelKind::Input | DataChannelKind::Gamepad, text) => {
                let key = a.input_key;
                if let Some(i) = self.input() {
                    i.message(key, text);
                }
            }
            TransportEvent::ChannelMessage(DataChannelKind::Control, text) => {
                match decode::<ControlMessage>(text.as_bytes(), MAX_DATA_CHANNEL_MESSAGE_BYTES) {
                    Ok(ControlMessage::Ping { id, client_time_us }) => {
                        if let Some(v) = &a.viewer {
                            let pong = ControlMessage::Pong {
                                id,
                                client_time_us,
                                host_time_us: crate::media::timing::now_us(),
                            };
                            let _ = v.send(DataChannelKind::Control, &encode(&pong));
                        }
                    }
                    Ok(ControlMessage::ClientStats(s)) => a.client_stats = Some(s),
                    Ok(ControlMessage::RequestKeyframe {}) => {
                        if let Some(v) = &a.viewer {
                            v.force_keyframe();
                        }
                    }
                    Ok(ControlMessage::Goodbye { .. }) => {
                        self.end_session(session_id, EndReason::ClientDisconnected).await
                    }
                    Ok(other) => tracing::debug!("unexpected control message {other:?}"),
                    Err(e) => tracing::debug!("invalid control message: {e}"),
                }
            }
            TransportEvent::Warning(w) => {
                tracing::warn!("{w}");
                self.emit(HostEvent::Warning { message: w });
            }
            TransportEvent::Error(e) => {
                tracing::error!(client = %a.client_name, "media error: {e}");
                self.emit(HostEvent::Error { message: e.clone() });
                self.end_session(session_id, EndReason::Error(e)).await;
            }
            _ => {}
        }
        self.update_snapshot();
    }

    /// Connection interrupted: stop acting on stale input right away.
    fn interrupted(&mut self, session_id: &str, why: &str) -> bool {
        let Some(a) = self.sessions.get_mut(session_id) else { return false };
        if a.disconnected_since.is_some() {
            return false;
        }
        a.disconnected_since = Some(Instant::now());
        let key = a.input_key;
        let changed = a.session.transition(SessionState::Reconnecting).is_ok();
        tracing::warn!(client = %a.client_name, "{why}; held input released, waiting for recovery");
        if let Some(i) = self.input() {
            i.release_client(key);
        }
        changed
    }

    fn remember_failed(&mut self, session_id: &str) {
        if let Some(a) = self.sessions.get(session_id) {
            self.recently_failed.insert(
                session_id.to_string(),
                FailedSession {
                    client: a.client_id.clone(),
                    at: Instant::now(),
                    role: a.role,
                    keyboard_mouse: a.keyboard_mouse,
                },
            );
        }
    }

    async fn on_stats(&mut self, session_id: &str, transport: TransportStats) {
        let encoder_hardware;
        let ev = {
            let Some(a) = self.sessions.get_mut(session_id) else { return };
            let Some(viewer) = a.viewer.clone() else { return };
            if a.settings.adaptive_bitrate {
                let dropped = a.client_stats.as_ref().map(|c| c.frames_dropped).unwrap_or(0);
                let sample = NetworkSample {
                    fraction_lost: transport.remote_fraction_lost,
                    rtt_ms: transport.remote_rtt_ms,
                    client_dropped_frames: dropped.saturating_sub(a.prev_client_dropped),
                };
                a.prev_client_dropped = dropped;
                match a.bitrate.update(sample) {
                    Decision::Decrease(k) | Decision::Increase(k) => viewer.set_bitrate(k),
                    Decision::Hold => {}
                }
            }
            let host = viewer.host_stats();
            let _ = viewer.send(DataChannelKind::Control, &encode(&ControlMessage::HostStats(host.clone())));
            encoder_hardware = viewer.encoder_hardware();
            let worker = self.input_stats.as_ref();
            let (input, controllers) = worker.and_then(|w| w.clients.get(&a.input_key)).cloned().unwrap_or_default();
            HostStatsEvent {
                session_id: session_id.to_string(),
                client_name: a.client_name.clone(),
                host,
                client: a.client_stats.clone(),
                transport,
                encoder_hardware,
                input_backend: worker.map(|w| w.keyboard_mouse_backend).unwrap_or("unavailable").to_string(),
                controller_backend: worker.map(|w| w.controller_backend).unwrap_or("unavailable").to_string(),
                input,
                controllers,
            }
        };
        self.emit(HostEvent::Stats(Box::new(ev)));
    }

    /// Show the pending pairing code with its remaining lifetime.
    fn show_pairing_code(&mut self) {
        let Some((_, code, _)) = self.code_unconfirmed.take() else { return };
        let Some((_, expires)) = self.codes.current() else { return };
        let left = expires.saturating_duration_since(Instant::now()).as_secs();
        self.emit(HostEvent::PairingCode { code, expires_in_secs: left });
    }

    async fn on_tick(&mut self) {
        let now = Instant::now();
        // The server normally confirms within milliseconds; never hide the
        // code for long if a confirmation got lost.
        if self.code_unconfirmed.as_ref().is_some_and(|(_, _, sent)| now.duration_since(*sent) > CODE_CONFIRM_WAIT) {
            self.emit(HostEvent::Warning {
                message: "the signaling server has not confirmed the pairing code yet; it may not work until the \
                          host is connected"
                    .into(),
            });
            self.show_pairing_code();
        }
        self.pending_pairings.retain(|_, p| now.duration_since(p.created) < PAIRING_APPROVAL_TIMEOUT);
        self.recently_failed.retain(|_, f| now.duration_since(f.at) < RESUME_WINDOW);
        if self.code_lookup.is_some() && self.codes.current().map(|(_, e)| e <= now).unwrap_or(true) {
            self.code_lookup = None;
            self.codes.cancel();
            self.emit(HostEvent::PairingCodeExpired);
        }
        // One input-worker round trip per second for everyone's counters.
        self.input_stats = self.input().and_then(|i| i.stats());

        let mut ends = Vec::new();
        let mut interrupted = Vec::new();
        for (id, a) in self.sessions.iter_mut() {
            let silent = a.last_heard.map(|t| now.duration_since(t) > PEER_SILENCE).unwrap_or(false);
            if silent && *a.session.state() == SessionState::Connected {
                interrupted.push(id.clone());
            }
            if let Some(r) = a.session.check_timeouts(now) {
                ends.push((id.clone(), r, false));
            } else if a.disconnected_since.map(|t| now.duration_since(t) > RECONNECT_GRACE).unwrap_or(false) {
                ends.push((id.clone(), EndReason::ConnectionFailed("connection lost".into()), true));
            } else if let Some(v) = &a.viewer {
                let rx = v.request_stats();
                let tx = self.internal_tx.clone();
                let session_id = id.clone();
                tokio::spawn(async move {
                    if let Ok(Ok(transport)) = tokio::time::timeout(Duration::from_millis(800), rx).await {
                        let _ = tx.send(Internal::Stats { session_id, transport: Box::new(transport) });
                    }
                });
            }
        }
        for id in interrupted {
            if self.interrupted(&id, "no data from the client") {
                self.emit_session_state(&id);
            }
        }
        for (id, reason, failed) in ends {
            if failed {
                self.remember_failed(&id);
            }
            self.end_session(&id, reason).await;
        }
        self.update_snapshot();
    }

    async fn end_all(&mut self, reason: EndReason) {
        let ids: Vec<String> = self.sessions.keys().cloned().collect();
        for id in ids {
            self.end_session(&id, reason.clone()).await;
        }
    }

    /// Notify the client, tear down its peer connection and input, record
    /// the reason.
    async fn end_session(&mut self, session_id: &str, reason: EndReason) {
        let Some(a) = self.sessions.get(session_id) else { return };
        // Release held input before anything that may wait on the network.
        let key = a.input_key;
        let client = a.client_id.clone();
        if let Some(i) = self.input() {
            i.remove_client(key);
        }
        if let Some(v) = &a.viewer {
            let _ =
                v.send(DataChannelKind::Control, &encode(&ControlMessage::Goodbye { reason: format!("{reason:?}") }));
        }
        if !matches!(reason, EndReason::ClientDisconnected) {
            self.relay(
                &client,
                PeerMessage::EndSession { session_id: session_id.to_string(), reason: format!("{reason:?}") },
            )
            .await;
        }
        self.finish_session(session_id, reason);
    }

    fn finish_session(&mut self, session_id: &str, reason: EndReason) {
        let Some(mut a) = self.sessions.remove(session_id) else { return };
        // Input first: releases held keys/buttons before anything else.
        if let Some(i) = self.input() {
            i.remove_client(a.input_key);
        }
        self.rumble_routes.lock().remove(&a.input_key);
        if let Some(v) = a.viewer.take() {
            stop_viewer(v);
        }
        let _ = a.session.transition(SessionState::Ended(reason.clone()));
        tracing::info!(session = %a.session.id, client = %a.client_name, ?reason, "session ended");
        self.emit(HostEvent::Session {
            session_id: a.session.id.clone(),
            client_name: a.client_name,
            state: a.session.state().clone(),
        });
        self.stop_stream_if_unused();
        self.update_snapshot();
    }

    /// Nobody left to stream to: stop capture, encoding and input.
    fn stop_stream_if_unused(&mut self) {
        if self.sessions.values().any(|a| a.session.is_authorized()) {
            return;
        }
        match std::mem::replace(&mut self.stream, StreamState::Idle) {
            StreamState::Running { stream, input } => {
                // Dropping the worker releases input and removes virtual pads.
                drop(input);
                self.input_stats = None;
                tracing::info!("last viewer left; stopping the stream");
                tokio::task::spawn_blocking(move || stream.stop());
            }
            // A start in flight is stopped when it reports in (generation check).
            StreamState::Starting { generation, waiting } if waiting.iter().any(|s| self.sessions.contains_key(s)) => {
                self.stream = StreamState::Starting { generation, waiting };
            }
            StreamState::Starting { .. } | StreamState::Idle => {}
        }
    }
}

/// Capture kinds a host can offer in this session (for UIs).
pub fn available_capture_backends() -> Vec<(CaptureKind, Vec<crate::capture::DisplayInfo>, Option<String>)> {
    let mut out = Vec::new();
    for b in crate::capture::detect_backends() {
        match b.displays() {
            Ok(d) => out.push((b.kind(), d, None)),
            Err(e) => out.push((b.kind(), Vec::new(), Some(e.to_string()))),
        }
    }
    out
}
