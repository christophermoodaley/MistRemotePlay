import { useEffect, useState } from "react";
import type { Page } from "../App";
import { Icon, type IconName } from "../components/Icon";
import { Badge, Button, HostArt, LiveDot, type Tone } from "../components/ui";
import { useApp } from "../hooks/useAppState";
import { errorMessage } from "../services/api";
import { fmtAgo, fmtKbps } from "../services/format";
import { PRESET_LABELS } from "../services/presets";
import type { PairedDevice } from "../types/api";

export function HomePage({ go }: { go: (p: Page) => void }) {
  const { info, config, devices, presence, presenceError, refreshPresence, hostRunning, host, client, connect, notify } = useApp();
  const [checking, setChecking] = useState(false);
  const [connecting, setConnecting] = useState<string | null>(null);

  useEffect(() => {
    if (devices.length > 0) {
      setChecking(true);
      refreshPresence().finally(() => setChecking(false));
    }
  }, [devices.length, refreshPresence]);

  const recent = [...devices].sort((a, b) => (b.last_connected_ms ?? 0) - (a.last_connected_ms ?? 0));
  const hero = recent[0];
  const st = config?.client.stream;

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

  function status(d: PairedDevice): { tone: Tone; label: string } {
    if (presenceError) return { tone: "neutral", label: "status unknown" };
    if (!presence) return { tone: "neutral", label: checking ? "checking…" : "unknown" };
    if (presence.online.includes(d.device_id)) return { tone: "ok", label: "online" };
    return { tone: "neutral", label: "offline" };
  }

  const inSession = (d: PairedDevice) => client?.running && client.deviceId === d.device_id;

  return (
    <div className="fade-in space-y-8">
      <header className="flex flex-wrap items-end justify-between gap-4">
        <div>
          <div className="mb-1.5 text-xs font-semibold uppercase tracking-[0.1em] text-accent">{info ? info.device_name : "Mist Remote Play"}</div>
          <h1 className="text-[28px] font-semibold leading-tight tracking-[-0.02em]">{recent.length ? "Pick up where you left off" : "Play your PC from anywhere"}</h1>
        </div>
        {devices.length > 0 && (
          <Button size="sm" variant="ghost" icon="refresh" busy={checking} onClick={() => { setChecking(true); refreshPresence().finally(() => setChecking(false)); }}>
            Refresh status
          </Button>
        )}
      </header>

      {client?.running && (
        <button type="button" onClick={() => go("streaming")} className="card fade-in flex w-full items-center gap-4 rounded-[var(--radius-card)] border border-ok/30 px-5 py-4 text-left transition-colors hover:border-ok/50">
          <LiveDot tone="ok" live size={10} />
          <div className="min-w-0 flex-1">
            <div className="text-sm font-medium">
              {client.state === "streaming" ? "Streaming from" : "Connecting to"} {client.hostName}
            </div>
            <div className="truncate text-xs text-muted">{client.detail}</div>
          </div>
          {client.stats?.stats.latency_ms != null && (
            <span className="num font-mono text-sm text-ink">
              {client.stats.stats.latency_ms.toFixed(1)} <span className="text-muted">ms</span>
            </span>
          )}
          <Icon name="chevron" className="text-muted" />
        </button>
      )}

      {hero ? (
        <div className="grid gap-5 lg:grid-cols-[1fr_300px]">
          {/* Hero: the most recently used host. */}
          <HostArt id={hero.device_id} className="flex min-h-[260px] flex-col justify-end rounded-[22px] border border-white/[0.07] p-7 shadow-2xl shadow-black/40">
            <div className="absolute right-6 top-6 flex items-center gap-2">
              {(() => {
                const s = status(hero);
                return (
                  <span className="inline-flex items-center gap-2 rounded-full border border-white/10 bg-black/55 px-3 py-1 text-xs text-white/85">
                    <LiveDot tone={s.tone} live={s.tone === "ok"} /> {s.label}
                  </span>
                );
              })()}
            </div>
            <div className="relative">
              <div className="mb-2 text-xs font-medium uppercase tracking-[0.1em] text-white/55">{hero.last_connected_ms ? `Last played ${fmtAgo(hero.last_connected_ms)}` : "Paired, never played"}</div>
              <h2 className="text-[34px] font-semibold leading-none tracking-[-0.025em] text-white">{hero.name}</h2>
              {st && (
                <div className="mt-3 flex flex-wrap gap-2 text-xs text-white/70">
                  <Chip icon="monitor">
                    {st.width}×{st.height}
                  </Chip>
                  <Chip icon="gauge">{st.fps} fps</Chip>
                  <Chip icon="zap">{PRESET_LABELS[st.preset]}</Chip>
                  <Chip icon="signal">up to {fmtKbps(st.max_bitrate_kbps)}</Chip>
                </div>
              )}
              <div className="mt-6 flex flex-wrap items-center gap-3">
                <Button variant="primary" size="lg" icon="play" busy={connecting === hero.device_id} disabled={client?.running} onClick={() => onConnect(hero)}>
                  {inSession(hero) ? "In session" : "Connect"}
                </Button>
                <span className="text-xs text-white/50">The host approves each session.</span>
              </div>
            </div>
          </HostArt>

          <div className="grid gap-4 sm:grid-cols-2 lg:grid-cols-1">
            <ActionCard icon="link" title="Pair a new host" text="Enter the code shown on the PC you want to play." onClick={() => go("connect")} />
            <ActionCard
              icon="host"
              title={hostRunning ? "Hosting is on" : "Host this PC"}
              text={hostRunning ? (host?.signaling_connected ? "Paired devices can request a session." : "Connecting to the signaling server…") : "Let your paired devices stream this computer."}
              onClick={() => go("host")}
              tone={hostRunning ? (host?.signaling_connected ? "ok" : "warn") : undefined}
            />
          </div>
        </div>
      ) : (
        <Onboarding go={go} />
      )}

      {recent.length > 0 && (
        <section>
          <div className="mb-4 flex items-baseline justify-between">
            <h2 className="text-[15px] font-semibold tracking-tight">Your hosts</h2>
            {presenceError && <span className="text-xs text-warn">Online status unavailable: {presenceError}</span>}
          </div>
          <ul className="grid gap-4 sm:grid-cols-2 xl:grid-cols-3">
            {recent.map((d) => {
              const s = status(d);
              return (
                <li key={d.device_id} className="card group overflow-hidden rounded-[var(--radius-card)] border border-line transition-colors hover:border-line-strong">
                  <HostArt id={d.device_id} className="h-28 border-b border-line">
                    <Icon name="monitor" size={30} className="absolute left-1/2 top-1/2 -translate-x-1/2 -translate-y-1/2 text-white/35" />
                    <span className="absolute left-3 top-3 inline-flex items-center gap-1.5 rounded-full bg-black/55 px-2.5 py-0.5 text-[11px] text-white/80">
                      <LiveDot tone={s.tone} size={6} /> {s.label}
                    </span>
                    {inSession(d) && (
                      <span className="absolute right-3 top-3">
                        <Badge tone="ok">in session</Badge>
                      </span>
                    )}
                  </HostArt>
                  <div className="flex items-center gap-3 p-4">
                    <div className="min-w-0 flex-1">
                      <div className="truncate text-sm font-semibold">{d.name}</div>
                      <div className="truncate text-xs text-faint">
                        {fmtAgo(d.last_connected_ms) === "never" ? "never used" : `used ${fmtAgo(d.last_connected_ms)}`}
                        {d.signaling_url ? ` · ${new URL(d.signaling_url).host}` : ""}
                      </div>
                    </div>
                    <Button variant={s.tone === "ok" ? "primary" : "secondary"} size="sm" icon="play" busy={connecting === d.device_id} disabled={client?.running} onClick={() => onConnect(d)} aria-label={`Connect to ${d.name}`}>
                      Connect
                    </Button>
                  </div>
                </li>
              );
            })}
            <li>
              <button
                type="button"
                onClick={() => go("connect")}
                className="flex h-full min-h-[180px] w-full flex-col items-center justify-center gap-2 rounded-[var(--radius-card)] border border-dashed border-line-strong text-muted transition-colors hover:border-accent/60 hover:text-ink"
              >
                <span className="flex h-10 w-10 items-center justify-center rounded-full border border-line bg-raised">
                  <Icon name="plus" />
                </span>
                <span className="text-sm font-medium">Add a host</span>
              </button>
            </li>
          </ul>
        </section>
      )}
    </div>
  );
}

function Chip({ icon, children }: { icon: IconName; children: React.ReactNode }) {
  return (
    <span className="inline-flex items-center gap-1.5 rounded-full border border-white/10 bg-black/45 px-2.5 py-1">
      <Icon name={icon} size={13} className="text-white/60" />
      {children}
    </span>
  );
}

function ActionCard({ icon, title, text, onClick, tone }: { icon: IconName; title: string; text: string; onClick: () => void; tone?: Tone }) {
  return (
    <button type="button" onClick={onClick} className="card group flex items-start gap-4 rounded-[var(--radius-card)] border border-line p-5 text-left transition-colors hover:border-accent/50">
      <span className={`flex h-11 w-11 shrink-0 items-center justify-center rounded-[13px] ${tone === "ok" ? "bg-ok/12 text-ok" : tone === "warn" ? "bg-warn/12 text-warn" : "bg-accent/12 text-accent"}`}>
        <Icon name={icon} size={21} />
      </span>
      <span className="min-w-0 flex-1">
        <span className="flex items-center gap-2 text-[15px] font-semibold">
          {title}
          {tone && <LiveDot tone={tone} live={tone === "ok"} />}
        </span>
        <span className="mt-0.5 block text-sm text-muted">{text}</span>
      </span>
      <Icon name="chevron" className="mt-1 text-faint transition-transform group-hover:translate-x-0.5 group-hover:text-ink" />
    </button>
  );
}

function Onboarding({ go }: { go: (p: Page) => void }) {
  const steps: { icon: IconName; title: string; text: string }[] = [
    { icon: "host", title: "Start hosting on your gaming PC", text: "Open Mist there and choose Host → Start hosting." },
    { icon: "link", title: "Pair this device", text: "Create a pairing code on the host and enter it here. The host approves it." },
    { icon: "play", title: "Connect and play", text: "Your host appears on Home. Keyboard, mouse and controllers just work." },
  ];
  return (
    <div className="card relative overflow-hidden rounded-[22px] border border-line p-8">
      <div className="pointer-events-none absolute -right-24 -top-24 h-72 w-72 rounded-full bg-accent/10 blur-3xl" aria-hidden="true" />
      <h2 className="text-xl font-semibold tracking-tight">Get set up in three steps</h2>
      <p className="mt-1 text-sm text-muted">Mist streams between your own Windows and Linux PCs (including the Raspberry Pi), encrypted end to end.</p>
      <ol className="mt-6 grid gap-4 md:grid-cols-3">
        {steps.map((s, i) => (
          <li key={s.title} className="rounded-[16px] border border-line bg-sunken/60 p-5">
            <div className="mb-4 flex items-center justify-between">
              <span className="flex h-10 w-10 items-center justify-center rounded-[12px] bg-accent/12 text-accent">
                <Icon name={s.icon} size={20} />
              </span>
              <span className="num font-mono text-xs text-faint">0{i + 1}</span>
            </div>
            <div className="text-sm font-semibold">{s.title}</div>
            <div className="mt-1 text-sm text-muted">{s.text}</div>
          </li>
        ))}
      </ol>
      <div className="mt-6 flex flex-wrap gap-3">
        <Button variant="primary" icon="link" onClick={() => go("connect")}>
          Pair with a host
        </Button>
        <Button icon="host" onClick={() => go("host")}>
          Host this PC instead
        </Button>
      </div>
    </div>
  );
}

