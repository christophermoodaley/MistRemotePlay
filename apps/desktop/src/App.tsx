import { useState } from "react";
import { Icon, type IconName } from "./components/Icon";
import { useApp } from "./hooks/useAppState";
import { ConnectPage } from "./pages/ConnectPage";
import { HomePage } from "./pages/HomePage";
import { HostPage } from "./pages/HostPage";
import { SettingsPage } from "./pages/SettingsPage";
import { StreamingPage } from "./pages/StreamingPage";

export type Page = "home" | "host" | "connect" | "streaming" | "settings";

const NAV: { id: Page; label: string; icon: IconName }[] = [
  { id: "home", label: "Home", icon: "home" },
  { id: "host", label: "Host", icon: "host" },
  { id: "connect", label: "Connect", icon: "connect" },
  { id: "streaming", label: "Streaming", icon: "play" },
  { id: "settings", label: "Settings", icon: "settings" },
];

export default function App() {
  const [page, setPage] = useState<Page>("home");
  const { info, host, client, notices, dismiss, sessionRequests } = useApp();
  const pendingHost = (host?.pending_pairings.length ?? 0) + sessionRequests.length;

  return (
    <div className="flex h-full">
      <nav aria-label="Main" className="flex w-60 shrink-0 flex-col border-r border-line bg-surface px-3 py-5">
        <div className="mb-8 flex items-center gap-2.5 px-3">
          <svg width="26" height="26" viewBox="0 0 32 32" aria-hidden="true">
            <rect width="32" height="32" rx="9" fill="#10151c" stroke="#2f3a48" />
            <path d="M7 19c3-4 6-4 9 0s6 4 9 0M7 13c3-4 6-4 9 0s6 4 9 0" stroke="#3ccbb4" strokeWidth="2.2" fill="none" strokeLinecap="round" />
          </svg>
          <span className="text-[15px] font-semibold tracking-tight">Mist</span>
        </div>
        <ul className="flex flex-col gap-1">
          {NAV.map((n) => {
            const active = page === n.id;
            const badge = n.id === "host" && pendingHost > 0 ? pendingHost : n.id === "streaming" && client?.running ? "●" : null;
            return (
              <li key={n.id}>
                <button
                  type="button"
                  onClick={() => setPage(n.id)}
                  aria-current={active ? "page" : undefined}
                  className={`flex w-full items-center gap-3 rounded-[10px] px-3 py-2.5 text-sm transition-colors ${active ? "bg-raised text-ink" : "text-muted hover:bg-raised/60 hover:text-ink"}`}
                >
                  <Icon name={n.icon} className={active ? "text-accent" : ""} />
                  <span className="flex-1 text-left">{n.label}</span>
                  {badge !== null && (
                    <span className={`rounded-full px-1.5 text-[11px] font-semibold ${n.id === "host" ? "bg-warn text-bg" : "text-accent"}`} aria-label={n.id === "host" ? `${badge} pending requests` : "session active"}>
                      {badge}
                    </span>
                  )}
                </button>
              </li>
            );
          })}
        </ul>
        <div className="mt-auto px-3 text-xs text-faint">
          {info ? (
            <>
              <div className="truncate text-muted">{info.device_name}</div>
              <div className="font-mono">{info.fingerprint}</div>
              <div className="mt-1">v{info.version}</div>
            </>
          ) : null}
        </div>
      </nav>

      <main className="relative min-w-0 flex-1 overflow-y-auto">
        <div className="mx-auto max-w-5xl px-8 py-8">
          {page === "home" && <HomePage go={setPage} />}
          {page === "host" && <HostPage />}
          {page === "connect" && <ConnectPage go={setPage} />}
          {page === "streaming" && <StreamingPage go={setPage} />}
          {page === "settings" && <SettingsPage />}
        </div>
        <div aria-live="polite" className="pointer-events-none fixed bottom-4 right-4 z-10 flex w-96 flex-col gap-2">
          {notices.map((n) => (
            <div
              key={n.id}
              role={n.tone === "error" ? "alert" : "status"}
              className={`fade-in pointer-events-auto flex items-start gap-3 rounded-xl border bg-raised p-3 text-sm shadow-lg ${n.tone === "error" ? "border-bad/50" : n.tone === "warn" ? "border-warn/50" : "border-accent/40"}`}
            >
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
