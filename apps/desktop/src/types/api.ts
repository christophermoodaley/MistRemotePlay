// Types mirroring the Rust structs serialized by the Tauri commands and
// events (crates/app-core). Keep in sync with serde attributes there.

export type QualityPreset = "low_bandwidth" | "low_latency" | "balanced" | "high_quality" | "custom";
export type EncoderQuality = "fastest" | "balanced" | "quality";

export interface AudioSettings {
  enabled: boolean;
  bitrate_kbps: number;
  channels: number;
}

export interface StreamSettings {
  preset: QualityPreset;
  width: number;
  height: number;
  fps: number;
  max_bitrate_kbps: number;
  min_bitrate_kbps: number;
  adaptive_bitrate: boolean;
  codec: "h264";
  encoder_quality: EncoderQuality;
  encoder: string | null;
  audio: AudioSettings;
}

export interface IceServer {
  urls: string[];
  username?: string | null;
  credential?: string | null;
}

export interface NetworkConfig {
  signaling_url: string;
  extra_ca_pem: string | null;
  ice_servers: IceServer[];
  relay_only: boolean;
  lan_discovery: boolean;
}

export type AudioChoice = "default" | "test_tone" | "off" | { device: string };
export type InputBackendKind = "auto" | "mock" | "x11_xtest" | "uinput" | "win32_send_input";
export type ControllerChoice = "platform" | "mock" | "off";
export type CaptureKind = "x11" | "wayland_portal" | "windows_dxgi" | "test_pattern";

export interface HostConfig {
  display_id: string | null;
  capture: CaptureKind | null;
  audio: AudioChoice;
  input_backend: InputBackendKind;
  controllers: ControllerChoice;
  input_allowed: boolean;
  max_width: number;
  max_height: number;
  max_fps: number;
  max_bitrate_kbps: number;
  encoder: string | null;
  trust_paired_sessions: boolean;
  /** Run the signaling server inside the host (default). */
  builtin_server: boolean;
  /** TCP port of the built-in server. */
  server_port: number;
}

export interface ControllerMapping {
  left_deadzone: number;
  right_deadzone: number;
  trigger_deadzone: number;
  invert_left_y: boolean;
  invert_right_y: boolean;
  remap: Record<string, string>;
}

export interface ClientConfig {
  stream: StreamSettings;
  decoder: string | null;
  jitter_buffer_ms: number;
  fullscreen: boolean;
  volume: number;
  muted: boolean;
  show_stats_overlay: boolean;
  controllers_enabled: boolean;
  controller_mapping: ControllerMapping;
  capture_mouse: boolean;
  renderer: Renderer;
}

/** Streaming-window renderer: GPU sink where fast enough, else software. */
export type Renderer = "auto" | "gpu" | "software";

export interface AppConfig {
  device_name: string;
  network: NetworkConfig;
  host: HostConfig;
  client: ClientConfig;
}

export interface AppInfo {
  version: string;
  device_id: string;
  device_name: string;
  fingerprint: string;
  config_dir: string;
  has_access_token: boolean;
  platform: string;
}

export interface PairedDevice {
  device_id: string;
  name: string;
  public_key: string;
  paired_at_ms: number;
  last_connected_ms: number | null;
  signaling_url: string | null;
}

export interface DiscoveredHost {
  device_id: string;
  name: string;
  signaling_url: string;
  addresses: string[];
  /** Certificate fingerprint of the host's built-in server, if it runs one. */
  fingerprint: string | null;
}

/** The host's built-in signaling server. */
export interface ServerInfo {
  port: number;
  /** SHA-256 of its certificate, lowercase hex. */
  fingerprint: string;
  /** "ip:port" on each local network. */
  lan_addresses: string[];
}

export interface Presence {
  online: string[];
  offline: string[];
}

export interface DisplayInfo {
  id: string;
  name: string;
  x: number;
  y: number;
  width: number;
  height: number;
  primary: boolean;
  backend: CaptureKind;
}

export interface CaptureBackendInfo {
  backend: CaptureKind;
  displays: DisplayInfo[];
  error: string | null;
}

// ---- sessions

export type EndReason =
  | "rejected_by_host"
  | "approval_timeout"
  | "client_disconnected"
  | "host_stopped"
  | "kicked_by_host"
  | "expired"
  | { connection_failed: string }
  | { error: string };

export type SessionState =
  | { state: "requested" }
  | { state: "awaiting_approval" }
  | { state: "negotiating" }
  | { state: "connected" }
  | { state: "reconnecting" }
  | { state: "ended"; detail: EndReason };

export interface SessionInfo {
  session_id: string;
  client_id: string;
  client_name: string;
  state: SessionState;
  input_allowed: boolean;
}

export interface PendingPairingInfo {
  request_id: string;
  client_name: string;
  fingerprint: string;
}

export interface HostSnapshot {
  device_id: string;
  device_name: string;
  signaling_connected: boolean;
  signaling_detail: string;
  pairing_code: string | null;
  pairing_expires_unix_ms: number | null;
  pending_pairings: PendingPairingInfo[];
  sessions: SessionInfo[];
  input_allowed: boolean;
  paired_devices: PairedDevice[];
  server: ServerInfo | null;
}

// ---- statistics

export interface HostStats {
  encoder: string;
  width: number;
  height: number;
  target_bitrate_kbps: number;
  capture_fps: number;
  encode_ms_avg: number;
  frames_dropped_before_encode: number;
  /** Screen grab time; null when the capture source does not report it. */
  capture_ms_avg?: number | null;
}

/** Where capture-to-display latency is spent (ms, 2 s means). */
export interface LatencyBreakdown {
  host_ms: number | null;
  network_ms: number | null;
  decode_ms: number | null;
  convert_ms: number | null;
  present_ms: number | null;
  samples: number;
}

export interface ClientStats {
  rendered_fps: number;
  frames_decoded: number;
  frames_dropped: number;
  packets_lost: number;
  packets_received: number;
  jitter_ms: number;
  decode_ms_avg: number;
  latency_ms: number | null;
}

export interface RtpStats {
  packets: number | null;
  bytes: number | null;
  packets_lost: number | null;
  jitter_ms: number | null;
  nack_count: number | null;
  pli_count: number | null;
  fir_count: number | null;
}

export interface TransportStats {
  outbound_video: RtpStats | null;
  outbound_audio: RtpStats | null;
  inbound_video: RtpStats | null;
  inbound_audio: RtpStats | null;
  remote_rtt_ms: number | null;
  remote_fraction_lost: number | null;
  remote_packets_lost: number | null;
  remote_jitter_ms: number | null;
  local_candidate_type: string | null;
  remote_candidate_type: string | null;
  local_candidate_protocol: string | null;
  relay_protocol: string | null;
}

export interface InputStats {
  applied: number;
  denied: number;
  rate_limited: number;
  invalid: number;
  backend_errors: number;
  /** Rumble commands forwarded from games to the client's controller. */
  rumble: number;
}

export interface HostStatsEvent {
  session_id: string;
  host: HostStats;
  client: ClientStats | null;
  transport: TransportStats;
  encoder_hardware: boolean;
  input_backend: string;
  controller_backend: string;
  input: InputStats;
}

export type HostEvent =
  | { event: "signaling"; connected: boolean; detail: string }
  | { event: "pairing_code"; code: string; expires_in_secs: number }
  | { event: "pairing_code_expired" }
  | { event: "pairing_request"; request_id: string; client_name: string; fingerprint: string }
  | { event: "paired"; client_name: string; device_id: string }
  | { event: "pairing_rejected"; client_name: string; reason: string }
  | { event: "session_request"; session_id: string; client_name: string; fingerprint: string; settings: StreamSettings }
  | { event: "session"; session_id: string; client_name: string; state: SessionState }
  | { event: "input_permission"; allowed: boolean }
  | ({ event: "stats" } & HostStatsEvent)
  | { event: "warning"; message: string }
  | { event: "error"; message: string };

export type ClientState =
  | "connecting_signaling"
  | "waiting_for_approval"
  | "negotiating"
  | "streaming"
  | "reconnecting"
  | "ended"
  | "failed";

export interface ConnectedController {
  slot: number;
  name: string;
  kind: "xbox" | "play_station" | "nintendo" | "generic";
}

export interface ClientStatsEvent {
  stats: ClientStats;
  rtt_ms: number | null;
  latency_uncertainty_ms: number | null;
  host: HostStats | null;
  transport: TransportStats;
  video_bitrate_kbps: number | null;
  decoder: string;
  relayed: boolean | null;
  latency_breakdown: LatencyBreakdown;
}

export type ClientEvent =
  | { event: "state"; state: ClientState; detail: string }
  | { event: "paired"; host_name: string; device_id: string }
  | ({ event: "stats" } & ClientStatsEvent)
  | { event: "controllers"; controllers: ConnectedController[] }
  | { event: "input_permission"; allowed: boolean }
  | { event: "rumble"; index: number; strong: number; weak: number; duration_ms: number }
  | { event: "warning"; message: string }
  | { event: "error"; message: string }
  | { event: "summary"; [key: string]: unknown };

export interface LogLine {
  unix_ms: number;
  level: string;
  target: string;
  message: string;
}

export interface CodecInfo {
  element: string;
  label: string;
  hardware: boolean;
  api: string;
  present: boolean;
  available: boolean;
  error: string | null;
}

export interface SystemReport {
  app_version: string;
  os: string;
  arch: string;
  session_type: string | null;
  gstreamer: string;
  encoders: CodecInfo[];
  decoders: CodecInfo[];
  capture: CaptureBackendInfo[];
  audio_devices: { Ok: { id: string | null; name: string; is_default: boolean }[] } | { Err: string };
  keyboard_mouse_injection: [string, { Ok: null } | { Err: string }][];
  virtual_controllers: { Ok: null } | { Err: string };
  device_id: string;
  config_dir: string;
}

export interface IceCheckReport {
  configured_turn: string[];
  verdict: string;
  result: {
    host_candidates: number;
    srflx_candidates: number;
    relay_candidates: number;
    relay_transports: string[];
    gathering_complete: boolean;
    elapsed_ms: number;
  };
}
