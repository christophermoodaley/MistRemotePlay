import { useEffect, useState } from "react";
import type { Page } from "../App";
import { Icon, type IconName } from "../components/Icon";
import { Badge, Button, HostArt, LiveDot, type Tone } from "../components/ui";
import { useApp } from "../hooks/useAppState";
import { api, errorMessage } from "../services/api";
import { fmtAgo } from "../services/format";
import type { PairedDevice } from "../types/api";

/** How often Home re-checks which hosts are online. */
const PRESENCE_EVERY_MS = 30_000;

export function HomePage({ go }: { go: (p: Page) => void }) {
  const { info, devices, refreshDevices, presence, presenceError, refreshPresence, hostRunning, host, client, connect, notify } = useApp();
  const [checking, setChecking] = useState(false);
  const [connecting, setConnecting] = useState<string | null>(null);

  // Online status refreshes by itself; there is no button to remember.
  useEffect(() => {
    if (devices.length === 0) return;
    const check = () => {
      setChecking(true);
      refreshPresence().finally(() => setChecking(false));
    };
    check();
    const t = setInterval(check, PRESENCE_EVERY_MS);
    return () => clearInterval(t);
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

  async function forget(d: PairedDevice) {
    try {
      await api.forgetDevice(d.device_id);
      await refreshDevices();
      notify("info", `Removed ${d.name}. Pair again with a new code to add it back.`);
    } catch (e) {
      notify("error", errorMessage(e));
    }
  }

  function status(d: PairedDevice): { tone: Tone; label: string } {
    if (presenceError || !presence) return { tone: "neutral", label: checking ? "Checking…" : "Status unknown" };
    if (presence.online.includes(d.device_id)) return { tone: "ok", label: "Ready to play" };
    return { tone: "neutral", label: "Offline" };
  }

  if (recent.length === 0) return <Welcome go={go} />;

  return (
    <div className="fade-in space-y-8">
      <header className="flex flex-wrap items-end justify-between gap-4">
        <div>
          <div className="mb-1.5 text-xs font-semibold uppercase tracking-[0.1em] text-accent">{info?.device_name ?? "Mist Remote Play"}</div>
          <h1 className="text-[28px] font-semibold leading-tight tracking-[-0.02em]">What do you want to play on?</h1>
        </div>
        <Button icon="plus" onClick={() => go("connect")}>
          Add a host
        </Button>
      </header>

      {client?.running && (
        <button type="button" onClick={() => go("streaming")} className="card fade-in flex w-full items-center gap-4 rounded-[var(--radius-card)] border border-ok/30 px-5 py-4 text-left transition-colors hover:border-ok/50">
          <LiveDot tone="ok" live size={10} />
          <div className="min-w-0 flex-1 text-sm font-medium">
            {client.state === "streaming" ? "Playing on" : "Connecting to"} {client.hostName}
          </div>
          <span className="text-sm text-muted">Open</span>
          <Icon name="chevron" className="text-muted" />
        </button>
      )}

      <ul className="grid gap-4 sm:grid-cols-2 xl:grid-cols-3">
        {recent.map((d) => {
          const s = status(d);
          const here = client?.running && client.deviceId === d.device_id;
          return (
            <li key={d.device_id} className="card group overflow-hidden rounded-[var(--radius-card)] border border-line transition-colors hover:border-line-strong">
              <HostArt id={d.device_id} className="h-32 border-b border-line">
                <Icon name="monitor" size={32} className="absolute left-1/2 top-1/2 -translate-x-1/2 -translate-y-1/2 text-white/35" />
                <span className="absolute left-3 top-3 inline-flex items-center gap-1.5 rounded-full bg-black/55 px-2.5 py-0.5 text-[11px] text-white/85">
                  <LiveDot tone={s.tone} size={6} live={s.tone === "ok"} /> {s.label}
                </span>
                {here && (
                  <span className="absolute right-3 top-3">
                    <Badge tone="ok">Playing</Badge>
                  </span>
                )}
              </HostArt>
              <div className="p-4">
                <div className="flex items-baseline justify-between gap-2">
                  <div className="truncate text-[15px] font-semibold">{d.name}</div>
                  <ForgetButton name={d.name} onForget={() => forget(d)} />
                </div>
                <div className="truncate text-xs text-faint">{d.last_connected_ms ? `Last played ${fmtAgo(d.last_connected_ms)}` : "Not played yet"}</div>
                <Button
                  variant="primary"
                  icon="play"
                  className="mt-4 w-full"
                  busy={connecting === d.device_id}
                  disabled={client?.running}
                  onClick={() => onConnect(d)}
                  aria-label={`Play on ${d.name}`}
                >
                  {here ? "Playing now" : "Play"}
                </Button>
              </div>
            </li>
          );
        })}
      </ul>
      {presenceError && <p className="-mt-4 text-xs text-faint">Couldn't check which hosts are online. You can still press Play.</p>}

      <ThisPcStrip hostRunning={hostRunning} online={!!host?.signaling_connected} people={host?.sessions.filter((s) => s.state.state !== "ended").length ?? 0} onOpen={() => go("host")} />
    </div>
  );
}

/** Two clicks to forget a host, so a stray click can't remove it. */
function ForgetButton({ name, onForget }: { name: string; onForget: () => void }) {
  const [armed, setArmed] = useState(false);
  useEffect(() => {
    if (!armed) return;
    const t = setTimeout(() => setArmed(false), 3000);
    return () => clearTimeout(t);
  }, [armed]);
  return (
    <button
      type="button"
      onClick={() => (armed ? onForget() : setArmed(true))}
      aria-label={armed ? `Confirm removing ${name}` : `Remove ${name}`}
      className={`shrink-0 text-xs transition-colors ${armed ? "font-medium text-bad" : "text-faint opacity-0 hover:text-ink focus-visible:opacity-100 group-hover:opacity-100"}`}
    >
      {armed ? "Click again to remove" : "Remove"}
    </button>
  );
}

function ThisPcStrip({ hostRunning, online, people, onOpen }: { hostRunning: boolean; online: boolean; people: number; onOpen: () => void }) {
  const text = !hostRunning
    ? "Others can't play this PC right now."
    : !online
      ? "Starting…"
      : people > 0
        ? `${people} ${people === 1 ? "person is" : "people are"} connected.`
        : "Your paired devices can play it.";
  return (
    <button type="button" onClick={onOpen} className="card group flex w-full items-center gap-4 rounded-[var(--radius-card)] border border-line px-5 py-4 text-left transition-colors hover:border-line-strong">
      <span className={`flex h-10 w-10 shrink-0 items-center justify-center rounded-[12px] ${hostRunning ? "bg-ok/12 text-ok" : "bg-raised text-muted"}`}>
        <Icon name="host" size={19} />
      </span>
      <span className="min-w-0 flex-1">
        <span className="flex items-center gap-2 text-sm font-semibold">
          {hostRunning ? "This PC is shared" : "This PC isn't shared"}
          {hostRunning && <LiveDot tone={online ? "ok" : "warn"} live={online} />}
        </span>
        <span className="block text-sm text-muted">{text}</span>
      </span>
      <span className="text-sm text-muted group-hover:text-ink">{hostRunning ? "Manage" : "Share this PC"}</span>
      <Icon name="chevron" className="text-faint transition-transform group-hover:translate-x-0.5 group-hover:text-ink" />
    </button>
  );
}

/** First run: one question, two answers. */
function Welcome({ go }: { go: (p: Page) => void }) {
  const { hostRunning, host } = useApp();
  const paired = host?.paired_devices.length ?? 0;
  return (
    <div className="fade-in mx-auto max-w-3xl py-6">
      <div className="text-center">
        <h1 className="text-[30px] font-semibold leading-tight tracking-[-0.02em]">Welcome to Mist</h1>
        <p className="mt-2 text-muted">Play games on one PC while they run on another. What would you like to do on this PC?</p>
      </div>
      <div className="mt-8 grid gap-4 md:grid-cols-2">
        <Choice
          icon="play"
          title="Play on this PC"
          text="Stream games from another PC that runs Mist. You'll need a code from that PC."
          action="Add a host"
          onClick={() => go("connect")}
          primary
        />
        <Choice
          icon="host"
          title="Share this PC"
          text={hostRunning ? `This PC is shared${paired ? ` with ${paired} ${paired === 1 ? "device" : "devices"}` : ""}. Add more people or see who's connected.` : "Let friends or your other PCs play the games on this one. You approve everyone."}
          action={hostRunning ? "Manage sharing" : "Set up sharing"}
          onClick={() => go("host")}
          tone={hostRunning ? "ok" : undefined}
        />
      </div>
      <p className="mt-6 text-center text-xs text-faint">You can do both. Connections are encrypted and nobody joins without your approval.</p>
    </div>
  );
}

function Choice({ icon, title, text, action, onClick, primary, tone }: { icon: IconName; title: string; text: string; action: string; onClick: () => void; primary?: boolean; tone?: Tone }) {
  return (
    <button
      type="button"
      onClick={onClick}
      className={`card group flex flex-col items-start rounded-[22px] border p-7 text-left transition-colors ${primary ? "border-accent/40 hover:border-accent/70" : "border-line hover:border-line-strong"}`}
    >
      <span className={`mb-5 flex h-12 w-12 items-center justify-center rounded-[14px] ${tone === "ok" ? "bg-ok/12 text-ok" : "bg-accent/12 text-accent"}`}>
        <Icon name={icon} size={22} />
      </span>
      <span className="text-lg font-semibold tracking-tight">{title}</span>
      <span className="mt-1.5 flex-1 text-sm text-muted">{text}</span>
      <span className={`mt-6 inline-flex items-center gap-1.5 text-sm font-medium ${primary ? "text-accent" : "text-ink"}`}>
        {action}
        <Icon name="chevron" size={16} className="transition-transform group-hover:translate-x-0.5" />
      </span>
    </button>
  );
}
