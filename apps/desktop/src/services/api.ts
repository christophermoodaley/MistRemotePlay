// Thin typed wrappers around the Tauri commands (apps/desktop/src-tauri).
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
  SystemReport,
  ConnectedController,
} from "../types/api";

export const api = {
  appInfo: () => invoke<AppInfo>("app_info"),
  getConfig: () => invoke<AppConfig>("get_config"),
  saveConfig: (config: AppConfig) => invoke<void>("save_config", { config }),
  setAccessToken: (token: string | null) => invoke<void>("set_access_token", { token }),
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
  hostDecideSession: (sessionId: string, approve: boolean) =>
    invoke<void>("host_decide_session", { sessionId, approve }),
  hostEndSession: (sessionId: string) => invoke<void>("host_end_session", { sessionId }),
  hostSetInputAllowed: (allowed: boolean) => invoke<void>("host_set_input_allowed", { allowed }),
  hostDisconnectAll: () => invoke<void>("host_disconnect_all"),
  hostRevokeDevice: (deviceId: string) => invoke<void>("host_revoke_device", { deviceId }),
  listDisplays: () => invoke<CaptureBackendInfo[]>("list_displays"),
  detectControllers: () => invoke<ConnectedController[]>("detect_controllers"),

  pairedDevices: () => invoke<PairedDevice[]>("paired_devices"),
  forgetDevice: (deviceId: string) => invoke<void>("forget_device", { deviceId }),
  pairWithCode: (code: string, signalingUrl?: string) =>
    invoke<PairedDevice>("pair_with_code", { code, signalingUrl: signalingUrl ?? null }),
  discoverLan: (secs = 3) => invoke<DiscoveredHost[]>("discover_lan", { secs }),
  presence: () => invoke<Presence>("host_presence"),

  clientConnect: (deviceId: string) => invoke<void>("client_connect", { deviceId }),
  clientCommand: (command: string) => invoke<void>("client_command", { command }),
  clientRunning: () => invoke<boolean>("client_running"),
};

export function onHostEvent(cb: (ev: HostEvent) => void): Promise<UnlistenFn> {
  return listen<HostEvent>("host-event", (e) => cb(e.payload));
}

export function onClientEvent(cb: (ev: ClientEvent) => void): Promise<UnlistenFn> {
  return listen<ClientEvent>("client-event", (e) => cb(e.payload));
}

export function onClientExit(cb: (code: number | null) => void): Promise<UnlistenFn> {
  return listen<{ code: number | null }>("client-exit", (e) => cb(e.payload.code));
}

/** Tauri rejects with a plain string; normalise to a message. */
export function errorMessage(e: unknown): string {
  if (typeof e === "string") return e;
  if (e instanceof Error) return e.message;
  return JSON.stringify(e);
}
