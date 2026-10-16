// Application state fed only by real backend data: command results and
// host/client events. Nothing here fabricates a status.
import { createContext, useCallback, useContext, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { api, errorMessage, onClientEvent, onClientExit, onHostEvent } from "../services/api";
import type {
  AppConfig,
  AppInfo,
  ClientState,
  ClientStatsEvent,
  ConnectedController,
  HostSnapshot,
  HostStatsEvent,
  PairedDevice,
  Presence,
  SessionState,
  StreamSettings,
} from "../types/api";

export interface Notice {
  id: number;
  tone: "info" | "warn" | "error";
  text: string;
}

export interface SessionRequest {
  session_id: string;
  client_name: string;
  fingerprint: string;
  settings: StreamSettings;
}

export interface ClientSession {
  deviceId: string;
  hostName: string;
  running: boolean;
  state: ClientState | "starting";
  detail: string;
  history: { state: ClientState | "starting"; detail: string; at: number }[];
  stats: ClientStatsEvent | null;
  /** Measured capture→display latency of recent stats events (ms), oldest first. */
  latencyHistory: number[];
  controllers: ConnectedController[];
  inputAllowed: boolean | null;
}

interface Ctx {
  info: AppInfo | null;
  config: AppConfig | null;
  saveConfig: (c: AppConfig) => Promise<void>;
  devices: PairedDevice[];
  refreshDevices: () => Promise<void>;
  presence: Presence | null;
  presenceError: string | null;
  refreshPresence: () => Promise<void>;
  host: HostSnapshot | null;
  hostRunning: boolean;
  hostStats: HostStatsEvent | null;
  sessionRequests: SessionRequest[];
  pairingExpiresAt: number | null;
  startHost: () => Promise<void>;
  stopHost: () => Promise<void>;
  refreshHost: () => Promise<void>;
  client: ClientSession | null;
  connect: (device: PairedDevice) => Promise<void>;
  notices: Notice[];
  notify: (tone: Notice["tone"], text: string) => void;
  dismiss: (id: number) => void;
}

/** Samples kept for the latency chart. */
const HISTORY = 90;

const AppContext = createContext<Ctx | null>(null);

export function useApp(): Ctx {
  const c = useContext(AppContext);
  if (!c) throw new Error("useApp outside provider");
  return c;
}

export function AppProvider({ children }: { children: ReactNode }) {
  const [info, setInfo] = useState<AppInfo | null>(null);
  const [config, setConfig] = useState<AppConfig | null>(null);
  const [devices, setDevices] = useState<PairedDevice[]>([]);
  const [presence, setPresence] = useState<Presence | null>(null);
  const [presenceError, setPresenceError] = useState<string | null>(null);
  const [host, setHost] = useState<HostSnapshot | null>(null);
  const [hostStats, setHostStats] = useState<HostStatsEvent | null>(null);
  const [sessionRequests, setSessionRequests] = useState<SessionRequest[]>([]);
  const [pairingExpiresAt, setPairingExpiresAt] = useState<number | null>(null);
  const [client, setClient] = useState<ClientSession | null>(null);
  const [notices, setNotices] = useState<Notice[]>([]);
  const noticeId = useRef(1);

  const notify = useCallback((tone: Notice["tone"], text: string) => {
    const id = noticeId.current++;
    setNotices((n) => [...n.slice(-4), { id, tone, text }]);
    if (tone !== "error") setTimeout(() => setNotices((n) => n.filter((x) => x.id !== id)), 8000);
  }, []);
  const dismiss = useCallback((id: number) => setNotices((n) => n.filter((x) => x.id !== id)), []);

  const refreshDevices = useCallback(async () => {
    setDevices(await api.pairedDevices());
  }, []);

  const refreshPresence = useCallback(async () => {
    try {
      setPresence(await api.presence());
      setPresenceError(null);
    } catch (e) {
      setPresence(null);
      setPresenceError(errorMessage(e));
    }
  }, []);

  const refreshHost = useCallback(async () => {
    setHost(await api.hostSnapshot());
  }, []);

  useEffect(() => {
    api.appInfo().then(setInfo).catch((e) => notify("error", errorMessage(e)));
    api.getConfig().then(setConfig).catch((e) => notify("error", errorMessage(e)));
    refreshDevices().catch(() => {});
    refreshHost().catch(() => {});
    api.clientRunning().then((running) => {
      if (!running) setClient(null);
    });
  }, [notify, refreshDevices, refreshHost]);

  // Host events.
  useEffect(() => {
    const un = onHostEvent((ev) => {
      switch (ev.event) {
        case "stats": {
          const { event: _e, ...rest } = ev;
          setHostStats(rest);
          break;
        }
        case "pairing_code":
          setPairingExpiresAt(Date.now() + ev.expires_in_secs * 1000);
          break;
        case "pairing_code_expired":
          setPairingExpiresAt(null);
          break;
        case "session_request":
          setSessionRequests((r) => [...r.filter((x) => x.session_id !== ev.session_id), ev]);
          break;
        case "session":
          if (ev.state.state !== "awaiting_approval") setSessionRequests((r) => r.filter((x) => x.session_id !== ev.session_id));
          if (ev.state.state === "ended") setHostStats(null);
          break;
        case "paired":
          notify("info", `Paired with ${ev.client_name}.`);
          refreshDevices();
          break;
        case "warning":
          notify("warn", ev.message);
          break;
        case "error":
          notify("error", ev.message);
          break;
      }
      // The snapshot is the source of truth for lists and flags.
      api.hostSnapshot().then(setHost).catch(() => {});
    });
    return () => {
      un.then((f) => f());
    };
  }, [notify, refreshDevices]);

  // Client (streaming process) events.
  useEffect(() => {
    const un1 = onClientEvent((ev) => {
      setClient((c) => {
        if (!c) return c;
        switch (ev.event) {
          case "state":
            return { ...c, state: ev.state, detail: ev.detail, history: [...c.history.slice(-30), { state: ev.state, detail: ev.detail, at: Date.now() }] };
          case "stats": {
            const { event: _e, ...rest } = ev;
            const stats = rest as ClientStatsEvent;
            const lat = stats.stats.latency_ms;
            const latencyHistory = lat != null && Number.isFinite(lat) ? [...c.latencyHistory.slice(-(HISTORY - 1)), lat] : c.latencyHistory;
            return { ...c, stats, latencyHistory };
          }
          case "controllers":
            return { ...c, controllers: ev.controllers };
          case "input_permission":
            return { ...c, inputAllowed: ev.allowed };
          default:
            return c;
        }
      });
      if (ev.event === "warning") notify("warn", ev.message);
      if (ev.event === "error") notify("error", ev.message);
    });
    const un2 = onClientExit((code) => {
      setClient((c) => (c ? { ...c, running: false, state: c.state === "streaming" || c.state === "starting" ? "ended" : c.state } : c));
      if (code !== 0 && code !== null) notify("error", `The streaming client exited with code ${code}.`);
      refreshDevices();
    });
    return () => {
      un1.then((f) => f());
      un2.then((f) => f());
    };
  }, [notify, refreshDevices]);

  const saveConfig = useCallback(
    async (c: AppConfig) => {
      await api.saveConfig(c);
      setConfig(c);
      setInfo((i) => (i ? { ...i, device_name: c.device_name } : i));
    },
    [],
  );

  const startHost = useCallback(async () => {
    setHost(await api.hostStart());
  }, []);

  const stopHost = useCallback(async () => {
    await api.hostStop();
    setHost(null);
    setHostStats(null);
    setSessionRequests([]);
    setPairingExpiresAt(null);
  }, []);

  const connect = useCallback(
    async (device: PairedDevice) => {
      await api.clientConnect(device.device_id);
      const now = Date.now();
      setClient({
        deviceId: device.device_id,
        hostName: device.name,
        running: true,
        state: "starting",
        detail: "starting the streaming window",
        history: [{ state: "starting", detail: "starting the streaming window", at: now }],
        stats: null,
        latencyHistory: [],
        controllers: [],
        inputAllowed: null,
      });
    },
    [],
  );

  const value = useMemo<Ctx>(
    () => ({
      info,
      config,
      saveConfig,
      devices,
      refreshDevices,
      presence,
      presenceError,
      refreshPresence,
      host,
      hostRunning: host !== null,
      hostStats,
      sessionRequests,
      pairingExpiresAt,
      startHost,
      stopHost,
      refreshHost,
      client,
      connect,
      notices,
      notify,
      dismiss,
    }),
    [info, config, saveConfig, devices, refreshDevices, presence, presenceError, refreshPresence, host, hostStats, sessionRequests, pairingExpiresAt, startHost, stopHost, refreshHost, client, connect, notices, notify, dismiss],
  );

  return <AppContext.Provider value={value}>{children}</AppContext.Provider>;
}

export function isLive(s: SessionState): boolean {
  return s.state !== "ended";
}
