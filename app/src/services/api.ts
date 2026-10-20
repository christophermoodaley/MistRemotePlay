// Thin typed wrappers around the Tauri commands (app/src-tauri).
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type {
  AppConfig,
  AppInfo,
  CaptureBackendInfo,
  ClientEvent,
  DiscoveredHost,
  HostEvent,
  HostSnapshot,
  IceCheckReport,
  LogLine,
  PairedDevice,
  Presence,
  SessionRole,
  SystemReport,
  ConnectedController,
  UpdateInfo,
  UpdateProgress,
} from "../types/api";

export const api = {
  appInfo: () => invoke<AppInfo>("app_info"),
  getConfig: () => invoke<AppConfig>("get_config"),
  saveConfig: (config: AppConfig) => invoke<void>("save_config", { config }),
  setAccessToken: (token: string | null) => invoke<void>("set_access_token", { token }),
  setGithubToken: (token: string | null) => invoke<void>("set_github_token", { token }),
  updateCheck: () => invoke<UpdateInfo | null>("update_check"),
  updateInstall: () => invoke<void>("update_install"),
  openExternal: (url: string) => invoke<void>("open_external", { url }),
  systemReport: () => invoke<SystemReport>("system_report"),
  logs: () => invoke<LogLine[]>("get_logs"),
  iceCheck: () => invoke<IceCheckReport>("ice_check"),

  hostStart: () => invoke<HostSnapshot>("host_start"),
  hostStop: () => invoke<void>("host_stop"),
  hostSnapshot: () => invoke<HostSnapshot | null>("host_snapshot"),
  hostGeneratePairingCode: () => invoke<void>("host_generate_pairing_code"),
  hostCancelPairing: () => invoke<void>("host_cancel_pairing"),
  hostDecidePairing: (requestId: string, approve: boolean) =>
    invoke<void>("host_decide_pairing", { requestId, approve }),
  hostDecideSession: (sessionId: string, approve: boolean, role: SessionRole = "play") =>
    invoke<void>("host_decide_session", { sessionId, approve, role }),
  hostEndSession: (sessionId: string) => invoke<void>("host_end_session", { sessionId }),
  hostSetRole: (sessionId: string, role: SessionRole) => invoke<void>("host_set_role", { sessionId, role }),
  hostSetKeyboardMouse: (sessionId: string, allowed: boolean) =>
    invoke<void>("host_set_keyboard_mouse", { sessionId, allowed }),
  hostSetInputAllowed: (allowed: boolean) => invoke<void>("host_set_input_allowed", { allowed }),
  hostDisconnectAll: () => invoke<void>("host_disconnect_all"),
  hostRevokeDevice: (deviceId: string) => invoke<void>("host_revoke_device", { deviceId }),
  listDisplays: () => invoke<CaptureBackendInfo[]>("list_displays"),
  detectControllers: () => invoke<ConnectedController[]>("detect_controllers"),

  pairedDevices: () => invoke<PairedDevice[]>("paired_devices"),
  forgetDevice: (deviceId: string) => invoke<void>("forget_device", { deviceId }),
  pairWithCode: (code: string, signalingUrl?: string) =>
    invoke<PairedDevice>("pair_with_code", { code, signalingUrl: signalingUrl ?? null }),
  trustServer: (url: string, fingerprint: string) => invoke<void>("trust_server", { url, fingerprint }),
  discoverLan: (secs = 3) => invoke<DiscoveredHost[]>("discover_lan", { secs }),
  presence: () => invoke<Presence>("host_presence"),

  clientConnect: (deviceId: string) => invoke<void>("client_connect", { deviceId }),
  clientCommand: (command: string) => invoke<void>("client_command", { command }),
  clientRunning: () => invoke<boolean>("client_running"),

  overlayViewers: () => invoke<OverlayViewer[]>("overlay_viewers"),
};

/** Someone shown in the host's connection overlay. */
export interface OverlayViewer {
  name: string;
  role: SessionRole;
}

export function onOverlayViewers(cb: (viewers: OverlayViewer[]) => void): Promise<UnlistenFn> {
  return listen<OverlayViewer[]>("overlay-viewers", (e) => cb(e.payload));
}

export function onHostEvent(cb: (ev: HostEvent) => void): Promise<UnlistenFn> {
  return listen<HostEvent>("host-event", (e) => cb(e.payload));
}

export function onClientEvent(cb: (ev: ClientEvent) => void): Promise<UnlistenFn> {
  return listen<ClientEvent>("client-event", (e) => cb(e.payload));
}

export function onUpdateProgress(cb: (p: UpdateProgress) => void): Promise<UnlistenFn> {
  return listen<UpdateProgress>("update-progress", (e) => cb(e.payload));
}

export function onClientExit(cb: (code: number | null) => void): Promise<UnlistenFn> {
  return listen<{ code: number | null }>("client-exit", (e) => cb(e.payload.code));
}

/** A pairing attempt stopped at a server certificate nobody vouches for yet. */
export interface UntrustedServer {
  fingerprint: string;
  url: string;
}

/** Recognises the backend's "UNTRUSTED_CERTIFICATE <fp> <url>" error. */
export function untrustedServer(e: unknown): UntrustedServer | null {
  const m = typeof e === "string" ? /^UNTRUSTED_CERTIFICATE ([0-9a-f]{64}) (\S+)$/.exec(e) : null;
  return m ? { fingerprint: m[1]!, url: m[2]! } : null;
}

/** Tauri rejects with a plain string; normalise to a message. */
export function errorMessage(e: unknown): string {
  if (typeof e === "string") return e;
  if (e instanceof Error) return e.message;
  return JSON.stringify(e);
}
