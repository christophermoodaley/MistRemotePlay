import { useEffect, useState } from "react";
import type { Page } from "../App";
import { Icon } from "../components/Icon";
import { Badge, Banner, Button, Card, EmptyState, PageHeader, StatusDot } from "../components/ui";
import { useApp } from "../hooks/useAppState";
import { errorMessage } from "../services/api";
import { fmtAgo } from "../services/format";
import type { PairedDevice } from "../types/api";

export function HomePage({ go }: { go: (p: Page) => void }) {
  const { info, devices, presence, presenceError, refreshPresence, hostRunning, host, client, connect, notify } = useApp();
  const [checking, setChecking] = useState(false);
  const [connecting, setConnecting] = useState<string | null>(null);

  useEffect(() => {
    if (devices.length > 0) {
      setChecking(true);
      refreshPresence().finally(() => setChecking(false));
    }
  }, [devices.length, refreshPresence]);

  const recent = [...devices].sort((a, b) => (b.last_connected_ms ?? 0) - (a.last_connected_ms ?? 0));

  async function onConnect(d: PairedDevice) {
    setConnecting(d.device_id);
    try {
      await connect(d);
      go("streaming");
    } catch (e) {
      notify("error", errorMessage(e));
    } finally {
      setConnecting(null);
    }
  }

  function status(d: PairedDevice) {
    if (presenceError) return <StatusDot tone="neutral" label="status unknown" />;
    if (!presence) return <StatusDot tone="neutral" label={checking ? "checking…" : "unknown"} />;
    if (presence.online.includes(d.device_id)) return <StatusDot tone="ok" label="online" />;
    return <StatusDot tone="neutral" label="offline" />;
  }

  return (
    <div className="fade-in">
      <PageHeader title="Home" subtitle={info ? `This device: ${info.device_name}` : "Stream games between your Windows and Linux PCs."} />

      {client?.running && (
        <div className="mb-6">
          <Banner tone="info" title={`Session with ${client.hostName}: ${client.state.replace(/_/g, " ")}`} action={<Button size="sm" onClick={() => go("streaming")}>Open</Button>}>
            {client.detail}
          </Banner>
        </div>
      )}

      <div className="mb-8 grid grid-cols-2 gap-4">
        <button type="button" onClick={() => go("connect")} className="group flex items-center gap-4 rounded-[var(--radius-card)] border border-line bg-surface p-5 text-left transition-colors hover:border-accent/60">
          <span className="flex h-11 w-11 items-center justify-center rounded-xl bg-accent/12 text-accent">
            <Icon name="connect" size={22} />
          </span>
          <span>
            <span className="block text-[15px] font-semibold">Connect to a host</span>
            <span className="block text-sm text-muted">Pair with a code or pick a saved PC</span>
          </span>
        </button>
        <button type="button" onClick={() => go("host")} className="group flex items-center gap-4 rounded-[var(--radius-card)] border border-line bg-surface p-5 text-left transition-colors hover:border-accent/60">
          <span className="flex h-11 w-11 items-center justify-center rounded-xl bg-raised text-ink">
            <Icon name="host" size={22} />
          </span>
          <span>
            <span className="block text-[15px] font-semibold">{hostRunning ? "Hosting is on" : "Start hosting"}</span>
            <span className="block text-sm text-muted">
              {hostRunning ? (host?.signaling_connected ? "Reachable by paired devices" : "Connecting to the signaling server…") : "Let paired devices stream this PC"}
            </span>
          </span>
        </button>
      </div>

      <Card
        title="Recent connections"
        actions={
          devices.length > 0 && (
            <Button size="sm" variant="ghost" icon="refresh" busy={checking} onClick={() => { setChecking(true); refreshPresence().finally(() => setChecking(false)); }}>
              Refresh status
            </Button>
          )
        }
      >
        {presenceError && devices.length > 0 && <p className="mb-3 text-xs text-warn">Online status unavailable: {presenceError}</p>}
        {recent.length === 0 ? (
          <EmptyState icon="link" title="No saved hosts yet">
            Start hosting on the PC you want to play on, then pair this device with the code it shows.
          </EmptyState>
        ) : (
          <ul className="divide-y divide-line">
            {recent.map((d) => (
              <li key={d.device_id} className="flex items-center gap-4 py-3">
                <span className="flex h-9 w-9 items-center justify-center rounded-lg bg-raised text-muted">
                  <Icon name="monitor" />
                </span>
                <div className="min-w-0 flex-1">
                  <div className="truncate text-sm font-medium">{d.name}</div>
                  <div className="text-xs text-faint">
                    last used {fmtAgo(d.last_connected_ms)} · <span className="font-mono">{d.device_id.slice(0, 8)}</span>
                  </div>
                </div>
                {status(d)}
                {d.signaling_url && <Badge>{new URL(d.signaling_url).host}</Badge>}
                <Button variant="primary" size="sm" icon="play" busy={connecting === d.device_id} disabled={client?.running} onClick={() => onConnect(d)}>
                  Connect
                </Button>
              </li>
            ))}
          </ul>
        )}
      </Card>
    </div>
  );
}
