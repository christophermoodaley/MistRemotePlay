import { useState } from "react";
import { Icon, type IconName } from "./components/Icon";
import { HostRequests } from "./components/Requests";
import { LiveDot } from "./components/ui";
import { useApp } from "./hooks/useAppState";
import { ConnectPage } from "./pages/ConnectPage";
import { HomePage } from "./pages/HomePage";
import { HostPage } from "./pages/HostPage";
import { SettingsPage, type SettingsTab } from "./pages/SettingsPage";
import { StreamingPage } from "./pages/StreamingPage";

export type Page = "home" | "host" | "connect" | "streaming" | "settings";

// "connect" (the Add a host flow) is reached from Home; "streaming" only
// appears while there is a session to show.
const NAV: { id: Page; label: string; icon: IconName }[] = [
  { id: "home", label: "Home", icon: "home" },
  { id: "host", label: "Host this PC", icon: "host" },
  { id: "settings", label: "Settings", icon: "settings" },
];

export function MistMark({ size = 30 }: { size?: number }) {
  return (
    <svg width={size} height={size} viewBox="0 0 32 32" aria-hidden="true">
      <defs>
        <linearGradient id="mist-mark" x1="0" y1="0" x2="1" y2="1">
          <stop offset="0" stopColor="#67e3cf" />
          <stop offset="1" stopColor="#3987e5" />
        </linearGradient>
      </defs>
      <rect width="32" height="32" rx="9" fill="#0e1319" stroke="#2b3542" />
      <path d="M7 19c3-4 6-4 9 0s6 4 9 0M7 13c3-4 6-4 9 0s6 4 9 0" stroke="url(#mist-mark)" strokeWidth="2.4" fill="none" strokeLinecap="round" />
    </svg>
  );
}

export default function App() {
  const [page, setPage] = useState<Page>("home");
  const [settingsTab, setSettingsTab] = useState<SettingsTab>("stream");
  const { info, host, hostRunning, client, notices, dismiss, sessionRequests, update } = useApp();
  const go = (p: Page) => {
    if (p === "settings") setSettingsTab("stream");
    setPage(p);
  };
  const pendingHost = (host?.pending_pairings.length ?? 0) + sessionRequests.length;
  const streaming = client?.running ?? false;
  const latency = client?.stats?.stats.latency_ms;
  const nav = client ? [NAV[0], { id: "streaming" as Page, label: "Now playing", icon: "play" as IconName }, ...NAV.slice(1)] : NAV;

  return (
    <div className="flex h-full">
      <nav aria-label="Main" className="flex w-[72px] shrink-0 flex-col border-r border-line bg-surface px-2.5 py-5 lg:w-60 lg:px-3">
        <div className="mb-8 flex items-center justify-center gap-3 lg:justify-start lg:px-2.5">
          <MistMark />
          <div className="hidden leading-tight lg:block">
            <div className="text-[15px] font-semibold tracking-tight">Mist</div>
            <div className="text-[11px] text-faint">Remote Play</div>
          </div>
        </div>
        <ul className="flex flex-col gap-1">
          {nav.map((n) => {
            const active = page === n.id || (n.id === "home" && page === "connect");
            const badge = n.id === "host" && pendingHost > 0 ? pendingHost : null;
            const live = n.id === "streaming" && streaming;
            return (
              <li key={n.id}>
                <button
                  type="button"
                  onClick={() => go(n.id)}
                  aria-current={active ? "page" : undefined}
                  aria-label={n.label}
                  title={n.label}
                  className={`relative flex w-full items-center justify-center gap-3 rounded-[12px] px-3 py-2.5 text-sm transition-colors lg:justify-start ${active ? "bg-raised text-ink shadow-[inset_0_1px_0_rgb(255_255_255/0.04)]" : "text-muted hover:bg-raised/60 hover:text-ink"}`}
                >
                  {active && <span className="absolute -left-2.5 top-1/2 h-5 w-[3px] -translate-y-1/2 rounded-r-full bg-accent lg:-left-3" aria-hidden="true" />}
                  <Icon name={n.icon} className={active ? "text-accent" : ""} />
                  <span className="hidden flex-1 text-left lg:block">{n.label}</span>
                  {badge !== null && (
                    <span className="absolute right-1.5 top-1 rounded-full bg-warn px-1.5 text-[11px] font-semibold text-bg lg:static" aria-label={`${badge} pending requests`}>
                      {badge}
                    </span>
                  )}
                  {live && (
                    <span className="absolute right-2 top-2 lg:static" aria-label="session active">
                      <LiveDot tone="ok" live />
                    </span>
                  )}
                </button>
              </li>
            );
          })}
        </ul>

        <div className="mt-auto space-y-2">
          {update.available && (
            <button
              type="button"
              onClick={() => {
                setSettingsTab("updates");
                setPage("settings");
              }}
              title={`Update to ${update.available.version}`}
              aria-label={`Update available: version ${update.available.version}`}
              className="flex w-full items-center justify-center gap-2 rounded-[13px] border border-accent/35 bg-accent/[0.08] px-3 py-2.5 text-left transition-colors hover:border-accent/60 lg:justify-start"
            >
              <Icon name="download" size={16} className="shrink-0 text-accent" />
              <span className="hidden min-w-0 flex-1 lg:block">
                <span className="block text-xs font-medium text-accent">Update available</span>
                <span className="block truncate text-sm text-ink">Version {update.available.version}</span>
              </span>
            </button>
          )}
          {streaming && client && (
            <button type="button" onClick={() => setPage("streaming")} className="hidden w-full rounded-[13px] border border-ok/25 bg-ok/[0.06] px-3 py-2.5 text-left transition-colors hover:border-ok/45 lg:block">
              <div className="flex items-center gap-2 text-xs font-medium text-ok">
                <LiveDot tone="ok" live /> {client.state === "streaming" ? "Streaming" : "Connecting"}
              </div>
              <div className="mt-1 truncate text-sm text-ink">{client.hostName}</div>
              {latency != null && <div className="num font-mono text-[11px] text-muted">{latency.toFixed(1)} ms latency</div>}
            </button>
          )}
          <button type="button" onClick={() => setPage("host")} className="hidden w-full rounded-[13px] border border-line bg-sunken/60 px-3 py-2.5 text-left transition-colors hover:border-line-strong lg:block">
            <div className="flex items-center gap-2 text-xs">
              <LiveDot tone={hostRunning ? (host?.signaling_connected ? "ok" : "warn") : "neutral"} live={hostRunning && !!host?.signaling_connected} />
              <span className="text-muted">{hostRunning ? (host?.signaling_connected ? "Sharing this PC" : "Starting…") : "Not sharing this PC"}</span>
            </div>
            {info && <div className="mt-1.5 truncate text-sm text-ink">{info.device_name}</div>}
          </button>
          {info && <div className="hidden px-3 text-[11px] text-faint lg:block">v{info.version}</div>}
        </div>
      </nav>

      <main className="relative min-w-0 flex-1 overflow-y-auto">
        <div className="mx-auto max-w-[1080px] px-6 py-8 lg:px-10">
          <HostRequests />
          {page === "home" && <HomePage go={go} />}
          {page === "host" && <HostPage go={go} openSettings={(t) => { setSettingsTab(t); setPage("settings"); }} />}
          {page === "connect" && <ConnectPage go={go} />}
          {page === "streaming" && <StreamingPage go={go} />}
          {page === "settings" && <SettingsPage initialTab={settingsTab} />}
        </div>
        <div aria-live="polite" className="pointer-events-none fixed bottom-5 right-5 z-20 flex w-[380px] max-w-[calc(100vw-110px)] flex-col gap-2">
          {notices.map((n) => (
            <div
              key={n.id}
              role={n.tone === "error" ? "alert" : "status"}
              className={`fade-in pointer-events-auto flex items-start gap-3 rounded-[14px] border bg-raised/95 p-3.5 text-sm shadow-2xl shadow-black/50 ${n.tone === "error" ? "border-bad/45" : n.tone === "warn" ? "border-warn/45" : "border-accent/35"}`}
            >
              <Icon name={n.tone === "info" ? "check" : "alert"} size={17} className={`mt-px shrink-0 ${n.tone === "error" ? "text-bad" : n.tone === "warn" ? "text-warn" : "text-accent"}`} />
              <span className="flex-1 text-ink">{n.text}</span>
              <button type="button" onClick={() => dismiss(n.id)} className="text-muted hover:text-ink" aria-label="Dismiss notification">
                <Icon name="x" size={15} />
              </button>
            </div>
          ))}
        </div>
      </main>
    </div>
  );
}
