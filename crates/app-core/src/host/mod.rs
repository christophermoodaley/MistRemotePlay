//! Host service: pairing, session consent, media and input for one
//! streaming session at a time.
//!
//! All state lives in a single task (`HostLoop`); UIs talk to it through a
//! [`HostHandle`] (commands + broadcast events + a read-only snapshot).

mod input_worker;

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use mist_audio_api::AudioSourceSpec;
use mist_capture_api::{CaptureKind, CaptureSourceSpec};
use mist_input_api::TargetRect;
use mist_media_engine::{
    ConnectionState, DataChannelKind, HostMedia, HostMediaConfig, IceConfig, MediaTransport, TransportEvent,
    TransportStats,
};
use mist_protocol::control::{ClientStats, ControlMessage};
use mist_protocol::identity::{random_hex, DeviceId, PublicKeyB64};
use mist_protocol::pairing::PairingCode;
use mist_protocol::peer::{now_ms, PeerMessage, ReplayGuard, SdpKind, SignedEnvelope};
use mist_protocol::settings::StreamSettings;
use mist_protocol::signaling::{ClientToServer, Role};
use mist_protocol::{decode, encode, Validate, MAX_DATA_CHANNEL_MESSAGE_BYTES};
use mist_session_manager::bitrate::{BitrateController, Decision, NetworkSample};
use mist_session_manager::pairing::{PairingCheck, PairingCodes, DEFAULT_CODE_TTL};
use mist_session_manager::session::DEFAULT_MAX_SESSION;
use mist_session_manager::trust::PairedDevice;
use mist_session_manager::{EndReason, Session, SessionError, SessionInfo, SessionManager, SessionState};
use mist_signaling_client::{SignalingConfig, SignalingEvent};
use parking_lot::Mutex;
use serde::Serialize;
use tokio::sync::{broadcast, mpsc, oneshot};

use crate::config::AudioChoice;
use crate::events::{HostEvent, HostStatsEvent};
use crate::link::{self, LinkEvent, LinkHandle};
use crate::AppContext;
use input_worker::InputWorker;

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
    DecideSession { session_id: String, approve: bool, reason: String },
    EndSession { session_id: String },
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
    pub input_allowed: bool,
    pub paired_devices: Vec<PairedDevice>,
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
}

impl SessionManager for HostHandle {
    fn sessions(&self) -> Vec<SessionInfo> {
        self.snapshot.lock().sessions.clone()
    }
    fn approve(&self, session_id: &str) -> Result<(), SessionError> {
        if !self.has_session(session_id) {
            return Err(SessionError::NotFound);
        }
        let _ = self.cmd.send(HostCmd::DecideSession {
            session_id: session_id.into(),
            approve: true,
            reason: String::new(),
        });
        Ok(())
    }
    fn reject(&self, session_id: &str, reason: &str) -> Result<(), SessionError> {
        if !self.has_session(session_id) {
            return Err(SessionError::NotFound);
        }
        let _ = self.cmd.send(HostCmd::DecideSession {
            session_id: session_id.into(),
            approve: false,
            reason: reason.into(),
        });
        Ok(())
    }
    fn end(&self, session_id: &str) -> Result<(), SessionError> {
        if !self.has_session(session_id) {
            return Err(SessionError::NotFound);
        }
        let _ = self.cmd.send(HostCmd::EndSession { session_id: session_id.into() });
        Ok(())
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
    MediaStarted {
        session_id: String,
        result: Result<(Arc<HostMedia>, mpsc::UnboundedReceiver<TransportEvent>, TargetRect), String>,
    },
    Stats {
        session_id: String,
        transport: Box<TransportStats>,
    },
}

struct ActiveSession {
    session: Session,
    client_id: DeviceId,
    client_name: String,
    settings: StreamSettings,
    media: Option<Arc<HostMedia>>,
    media_rx: Option<mpsc::UnboundedReceiver<TransportEvent>>,
    input: Option<InputWorker>,
    bitrate: BitrateController,
    client_stats: Option<ClientStats>,
    prev_client_dropped: u64,
    disconnected_since: Option<Instant>,
    last_heard: Option<Instant>,
    early_candidates: Vec<(u32, String)>,
}

impl ActiveSession {
    /// Connection interrupted: stop acting on stale input right away.
    fn interrupted(&mut self, why: &str) -> bool {
        if self.disconnected_since.is_some() {
            return false;
        }
        self.disconnected_since = Some(Instant::now());
        if let Some(i) = &self.input {
            i.release_all();
        }
        tracing::warn!("{why}; held input released, waiting for recovery");
        self.session.transition(SessionState::Reconnecting).is_ok()
    }
}

struct HostLoop {
    ctx: AppContext,
    link: LinkHandle,
    events: broadcast::Sender<HostEvent>,
    snapshot: Arc<Mutex<HostSnapshot>>,
    internal_tx: mpsc::UnboundedSender<Internal>,
    codes: PairingCodes,
    code_lookup: Option<String>,
    pending_pairings: HashMap<String, PendingPairing>,
    replay: ReplayGuard,
    active: Option<ActiveSession>,
    recently_failed: Option<(String, DeviceId, Instant)>,
    input_allowed: bool,
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
        paired_devices: ctx.trust.lock().list(),
        ..Default::default()
    }));
    let handle =
        HostHandle { cmd: cmd_tx, events: events.clone(), snapshot: snapshot.clone(), _alive: Arc::new(alive_tx) };

    tokio::task::spawn_blocking(|| {
        let _ = mist_media_engine::probe_encoders();
    });

    let sig_cfg = SignalingConfig {
        url: ctx.config.network.signaling_url.clone(),
        device_name: ctx.config.device_name.clone(),
        role: Role::Host,
        access_token: ctx.secrets.signaling_access_token.clone(),
        extra_ca_pem: ctx.config.network.extra_ca_pem.clone(),
    };
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
        pending_pairings: HashMap::new(),
        replay: ReplayGuard::default(),
        active: None,
        recently_failed: None,
        input_allowed,
    };
    tokio::spawn(state.run(cmd_rx, link_rx, internal_rx, alive_rx));
    handle
}

async fn recv_opt<T>(rx: &mut Option<mpsc::UnboundedReceiver<T>>) -> Option<T> {
    match rx {
        Some(r) => r.recv().await,
        None => std::future::pending().await,
    }
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
            Some(if cfg!(windows) {
                AudioSourceSpec::WasapiLoopback { device }
            } else {
                AudioSourceSpec::PulseMonitor { device }
            })
        }
    }
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
                Some(ev) = async { match self.active.as_mut() { Some(a) => recv_opt(&mut a.media_rx).await, None => std::future::pending().await } } => self.on_media(ev).await,
                _ = tick.tick() => self.on_tick().await,
                _ = &mut alive_rx => {
                    tracing::info!("host handle dropped; shutting down");
                    self.end_session(EndReason::HostStopped).await;
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
        s.sessions = self
            .active
            .iter()
            .map(|a| SessionInfo {
                session_id: a.session.id.clone(),
                client_id: a.client_id.clone(),
                client_name: a.client_name.clone(),
                state: a.session.state().clone(),
                input_allowed: self.input_allowed,
            })
            .collect();
        s.paired_devices = self.ctx.trust.lock().list();
    }

    async fn relay(&self, to: &DeviceId, msg: PeerMessage) {
        let env = SignedEnvelope::seal(&self.ctx.identity, Some(to.clone()), msg);
        if let Err(e) = self.link.send(ClientToServer::Relay { to: to.clone(), envelope: env }).await {
            tracing::warn!("cannot reach client through signaling: {e}");
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
                let reg = ClientToServer::RegisterPairing { lookup, ttl_secs: DEFAULT_CODE_TTL.as_secs() as u32 };
                if let Err(e) = self.link.send(reg).await {
                    self.emit(HostEvent::Warning {
                        message: format!("pairing code not registered yet ({e}); it will be registered on reconnect"),
                    });
                }
                self.emit(HostEvent::PairingCode { code: code.display(), expires_in_secs: DEFAULT_CODE_TTL.as_secs() });
            }
            HostCmd::CancelPairing => {
                self.codes.cancel();
                if let Some(old) = self.code_lookup.take() {
                    let _ = self.link.send(ClientToServer::CancelPairing { lookup: old }).await;
                }
            }
            HostCmd::DecidePairing { request_id, approve } => self.decide_pairing(&request_id, approve).await,
            HostCmd::DecideSession { session_id, approve, reason } => {
                let matches = self
                    .active
                    .as_ref()
                    .map(|a| a.session.id == session_id && *a.session.state() == SessionState::AwaitingApproval)
                    .unwrap_or(false);
                if !matches {
                    self.emit(HostEvent::Warning { message: "that session is no longer waiting for approval".into() });
                } else if approve {
                    self.accept_session().await;
                } else {
                    let client = self.active.as_ref().map(|a| a.client_id.clone()).expect("checked");
                    let reason = if reason.is_empty() { "declined by the host".to_string() } else { reason };
                    self.relay(&client, PeerMessage::SessionRejected { session_id, reason }).await;
                    self.finish_session(EndReason::RejectedByHost);
                }
            }
            HostCmd::EndSession { session_id } => {
                if self.active.as_ref().map(|a| a.session.id == session_id).unwrap_or(false) {
                    self.end_session(EndReason::KickedByHost).await;
                }
            }
            HostCmd::SetInputAllowed(allowed) => {
                self.input_allowed = allowed;
                if let Some(a) = &self.active {
                    if let Some(i) = &a.input {
                        i.set_allowed(allowed);
                    }
                    if let Some(m) = &a.media {
                        let _ = m.send(DataChannelKind::Control, &encode(&ControlMessage::InputPermission { allowed }));
                    }
                }
                tracing::info!(allowed, "remote input permission changed");
                self.emit(HostEvent::InputPermission { allowed });
            }
            HostCmd::DisconnectAll => self.end_session(EndReason::KickedByHost).await,
            HostCmd::Revoke(id) => {
                let removed = self.ctx.trust.lock().remove(&id);
                if let Err(e) = removed {
                    self.emit(HostEvent::Error { message: format!("could not update trust store: {e}") });
                }
                if self.active.as_ref().map(|a| a.client_id == id).unwrap_or(false) {
                    self.end_session(EndReason::KickedByHost).await;
                }
            }
        }
        self.update_snapshot();
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
                    if let Some(m) = &a.media {
                        if let Err(e) = m.set_remote_sdp(SdpKind::Answer, &sdp) {
                            tracing::warn!("bad answer: {e}");
                        }
                    }
                }
            }
            PeerMessage::IceCandidate { session_id, mline_index, candidate } => {
                if let Some(a) = self.session_for(&session_id, &from) {
                    match &a.media {
                        Some(m) => {
                            let _ = m.add_remote_candidate(mline_index, &candidate);
                        }
                        None => a.early_candidates.push((mline_index, candidate)),
                    }
                }
            }
            PeerMessage::EndSession { session_id, reason } => {
                if self.session_for(&session_id, &from).is_some() {
                    tracing::info!(%reason, "client ended the session");
                    self.end_session(EndReason::ClientDisconnected).await;
                }
            }
            other => tracing::debug!("ignoring peer message {other:?}"),
        }
    }

    fn session_for(&mut self, session_id: &str, from: &DeviceId) -> Option<&mut ActiveSession> {
        self.active.as_mut().filter(|a| a.session.id == session_id && &a.client_id == from)
    }

    async fn on_session_request(
        &mut self,
        device: PairedDevice,
        session_id: String,
        requested: StreamSettings,
        platform: String,
        resume: bool,
    ) {
        if let Some(a) = &self.active {
            if a.session.id == session_id && a.client_id == device.device_id {
                if resume && a.session.is_authorized() {
                    // The client abandoned its broken peer connection; build
                    // a new one for the same, already approved session.
                    self.restart_media().await;
                } else {
                    tracing::debug!("duplicate session request ignored");
                }
                return;
            }
            self.relay(
                &device.device_id,
                PeerMessage::SessionRejected { session_id, reason: "the host is busy with another session".into() },
            )
            .await;
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
        self.active = Some(ActiveSession {
            session,
            client_id: device.device_id.clone(),
            client_name: device.name.clone(),
            settings: settings.clone(),
            media: None,
            media_rx: None,
            input: None,
            bitrate,
            client_stats: None,
            prev_client_dropped: 0,
            disconnected_since: None,
            last_heard: None,
            early_candidates: Vec::new(),
        });
        let resumable = resume
            && self
                .recently_failed
                .as_ref()
                .map(|(id, client, t)| id == &session_id && client == &device.device_id && t.elapsed() < RESUME_WINDOW)
                .unwrap_or(false);
        tracing::info!(%session_id, client = %device.name, %platform, resume = resumable, "session requested");
        if resumable {
            tracing::info!("resuming previously approved session after a connection failure");
            self.accept_session().await;
        } else if self.ctx.config.host.trust_paired_sessions {
            tracing::info!("session auto-approved by host policy (trust_paired_sessions)");
            self.accept_session().await;
        } else {
            self.emit(HostEvent::SessionRequest {
                session_id: session_id.clone(),
                client_name: device.name.clone(),
                fingerprint: device.device_id.fingerprint(),
                settings,
            });
            self.emit_session_state();
        }
    }

    fn emit_session_state(&self) {
        if let Some(a) = &self.active {
            self.emit(HostEvent::Session {
                session_id: a.session.id.clone(),
                client_name: a.client_name.clone(),
                state: a.session.state().clone(),
            });
        }
    }

    async fn accept_session(&mut self) {
        let Some(a) = self.active.as_mut() else { return };
        if a.session.transition(SessionState::Negotiating).is_err() {
            return;
        }
        self.start_media().await;
    }

    /// Replace the peer connection of the active session (resume).
    async fn restart_media(&mut self) {
        let Some(a) = self.active.as_mut() else { return };
        tracing::info!(session = %a.session.id, "client is re-establishing the connection; restarting media");
        drop(a.input.take());
        if let Some(m) = a.media.take() {
            tokio::task::spawn_blocking(move || m.stop());
        }
        a.media_rx = None;
        a.last_heard = None;
        a.disconnected_since = None;
        a.early_candidates.clear();
        if *a.session.state() == SessionState::Connected {
            let _ = a.session.transition(SessionState::Reconnecting);
        }
        if *a.session.state() == SessionState::Reconnecting {
            let _ = a.session.transition(SessionState::Negotiating);
        }
        self.start_media().await;
    }

    async fn start_media(&mut self) {
        let Some(a) = self.active.as_mut() else { return };
        let session_id = a.session.id.clone();
        let client = a.client_id.clone();
        let settings = a.settings.clone();
        self.recently_failed = None;
        self.relay(
            &client,
            PeerMessage::SessionAccepted {
                session_id: session_id.clone(),
                expires_at_ms: now_ms() + DEFAULT_MAX_SESSION.as_millis() as u64,
                settings: settings.clone(),
                input_allowed: self.input_allowed,
            },
        )
        .await;
        self.emit_session_state();

        // Capture + pipeline setup may block (portal dialog, encoder probe).
        let host_cfg = self.ctx.config.host.clone();
        let mut ice = IceConfig { servers: self.link.ice_servers(), relay_only: self.ctx.config.network.relay_only };
        ice.servers.extend(self.ctx.config.network.ice_servers.iter().cloned());
        let tx = self.internal_tx.clone();
        tokio::task::spawn_blocking(move || {
            let result = (|| -> anyhow::Result<_> {
                let backend = match host_cfg.capture {
                    Some(kind) => mist_capture_api::backend_for(kind)?,
                    None => mist_capture_api::detect_backends()
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
                let target = match &capture {
                    CaptureSourceSpec::PipeWire { width, height, .. } => TargetRect {
                        x: 0,
                        y: 0,
                        width: width.unwrap_or(display.width.max(1)),
                        height: height.unwrap_or(display.height.max(1)),
                    },
                    _ => TargetRect { x: display.x, y: display.y, width: display.width, height: display.height },
                };
                let (etx, erx) = mpsc::unbounded_channel();
                let media = HostMedia::start(
                    HostMediaConfig { capture, audio: audio_spec(&host_cfg.audio), settings, ice },
                    etx,
                )?;
                Ok((media, erx, target))
            })();
            let _ = tx.send(Internal::MediaStarted { session_id, result: result.map_err(|e| e.to_string()) });
        });
    }

    async fn on_internal(&mut self, ev: Internal) {
        match ev {
            Internal::MediaStarted { session_id, result } => {
                let Some(a) = self.active.as_mut().filter(|a| a.session.id == session_id) else {
                    if let Ok((media, _, _)) = result {
                        media.stop();
                    }
                    return;
                };
                match result {
                    Ok((media, rx, target)) => {
                        for (m, c) in a.early_candidates.drain(..) {
                            let _ = media.add_remote_candidate(m, &c);
                        }
                        // Force feedback from games goes back to the client's controller.
                        let weak = Arc::downgrade(&media);
                        let on_rumble = move |r: mist_controller_api::Rumble| {
                            if let Some(m) = weak.upgrade() {
                                let msg = ControlMessage::Rumble {
                                    index: r.index,
                                    strong: r.strong,
                                    weak: r.weak,
                                    duration_ms: r.duration_ms,
                                };
                                let _ = m.send(DataChannelKind::Control, &encode(&msg));
                            }
                        };
                        let input = InputWorker::start(&self.ctx.config.host, target, self.input_allowed, on_rumble);
                        a.media = Some(media);
                        a.media_rx = Some(rx);
                        match input {
                            Ok(w) => a.input = Some(w),
                            Err(e) => {
                                let msg = format!("remote input unavailable: {e}");
                                tracing::warn!("{msg}");
                                self.emit(HostEvent::Warning { message: msg });
                            }
                        }
                    }
                    Err(e) => {
                        tracing::error!("cannot start streaming: {e}");
                        self.emit(HostEvent::Error { message: format!("cannot start streaming: {e}") });
                        self.end_session(EndReason::Error(e)).await;
                    }
                }
            }
            Internal::Stats { session_id, transport } => self.on_stats(session_id, *transport).await,
        }
        self.update_snapshot();
    }

    // ------------------------------------------------------------------- media

    async fn on_media(&mut self, ev: TransportEvent) {
        let Some(a) = self.active.as_mut() else { return };
        let client = a.client_id.clone();
        let session_id = a.session.id.clone();
        if let TransportEvent::ChannelMessage(..) = ev {
            a.last_heard = Some(Instant::now());
            if a.disconnected_since.take().is_some() && a.session.transition(SessionState::Connected).is_ok() {
                tracing::info!("peer is reachable again");
                self.emit_session_state();
            }
        }
        let Some(a) = self.active.as_mut() else { return };
        match ev {
            TransportEvent::LocalSdp { kind, sdp } => {
                self.relay(&client, PeerMessage::Sdp { session_id, kind, sdp }).await;
            }
            TransportEvent::LocalCandidate { mline_index, candidate } => {
                self.relay(&client, PeerMessage::IceCandidate { session_id, mline_index, candidate }).await;
            }
            TransportEvent::PeerState(ConnectionState::Connected) => {
                a.disconnected_since = None;
                if a.session.transition(SessionState::Connected).is_ok() {
                    let _ = self.ctx.trust.lock().touch(&client, now_ms());
                    tracing::info!(%session_id, "peer connected; streaming");
                    self.emit_session_state();
                }
            }
            TransportEvent::PeerState(ConnectionState::Disconnected) => {
                if a.interrupted("peer connection interrupted") {
                    self.emit_session_state();
                }
            }
            TransportEvent::PeerState(ConnectionState::Failed) => {
                self.recently_failed = Some((session_id, client, Instant::now()));
                self.end_session(EndReason::ConnectionFailed("ICE/DTLS connection failed".into())).await;
            }
            TransportEvent::ChannelOpen(DataChannelKind::Control) => {
                if let Some(m) = &a.media {
                    let _ = m.send(
                        DataChannelKind::Control,
                        &encode(&ControlMessage::InputPermission { allowed: self.input_allowed }),
                    );
                }
            }
            TransportEvent::ChannelMessage(DataChannelKind::Input | DataChannelKind::Gamepad, text) => {
                if let Some(i) = &a.input {
                    i.message(text);
                }
            }
            TransportEvent::ChannelMessage(DataChannelKind::Control, text) => {
                match decode::<ControlMessage>(text.as_bytes(), MAX_DATA_CHANNEL_MESSAGE_BYTES) {
                    Ok(ControlMessage::Ping { id, client_time_us }) => {
                        if let Some(m) = &a.media {
                            let pong = ControlMessage::Pong {
                                id,
                                client_time_us,
                                host_time_us: mist_media_engine::timing::now_us(),
                            };
                            let _ = m.send(DataChannelKind::Control, &encode(&pong));
                        }
                    }
                    Ok(ControlMessage::ClientStats(s)) => a.client_stats = Some(s),
                    Ok(ControlMessage::RequestKeyframe {}) => {
                        if let Some(m) = &a.media {
                            m.force_keyframe();
                        }
                    }
                    Ok(ControlMessage::Goodbye { .. }) => self.end_session(EndReason::ClientDisconnected).await,
                    Ok(other) => tracing::debug!("unexpected control message {other:?}"),
                    Err(e) => tracing::debug!("invalid control message: {e}"),
                }
            }
            TransportEvent::Warning(w) => {
                tracing::warn!("{w}");
                self.emit(HostEvent::Warning { message: w });
            }
            TransportEvent::Error(e) => {
                tracing::error!("media error: {e}");
                self.emit(HostEvent::Error { message: e.clone() });
                self.end_session(EndReason::Error(e)).await;
            }
            _ => {}
        }
        self.update_snapshot();
    }

    async fn on_stats(&mut self, session_id: String, transport: TransportStats) {
        let Some(a) = self.active.as_mut().filter(|a| a.session.id == session_id) else { return };
        let Some(media) = a.media.clone() else { return };
        if a.settings.adaptive_bitrate {
            let dropped = a.client_stats.as_ref().map(|c| c.frames_dropped).unwrap_or(0);
            let sample = NetworkSample {
                fraction_lost: transport.remote_fraction_lost,
                rtt_ms: transport.remote_rtt_ms,
                client_dropped_frames: dropped.saturating_sub(a.prev_client_dropped),
            };
            a.prev_client_dropped = dropped;
            match a.bitrate.update(sample) {
                Decision::Decrease(k) | Decision::Increase(k) => media.set_bitrate(k),
                Decision::Hold => {}
            }
        }
        let host = media.host_stats();
        let _ = media.send(DataChannelKind::Control, &encode(&ControlMessage::HostStats(host.clone())));
        let (input, kb, pads) = a
            .input
            .as_ref()
            .and_then(|i| i.stats())
            .map(|(s, k, p)| (s, k.to_string(), p.to_string()))
            .unwrap_or_else(|| (Default::default(), "unavailable".into(), "unavailable".into()));
        let ev = HostStatsEvent {
            session_id,
            host,
            client: a.client_stats.clone(),
            transport,
            encoder_hardware: media.encoder_hardware(),
            input_backend: kb,
            controller_backend: pads,
            input,
        };
        self.emit(HostEvent::Stats(Box::new(ev)));
    }

    async fn on_tick(&mut self) {
        let now = Instant::now();
        self.pending_pairings.retain(|_, p| now.duration_since(p.created) < PAIRING_APPROVAL_TIMEOUT);
        if self.code_lookup.is_some() && self.codes.current().map(|(_, e)| e <= now).unwrap_or(true) {
            self.code_lookup = None;
            self.codes.cancel();
            self.emit(HostEvent::PairingCodeExpired);
        }
        let mut end = None;
        let mut changed = false;
        if let Some(a) = self.active.as_mut() {
            let silent = a.last_heard.map(|t| now.duration_since(t) > PEER_SILENCE).unwrap_or(false);
            if silent && *a.session.state() == SessionState::Connected {
                changed = a.interrupted("no data from the client");
            }
            if let Some(r) = a.session.check_timeouts(now) {
                end = Some(r);
            } else if a.disconnected_since.map(|t| now.duration_since(t) > RECONNECT_GRACE).unwrap_or(false) {
                self.recently_failed = Some((a.session.id.clone(), a.client_id.clone(), now));
                end = Some(EndReason::ConnectionFailed("connection lost".into()));
            } else if let Some(m) = &a.media {
                let rx = m.request_stats();
                let tx = self.internal_tx.clone();
                let session_id = a.session.id.clone();
                tokio::spawn(async move {
                    if let Ok(Ok(transport)) = tokio::time::timeout(Duration::from_millis(800), rx).await {
                        let _ = tx.send(Internal::Stats { session_id, transport: Box::new(transport) });
                    }
                });
            }
        }
        if changed {
            self.emit_session_state();
        }
        if let Some(r) = end {
            self.end_session(r).await;
        }
        self.update_snapshot();
    }

    /// Notify the client, tear down media and input, record the reason.
    async fn end_session(&mut self, reason: EndReason) {
        let Some(a) = self.active.as_mut() else { return };
        // Release held input before anything that may wait on the network.
        drop(a.input.take());
        let session_id = a.session.id.clone();
        let client = a.client_id.clone();
        if let Some(m) = &a.media {
            let _ =
                m.send(DataChannelKind::Control, &encode(&ControlMessage::Goodbye { reason: format!("{reason:?}") }));
        }
        if !matches!(reason, EndReason::ClientDisconnected) {
            self.relay(&client, PeerMessage::EndSession { session_id, reason: format!("{reason:?}") }).await;
        }
        self.finish_session(reason);
    }

    fn finish_session(&mut self, reason: EndReason) {
        let Some(mut a) = self.active.take() else { return };
        // Input first: releases held keys/buttons before anything else.
        drop(a.input.take());
        if let Some(m) = a.media.take() {
            tokio::task::spawn_blocking(move || m.stop());
        }
        let _ = a.session.transition(SessionState::Ended(reason.clone()));
        tracing::info!(session = %a.session.id, ?reason, "session ended");
        self.emit(HostEvent::Session {
            session_id: a.session.id.clone(),
            client_name: a.client_name,
            state: a.session.state().clone(),
        });
        self.update_snapshot();
    }
}

/// Capture kinds a host can offer in this session (for UIs).
pub fn available_capture_backends() -> Vec<(CaptureKind, Vec<mist_capture_api::DisplayInfo>, Option<String>)> {
    let mut out = Vec::new();
    for b in mist_capture_api::detect_backends() {
        match b.displays() {
            Ok(d) => out.push((b.kind(), d, None)),
            Err(e) => out.push((b.kind(), Vec::new(), Some(e.to_string()))),
        }
    }
    out
}
