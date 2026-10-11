//! Client session runner.
//!
//! Flow: signaling connect -> SessionRequest -> wait for host consent ->
//! WebRTC negotiation (host offers) -> streaming. Recoverable failures
//! (ICE failure, network change, signaling drop) lead to a new peer
//! connection with `resume = true`, up to the reconnect policy's limit.

use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

use mist_controller_api::source::{ControllerEvent, ControllerSource};
use mist_media_engine::{
    ClientMedia, ClientMediaConfig, ConnectionState, DataChannelKind, FrameSink, IceConfig, MediaTransport,
    TransportEvent, TransportStats, VideoOutput, WindowTarget,
};
use mist_protocol::control::{ClientStats, ControlMessage, HostStats};
use mist_protocol::identity::{random_hex, DeviceId};
use mist_protocol::input::InputMessage;
use mist_protocol::peer::{ClientCapabilities, PeerMessage, ReplayGuard, SdpKind, SignedEnvelope};
use mist_protocol::signaling::{ClientToServer, Role};
use mist_protocol::{decode, encode, MAX_DATA_CHANNEL_MESSAGE_BYTES};
use mist_session_manager::reconnect::ReconnectPolicy;
use mist_signaling_client::{SignalingConfig, SignalingEvent};
use tokio::sync::{broadcast, mpsc};

use super::shared::{ClientCmd, ClientShared};
use crate::config::Renderer;
use crate::events::{ClientEvent, ClientState, ClientStatsEvent};
use crate::link::{self, LinkEvent, LinkHandle};
use crate::AppContext;

/// Probe the GPU renderer once per window and remember the outcome.
/// `force` accepts a working but slow renderer.
fn gpu_renderer_works(target: WindowTarget, force: bool) -> Result<(), String> {
    type Probe = Option<(WindowTarget, Result<f64, String>)>;
    static CHECKED: parking_lot::Mutex<Probe> = parking_lot::Mutex::new(None);
    let mut checked = CHECKED.lock();
    let measured = match checked.as_ref() {
        Some((t, r)) if *t == target => r.clone(),
        _ => {
            let r = mist_media_engine::window_sink_works(target);
            match &r {
                Ok(ms) => tracing::info!("GPU video renderer: {ms:.2} ms per 1080p frame"),
                Err(e) => tracing::warn!("GPU video renderer does not work: {e}"),
            }
            *checked = Some((target, r.clone()));
            r
        }
    }?;
    let limit = mist_media_engine::client::MAX_GPU_FRAME_MS;
    if measured > limit && !force {
        return Err(format!(
            "OpenGL needs {measured:.1} ms per 1080p frame (limit {limit} ms), probably software rendering"
        ));
    }
    Ok(())
}

const APPROVAL_TIMEOUT: Duration = Duration::from_secs(90);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
const DISCONNECT_GRACE: Duration = Duration::from_secs(6);

pub struct SessionOptions {
    pub host: DeviceId,
    pub controllers: bool,
}

enum AttemptEnd {
    /// Clean end (user, host or rejection); do not retry.
    Final(String),
    /// Recoverable failure; retry with resume.
    Retry(String),
}

struct Runner {
    ctx: AppContext,
    host: DeviceId,
    link: LinkHandle,
    link_rx: mpsc::Receiver<LinkEvent>,
    shared: Arc<ClientShared>,
    sink: Arc<dyn FrameSink>,
    events: broadcast::Sender<ClientEvent>,
    cmd_rx: mpsc::UnboundedReceiver<ClientCmd>,
    pad_rx: mpsc::UnboundedReceiver<ControllerEvent>,
    controllers: Option<ControllerSource>,
    replay: ReplayGuard,
    host_stats: Option<HostStats>,
    overlay: bool,
    muted: bool,
    last_bytes: Option<(Instant, u64)>,
}

/// Run a session until it ends. Returns the final reason.
pub async fn run(
    ctx: AppContext,
    opts: SessionOptions,
    shared: Arc<ClientShared>,
    sink: Arc<dyn FrameSink>,
    events: broadcast::Sender<ClientEvent>,
    cmd_rx: mpsc::UnboundedReceiver<ClientCmd>,
) -> anyhow::Result<String> {
    let host = ctx
        .trust
        .lock()
        .get(&opts.host)
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("host {} is not paired; pair with a code first", opts.host))?;
    let url = host.signaling_url.clone().unwrap_or_else(|| ctx.config.network.signaling_url.clone());
    let sig_cfg = SignalingConfig {
        url,
        device_name: ctx.config.device_name.clone(),
        role: Role::Client,
        access_token: ctx.secrets.signaling_access_token.clone(),
        extra_ca_pem: ctx.config.network.extra_ca_pem.clone(),
    };
    let (link, link_rx) = link::spawn(sig_cfg, ctx.identity.clone());

    // Controllers: gilrs thread -> tokio channel.
    let (pad_tx, pad_rx) = mpsc::unbounded_channel();
    let controllers = if opts.controllers && ctx.config.client.controllers_enabled {
        let (std_tx, std_rx) = std::sync::mpsc::channel();
        match ControllerSource::start(ctx.config.client.controller_mapping.clone(), std_tx) {
            Ok(src) => {
                std::thread::spawn(move || {
                    while let Ok(ev) = std_rx.recv() {
                        if pad_tx.send(ev).is_err() {
                            break;
                        }
                    }
                });
                Some(src)
            }
            Err(e) => {
                let _ = events.send(ClientEvent::Warning { message: format!("controllers unavailable: {e}") });
                None
            }
        }
    } else {
        None
    };

    let overlay = ctx.config.client.show_stats_overlay;
    let muted = ctx.config.client.muted;
    let mut r = Runner {
        ctx,
        host: opts.host,
        link,
        link_rx,
        shared,
        sink,
        events,
        cmd_rx,
        pad_rx,
        controllers,
        replay: ReplayGuard::default(),
        host_stats: None,
        overlay,
        muted,
        last_bytes: None,
    };
    let result = r.run().await;
    let (state, detail) = match &result {
        Ok(reason) => (ClientState::Ended, reason.clone()),
        Err(e) => (ClientState::Failed, e.to_string()),
    };
    r.emit(ClientEvent::State { state, detail });
    result
}

impl Runner {
    fn emit(&self, ev: ClientEvent) {
        let _ = self.events.send(ev);
    }

    fn state(&self, state: ClientState, detail: impl Into<String>) {
        let detail = detail.into();
        tracing::info!(?state, "{detail}");
        self.emit(ClientEvent::State { state, detail });
    }

    async fn relay(&self, msg: PeerMessage) {
        let env = SignedEnvelope::seal(&self.ctx.identity, Some(self.host.clone()), msg);
        if let Err(e) = self.link.send(ClientToServer::Relay { to: self.host.clone(), envelope: env }).await {
            tracing::warn!("signaling send failed: {e}");
        }
    }

    async fn wait_link(&mut self) -> anyhow::Result<()> {
        self.state(
            ClientState::ConnectingSignaling,
            format!("connecting to {}", self.ctx.config.network.signaling_url),
        );
        let deadline = tokio::time::Instant::now() + CONNECT_TIMEOUT;
        loop {
            if self.link.connected() {
                return Ok(());
            }
            tokio::select! {
                ev = self.link_rx.recv() => match ev {
                    Some(LinkEvent::Connected { .. }) => return Ok(()),
                    Some(LinkEvent::Disconnected { reason, retry_in }) => {
                        self.emit(ClientEvent::Warning { message: format!("signaling: {reason}") });
                        if retry_in.is_none() {
                            anyhow::bail!("cannot reach signaling server: {reason}");
                        }
                    }
                    Some(_) => {}
                    None => anyhow::bail!("signaling stopped"),
                },
                Some(ClientCmd::Disconnect) = self.cmd_rx.recv() => anyhow::bail!("cancelled"),
                _ = tokio::time::sleep_until(deadline) => anyhow::bail!("cannot reach signaling server {} (timed out)", self.ctx.config.network.signaling_url),
            }
        }
    }

    async fn run(&mut self) -> anyhow::Result<String> {
        self.wait_link().await?;
        let session_id = random_hex(16);
        let mut policy = ReconnectPolicy::session();
        let mut resume = false;
        loop {
            match self.attempt(&session_id, resume).await? {
                AttemptEnd::Final(reason) => return Ok(reason),
                AttemptEnd::Retry(reason) => {
                    let Some(delay) = policy.next_delay() else {
                        anyhow::bail!("connection lost and could not be re-established: {reason}");
                    };
                    self.state(ClientState::Reconnecting, format!("{reason}; retrying in {:.1}s", delay.as_secs_f32()));
                    tokio::time::sleep(delay).await;
                    if !self.link.connected() {
                        self.wait_link().await?;
                    }
                    resume = true;
                }
            }
        }
    }

    fn capabilities() -> ClientCapabilities {
        let decoders =
            mist_media_engine::probe_decoders().iter().filter(|d| d.available).map(|d| d.element.clone()).collect();
        ClientCapabilities { decoders, max_width: 7680, max_height: 4320, platform: std::env::consts::OS.into() }
    }

    async fn attempt(&mut self, session_id: &str, resume: bool) -> anyhow::Result<AttemptEnd> {
        let caps = tokio::task::spawn_blocking(Self::capabilities).await?;
        self.relay(PeerMessage::SessionRequest {
            session_id: session_id.into(),
            settings: self.ctx.config.client.stream.clone(),
            capabilities: caps,
            resume,
        })
        .await;
        if !resume {
            self.state(ClientState::WaitingForApproval, "waiting for the host to approve the session");
        }

        // ---- wait for consent
        let deadline = tokio::time::Instant::now() + APPROVAL_TIMEOUT;
        let accepted = loop {
            tokio::select! {
                ev = self.link_rx.recv() => match ev {
                    Some(LinkEvent::Signal(SignalingEvent::Relay { from, from_public_key, envelope })) => {
                        match self.open(from, from_public_key, envelope) {
                            Some(PeerMessage::SessionAccepted { session_id: id, settings, input_allowed, .. }) if id == session_id => break (settings, input_allowed),
                            Some(PeerMessage::SessionRejected { session_id: id, reason }) if id == session_id => return Ok(AttemptEnd::Final(format!("rejected by host: {reason}"))),
                            Some(other) => tracing::debug!("ignoring {other:?} while waiting for approval"),
                            None => {}
                        }
                    }
                    Some(LinkEvent::Signal(SignalingEvent::ServerError { message, .. })) => {
                        if message.contains("not connected") {
                            return Ok(AttemptEnd::Retry("host is offline".into()));
                        }
                    }
                    Some(LinkEvent::Disconnected { reason, .. }) => return Ok(AttemptEnd::Retry(format!("signaling lost: {reason}"))),
                    Some(_) => {}
                    None => anyhow::bail!("signaling stopped"),
                },
                cmd = self.cmd_rx.recv() => if let Some(ClientCmd::Disconnect) | None = cmd {
                    self.relay(PeerMessage::EndSession { session_id: session_id.into(), reason: "cancelled by user".into() }).await;
                    return Ok(AttemptEnd::Final("cancelled".into()));
                },
                _ = tokio::time::sleep_until(deadline) => return Ok(AttemptEnd::Final("the host did not approve the session in time".into())),
            }
        };
        let (settings, input_allowed) = accepted;
        self.shared.input_allowed.store(input_allowed, Ordering::Relaxed);
        self.emit(ClientEvent::InputPermission { allowed: input_allowed });
        self.state(
            ClientState::Negotiating,
            format!(
                "approved: {}x{}@{} up to {} kbps",
                settings.width, settings.height, settings.fps, settings.max_bitrate_kbps
            ),
        );

        // ---- media
        let mut ice = IceConfig { servers: self.link.ice_servers(), relay_only: self.ctx.config.network.relay_only };
        ice.servers.extend(self.ctx.config.network.ice_servers.iter().cloned());
        let (etx, mut erx) = mpsc::unbounded_channel();
        let cfg = ClientMediaConfig {
            ice,
            decoder: self.ctx.config.client.decoder.clone(),
            jitter_latency_ms: self.ctx.config.client.jitter_buffer_ms,
            audio_enabled: true,
        };
        let sink = self.sink.clone();
        let renderer = self.ctx.config.client.renderer;
        let target = match renderer {
            Renderer::Auto | Renderer::Gpu => *self.shared.video_target.lock(),
            Renderer::Software => None,
        };
        let shared = self.shared.clone();
        let events = self.events.clone();
        let media = tokio::task::spawn_blocking(move || {
            let output = match target.map(|t| (t, gpu_renderer_works(t, renderer == Renderer::Gpu))) {
                Some((target, Ok(()))) => {
                    let s = shared.clone();
                    VideoOutput::Window { target, on_present: Arc::new(move |times| s.on_presented(times)) }
                }
                Some((_, Err(e))) => {
                    let _ = events.send(ClientEvent::Warning {
                        message: format!("GPU video renderer unavailable ({e}); using the software renderer"),
                    });
                    VideoOutput::Frames(sink)
                }
                None => VideoOutput::Frames(sink),
            };
            let gpu = output.is_window();
            shared.gpu_presenting.store(gpu, Ordering::Relaxed);
            tracing::info!(gpu_renderer = gpu, "video output");
            ClientMedia::start(cfg, etx, output)
        })
        .await??;
        media.set_volume(self.ctx.config.client.volume);
        media.set_muted(self.muted);
        *self.shared.media.write() = Some(media.clone());
        *self.shared.clock.lock() = Default::default();

        let end = self.stream(session_id, &media, &mut erx).await;
        *self.shared.media.write() = None;
        let m = media.clone();
        tokio::task::spawn_blocking(move || m.stop()).await.ok();
        end
    }

    fn open(
        &mut self,
        from: DeviceId,
        pk: mist_protocol::identity::PublicKeyB64,
        env: SignedEnvelope,
    ) -> Option<PeerMessage> {
        if from != self.host {
            return None;
        }
        // Only the key we paired with may speak for this host.
        match self.ctx.trust.lock().verify(&from, &pk) {
            Ok(Some(_)) => {}
            _ => {
                tracing::warn!("message claiming to be from the host has an unexpected key; ignored");
                return None;
            }
        }
        let me = self.ctx.identity.device_id();
        match env.open(&pk, Some(&me)).and_then(|b| self.replay.check(&b).map(|_| b)) {
            Ok(b) => Some(b.message),
            Err(e) => {
                tracing::warn!("rejected envelope from host: {e}");
                None
            }
        }
    }

    async fn stream(
        &mut self,
        session_id: &str,
        media: &Arc<ClientMedia>,
        erx: &mut mpsc::UnboundedReceiver<TransportEvent>,
    ) -> anyhow::Result<AttemptEnd> {
        let mut tick = tokio::time::interval(Duration::from_secs(1));
        let mut ping_id = 0u32;
        let mut disconnected_since: Option<Instant> = None;
        let negotiate_deadline = Instant::now() + Duration::from_secs(30);
        let mut connected = false;
        let mut last_heard = Instant::now();
        let mut last_decoded = 0u64;
        loop {
            tokio::select! {
                ev = self.link_rx.recv() => match ev {
                    Some(LinkEvent::Signal(SignalingEvent::Relay { from, from_public_key, envelope })) => {
                        match self.open(from, from_public_key, envelope) {
                            Some(PeerMessage::Sdp { session_id: id, kind: SdpKind::Offer, sdp }) if id == session_id => {
                                media.set_remote_sdp(SdpKind::Offer, &sdp)?;
                            }
                            Some(PeerMessage::IceCandidate { session_id: id, mline_index, candidate }) if id == session_id => {
                                let _ = media.add_remote_candidate(mline_index, &candidate);
                            }
                            Some(PeerMessage::EndSession { session_id: id, reason }) if id == session_id => {
                                if reason.contains("ConnectionFailed") {
                                    return Ok(AttemptEnd::Retry(format!("host reported: {reason}")));
                                }
                                return Ok(AttemptEnd::Final(format!("host ended the session ({reason})")));
                            }
                            _ => {}
                        }
                    }
                    Some(LinkEvent::Disconnected { reason, .. }) => {
                        // Media keeps flowing without signaling; only matters
                        // if the peer connection also fails.
                        self.emit(ClientEvent::Warning { message: format!("signaling connection lost ({reason}); reconnecting") });
                    }
                    Some(_) => {}
                    None => anyhow::bail!("signaling stopped"),
                },
                Some(ev) = erx.recv() => match ev {
                    TransportEvent::LocalSdp { kind, sdp } => {
                        self.relay(PeerMessage::Sdp { session_id: session_id.into(), kind, sdp }).await;
                    }
                    TransportEvent::LocalCandidate { mline_index, candidate } => {
                        self.relay(PeerMessage::IceCandidate { session_id: session_id.into(), mline_index, candidate }).await;
                    }
                    TransportEvent::PeerState(ConnectionState::Connected) => {
                        disconnected_since = None;
                        connected = true;
                        self.state(ClientState::Streaming, "connected; streaming");
                    }
                    TransportEvent::PeerState(ConnectionState::Disconnected) => {
                        disconnected_since.get_or_insert_with(Instant::now);
                        self.state(ClientState::Reconnecting, "connection interrupted; waiting for recovery");
                    }
                    TransportEvent::PeerState(ConnectionState::Failed) => {
                        return Ok(AttemptEnd::Retry("peer connection failed".into()));
                    }
                    TransportEvent::ChannelOpen(DataChannelKind::Control) => {
                        // A fresh connection: re-announce controllers.
                        if let Some(src) = &self.controllers {
                            for c in src.controllers() {
                                self.shared.send_input(InputMessage::GamepadConnected { index: c.slot, kind: c.kind });
                            }
                        }
                    }
                    TransportEvent::ChannelMessage(DataChannelKind::Control, text) => {
                        last_heard = Instant::now();
                        if disconnected_since.take().is_some() {
                            self.state(ClientState::Streaming, "connection recovered");
                        }
                        if let Some(end) = self.on_control(&text) {
                            return Ok(end);
                        }
                    }
                    TransportEvent::Warning(w) => self.emit(ClientEvent::Warning { message: w }),
                    TransportEvent::Error(e) => return Ok(AttemptEnd::Retry(format!("media error: {e}"))),
                    _ => {}
                },
                Some(ev) = self.pad_rx.recv() => self.on_controller(ev),
                cmd = self.cmd_rx.recv() => match cmd {
                    Some(ClientCmd::Disconnect) | None => {
                        self.shared.send_input(InputMessage::ReleaseAll {});
                        let _ = media.send(DataChannelKind::Control, &encode(&ControlMessage::Goodbye { reason: "client disconnected".into() }));
                        self.relay(PeerMessage::EndSession { session_id: session_id.into(), reason: "client disconnected".into() }).await;
                        return Ok(AttemptEnd::Final("disconnected".into()));
                    }
                    Some(ClientCmd::SetVolume(v)) => media.set_volume(v),
                    Some(ClientCmd::ToggleMute) => {
                        self.muted = !self.muted;
                        media.set_muted(self.muted);
                    }
                    Some(ClientCmd::ToggleOverlay) => {
                        self.overlay = !self.overlay;
                        if !self.overlay {
                            media.set_overlay_text(None);
                        }
                    }
                },
                _ = tick.tick() => {
                    ping_id += 1;
                    let ping = ControlMessage::Ping { id: ping_id, client_time_us: mist_media_engine::timing::now_us() };
                    let _ = media.send(DataChannelKind::Control, &encode(&ping));
                    if !connected && Instant::now() > negotiate_deadline {
                        return Ok(AttemptEnd::Retry("could not establish a peer connection (ICE). Check firewall/TURN settings".into()));
                    }
                    // Decoded video also proves the path works (the reliable
                    // control channel can stall under heavy loss).
                    let decoded = media.frames_decoded();
                    if decoded > last_decoded {
                        last_decoded = decoded;
                        last_heard = Instant::now();
                        if disconnected_since.take().is_some() {
                            self.state(ClientState::Streaming, "connection recovered");
                        }
                    }
                    if connected && disconnected_since.is_none() && last_heard.elapsed() > Duration::from_secs(3) {
                        disconnected_since = Some(Instant::now());
                        self.shared.send_input(InputMessage::ReleaseAll {});
                        self.state(ClientState::Reconnecting, "no data from the host; waiting for recovery");
                    }
                    if disconnected_since.map(|t| t.elapsed() > DISCONNECT_GRACE).unwrap_or(false) {
                        return Ok(AttemptEnd::Retry("connection lost".into()));
                    }
                    if connected {
                        let transport = tokio::time::timeout(Duration::from_millis(500), media.request_stats()).await.ok().and_then(|r| r.ok()).unwrap_or_default();
                        self.report_stats(media, transport);
                    }
                }
            }
        }
    }

    fn on_control(&mut self, text: &str) -> Option<AttemptEnd> {
        match decode::<ControlMessage>(text.as_bytes(), MAX_DATA_CHANNEL_MESSAGE_BYTES) {
            Ok(ControlMessage::Pong { client_time_us, host_time_us, .. }) => {
                self.shared.clock.lock().add_sample(client_time_us, host_time_us, mist_media_engine::timing::now_us());
            }
            Ok(ControlMessage::HostStats(s)) => {
                *self.shared.stream_size.lock() = Some((s.width, s.height));
                self.host_stats = Some(s);
            }
            Ok(ControlMessage::InputPermission { allowed }) => {
                self.shared.input_allowed.store(allowed, Ordering::Relaxed);
                self.emit(ClientEvent::InputPermission { allowed });
            }
            Ok(ControlMessage::Goodbye { reason }) => {
                return Some(AttemptEnd::Final(format!("host ended the session ({reason})")));
            }
            Ok(other) => tracing::debug!("unexpected control message {other:?}"),
            Err(e) => tracing::debug!("invalid control message: {e}"),
        }
        None
    }

    fn on_controller(&mut self, ev: ControllerEvent) {
        match ev {
            ControllerEvent::Connected { slot, kind, .. } => {
                self.shared.send_input(InputMessage::GamepadConnected { index: slot, kind });
            }
            ControllerEvent::State { slot, state } => {
                let seq = self.shared.next_gamepad_seq();
                self.shared.send_input(InputMessage::Gamepad { index: slot, seq, state });
            }
            ControllerEvent::Disconnected { slot } => {
                self.shared.send_input(InputMessage::GamepadDisconnected { index: slot });
            }
        }
        if let Some(src) = &self.controllers {
            self.emit(ClientEvent::Controllers { controllers: src.controllers() });
        }
    }

    fn report_stats(&mut self, media: &Arc<ClientMedia>, transport: TransportStats) {
        let video = transport.inbound_video.clone().unwrap_or_default();
        let now = Instant::now();
        let bytes = video.bytes.unwrap_or(0);
        let kbps = self.last_bytes.and_then(|(t, b)| {
            let dt = now.duration_since(t).as_secs_f32();
            (dt > 0.0 && bytes >= b).then(|| (bytes - b) as f32 * 8.0 / 1000.0 / dt)
        });
        self.last_bytes = Some((now, bytes));
        let clock = self.shared.clock.lock();
        let rtt_ms = clock.latest_rtt_us().map(|r| r as f32 / 1000.0);
        let uncertainty = clock.uncertainty_us().map(|u| u as f32 / 1000.0);
        drop(clock);
        let stats = ClientStats {
            rendered_fps: self.shared.present_rate.lock().rate() as f32,
            frames_decoded: media.frames_decoded(),
            frames_dropped: self.shared.frames_dropped.load(Ordering::Relaxed),
            packets_lost: video.packets_lost.unwrap_or(0),
            packets_received: video.packets.unwrap_or(0),
            jitter_ms: video.jitter_ms.unwrap_or(0.0).max(0.0) as f32,
            decode_ms_avg: media.decode_ms_avg().unwrap_or(0.0) as f32,
            latency_ms: self.shared.recent_latency_ms(),
        };
        let _ = media.send(DataChannelKind::Control, &encode(&ControlMessage::ClientStats(stats.clone())));
        let breakdown = self.shared.latency_breakdown();
        if self.overlay {
            let host = self.host_stats.as_ref();
            let stage = |v: Option<f32>| v.map(|v| format!("{v:.1}")).unwrap_or_else(|| "?".into());
            let text = format!(
                "{}x{} {} | {:.0} fps | {:.1} Mbps\nlatency {} (±{}) | rtt {} | decode {:.1} ms\nhost {} + net {} + dec {} + conv {} + present {} ms\nloss {} | dropped {} | encoder {} {:.1} ms | {}",
                host.map(|h| h.width).unwrap_or(0),
                host.map(|h| h.height).unwrap_or(0),
                media.decoder_name(),
                stats.rendered_fps,
                kbps.unwrap_or(0.0) / 1000.0,
                stats.latency_ms.map(|l| format!("{l:.0} ms")).unwrap_or_else(|| "n/a".into()),
                uncertainty.map(|u| format!("{u:.0} ms")).unwrap_or_else(|| "?".into()),
                rtt_ms.map(|r| format!("{r:.0} ms")).unwrap_or_else(|| "n/a".into()),
                stats.decode_ms_avg,
                stage(breakdown.host_ms),
                stage(breakdown.network_ms),
                stage(breakdown.decode_ms),
                stage(breakdown.convert_ms),
                stage(breakdown.present_ms),
                stats.packets_lost,
                stats.frames_dropped,
                host.map(|h| h.encoder.as_str()).unwrap_or("?"),
                host.map(|h| h.encode_ms_avg).unwrap_or(0.0),
                match transport.relayed() {
                    Some(true) => "TURN relay",
                    Some(false) => "direct",
                    None => "path ?",
                },
            );
            media.set_overlay_text(Some(&text));
        }
        let ev = ClientStatsEvent {
            stats,
            rtt_ms,
            latency_uncertainty_ms: uncertainty,
            host: self.host_stats.clone(),
            relayed: transport.relayed(),
            transport,
            video_bitrate_kbps: kbps,
            decoder: media.decoder_name().to_string(),
            latency_breakdown: breakdown,
        };
        self.emit(ClientEvent::Stats(Box::new(ev)));
    }
}
