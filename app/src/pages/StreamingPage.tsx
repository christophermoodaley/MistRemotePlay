import { useState } from "react";
import type { Page } from "../App";
import { LatencyBar, Sparkline } from "../components/charts";
import { Icon, type IconName } from "../components/Icon";
import { Badge, Banner, Button, Card, Disclosure, EmptyState, HostArt, Kbd, LiveDot, PageHeader, SectionLabel, Slider, Stat, type Tone } from "../components/ui";
import { useApp } from "../hooks/useAppState";
import { api, errorMessage } from "../services/api";
import { fmtKbps } from "../services/format";
import type { ClientState, ConnectedController } from "../types/api";

const STEPS: { state: ClientState; label: string; icon: IconName }[] = [
  { state: "connecting_signaling", label: "Reaching the host", icon: "globe" },
  { state: "waiting_for_approval", label: "Host approves", icon: "shield" },
  { state: "negotiating", label: "Connecting securely", icon: "lock" },
  { state: "streaming", label: "Playing", icon: "play" },
];

const STATE_TEXT: Partial<Record<ClientState | "starting", string>> = {
  starting: "Starting",
  connecting_signaling: "Reaching the host",
  waiting_for_approval: "Waiting for approval",
  negotiating: "Connecting",
  streaming: "Playing",
  reconnecting: "Reconnecting",
  failed: "Couldn't connect",
};

function hint(detail: string): string | null {
  const d = detail.toLowerCase();
  if (d.includes("not paired")) return "Remove this host on Home and add it again with a new code.";
  if (d.includes("signaling") && (d.includes("reach") || d.includes("timed out")))
    return "Check that the host is on and sharing (Host this PC), and that its address is right.";
  if (d.includes("ice") || d.includes("peer connection")) return "The PCs couldn't reach each other directly. A relay server can help: Settings → Advanced.";
  if (d.includes("approve")) return "Ask the person at the host to approve the request.";
  if (d.includes("rejected")) return "The host declined. Ask them to approve the next request.";
  return null;
}

function stateTone(state: string): Tone {
  if (state === "streaming") return "ok";
  if (state === "failed") return "bad";
  if (state === "reconnecting") return "warn";
  return "neutral";
}

/** A quality verdict for the measured latency (thresholds for game streaming). */
function latencyVerdict(ms: number): { label: string; tone: Tone } {
  if (ms < 20) return { label: "Excellent", tone: "ok" };
  if (ms < 40) return { label: "Good", tone: "ok" };
  if (ms < 70) return { label: "Noticeable", tone: "warn" };
  return { label: "High", tone: "bad" };
}

function Stepper({ state }: { state: ClientState | "starting" }) {
  const stepIndex = STEPS.findIndex((x) => x.state === state);
  const streaming = state === "streaming";
  const failed = state === "failed";
  return (
    <ol className="flex items-center" aria-label="Connection progress">
      {STEPS.map((step, i) => {
        const done = stepIndex > i || streaming;
        const current = stepIndex === i && !streaming;
        return (
          <li key={step.state} className="flex flex-1 items-center last:flex-none" aria-current={current ? "step" : undefined}>
            <div className="flex items-center gap-2.5">
              <span
                className={`flex h-8 w-8 shrink-0 items-center justify-center rounded-full border transition-colors ${
                  done ? "border-accent/60 bg-accent/15 text-accent" : current ? (failed ? "border-bad/60 bg-bad/10 text-bad" : "border-accent text-accent") : "border-line-strong text-faint"
                }`}
              >
                {done ? <Icon name="check" size={15} /> : current && !failed ? <span className="h-3.5 w-3.5 animate-spin rounded-full border-2 border-current border-t-transparent" /> : <Icon name={step.icon} size={14} />}
              </span>
              <span className={`hidden whitespace-nowrap text-xs font-medium md:inline ${done || current ? "text-ink" : "text-faint"}`}>{step.label}</span>
            </div>
            {i < STEPS.length - 1 && <span className={`mx-3 h-px flex-1 ${done && (stepIndex > i + 1 || streaming) ? "bg-accent/50" : done ? "bg-gradient-to-r from-accent/50 to-line" : "bg-line"}`} aria-hidden="true" />}
          </li>
        );
      })}
    </ol>
  );
}

function ControllerCard({ c }: { c: ConnectedController }) {
  const kind = { xbox: "Xbox layout", play_station: "PlayStation layout", nintendo: "Nintendo layout", generic: "Generic layout" }[c.kind];
  return (
    <li className="flex items-center gap-3 rounded-[14px] border border-line bg-sunken/60 px-3.5 py-3">
      <span className="flex h-10 w-10 shrink-0 items-center justify-center rounded-[12px] bg-accent/12 text-accent">
        <Icon name="gamepad" size={20} />
      </span>
      <div className="min-w-0 flex-1">
        <div className="truncate text-sm font-medium">{c.name}</div>
        <div className="text-xs text-faint">{kind}</div>
      </div>
      <span className="flex h-7 min-w-7 items-center justify-center rounded-full border border-accent/40 px-2 font-mono text-xs font-semibold text-accent" aria-label={`Player ${c.slot + 1}`}>
        P{c.slot + 1}
      </span>
    </li>
  );
}

export function StreamingPage({ go }: { go: (p: Page) => void }) {
  const { client, config, notify } = useApp();
  const [volume, setVolume] = useState(config?.client.volume ?? 1);

  async function cmd(c: string) {
    try {
      await api.clientCommand(c);
    } catch (e) {
      notify("error", errorMessage(e));
    }
  }

  if (!client) {
    return (
      <div className="fade-in">
        <PageHeader title="Now playing" />
        <EmptyState icon="play" title="You're not playing anything">
          Press Play on a host on the Home screen.
          <div className="mt-4">
            <Button variant="primary" icon="home" onClick={() => go("home")}>
              Go to Home
            </Button>
          </div>
        </EmptyState>
      </div>
    );
  }

  const s = client.stats;
  const failed = client.state === "failed";
  const h = hint(client.detail);
  const latency = s?.stats.latency_ms ?? null;
  const verdict = latency != null ? latencyVerdict(latency) : null;
  const tone = stateTone(client.state);

  return (
    <div className="fade-in space-y-6">
      <HostArt id={client.deviceId} className="rounded-[22px] border border-white/[0.07] p-6 shadow-2xl shadow-black/40">
        <div className="relative flex flex-wrap items-center gap-4">
          <div className="min-w-0 flex-1">
            <div className="mb-2 flex items-center gap-2">
              <span className="inline-flex items-center gap-2 rounded-full border border-white/10 bg-black/55 px-3 py-1 text-xs font-medium capitalize text-white/85">
                <LiveDot tone={tone} live={client.running && tone === "ok"} /> {client.running ? STATE_TEXT[client.state] ?? client.state.replace(/_/g, " ") : "finished"}
              </span>
            </div>
            <h1 className="truncate text-[30px] font-semibold leading-tight tracking-[-0.02em] text-white">{client.hostName}</h1>
            <p className="mt-1 truncate text-sm text-white/60">{client.running ? (client.state === "streaming" ? "The game plays in its own window." : client.detail) : "Session finished."}</p>
          </div>
          {client.running && (
            <div className="flex gap-2">
              <Button icon="maximize" onClick={() => cmd("fullscreen")} className="border-white/15 bg-black/45 hover:bg-black/45">
                Fullscreen
              </Button>
              <Button variant="danger" icon="power" onClick={() => cmd("disconnect")}>
                Leave
              </Button>
            </div>
          )}
        </div>
        <div className="relative mt-6 rounded-[14px] border border-white/[0.08] bg-black/45 px-4 py-3.5">
          <Stepper state={client.state} />
        </div>
      </HostArt>

      {(failed || (!client.running && client.state !== "streaming")) && h && (
        <Banner tone={failed ? "error" : "warn"} title="What to try">
          {h}
        </Banner>
      )}
      {client.inputAllowed === false && (
        <Banner tone="warn" title="Remote input is disabled by the host">
          You can watch, but keyboard, mouse and controllers are ignored.
        </Banner>
      )}

      {s ? (
        <Card>
          <div className="flex flex-wrap items-center gap-x-10 gap-y-4">
            <div>
              <div className="text-xs font-medium text-muted">Delay</div>
              <div className="mt-1 flex items-end gap-2">
                <span className="num text-[44px] font-semibold leading-none tracking-[-0.03em]">{latency != null ? latency.toFixed(0) : "—"}</span>
                <span className="mb-1 text-muted">ms</span>
                {verdict && (
                  <span className="mb-1 ml-1">
                    <Badge tone={verdict.tone}>{verdict.label}</Badge>
                  </span>
                )}
              </div>
            </div>
            <div className="grid flex-1 grid-cols-2 gap-3 md:grid-cols-3">
              <Stat label="Smoothness" value={s.stats.rendered_fps.toFixed(0)} unit="fps" />
              <Stat label="Picture" value={s.host ? `${s.host.height}p` : "—"} hint={s.host ? `${s.host.width}×${s.host.height}` : undefined} />
              <Stat label="Connection" value={s.relayed ? "Relayed" : "Direct"} hint={s.relayed ? "Through a TURN relay, which adds delay" : "Straight between the PCs"} />
            </div>
          </div>
        </Card>
      ) : (
        client.running && (
          <Card>
            <div className="flex items-center gap-3 text-sm text-muted">
              <span className="h-4 w-4 animate-spin rounded-full border-2 border-accent border-t-transparent" aria-hidden="true" />
              Getting the stream going…
            </div>
          </Card>
        )
      )}

      <div className="grid gap-6 lg:grid-cols-2">
        <Card title="Sound" icon="audio">
          <div className="flex items-center gap-3">
            <Button size="sm" variant="ghost" icon="mute" disabled={!client.running} onClick={() => cmd("mute")} aria-label="Mute or unmute" title="Mute or unmute" />
            <div className="flex-1">
              <Slider label="Volume" value={volume} min={0} max={2} step={0.05} format={(v) => `${Math.round(v * 100)}%`} onChange={(v) => { setVolume(v); cmd(`volume ${v.toFixed(2)}`); }} />
            </div>
          </div>
        </Card>

        <Card title="Controllers" icon="gamepad" actions={client.controllers.length > 0 && <Badge tone="accent">{client.controllers.length} connected</Badge>}>
          {client.controllers.length === 0 ? (
            <p className="text-sm text-muted">{config?.client.controllers_enabled === false ? "Controllers are turned off in Settings." : "Plug one in any time and it just works."}</p>
          ) : (
            <ul className="space-y-2">
              {client.controllers.map((c) => (
                <ControllerCard key={c.slot} c={c} />
              ))}
            </ul>
          )}
        </Card>
      </div>

      <p className="text-center text-xs leading-relaxed text-faint">
        In the game window, hold <Kbd>Ctrl</Kbd> <Kbd>Alt</Kbd> <Kbd>Shift</Kbd> and press <Kbd>Q</Kbd> leave · <Kbd>F</Kbd> fullscreen · <Kbd>M</Kbd> grab mouse · <Kbd>S</Kbd> stats · <Kbd>A</Kbd> mute
      </p>

      {s && (
        <Disclosure title="Details" subtitle="Latency breakdown and connection statistics.">
          <div className="space-y-6">
            <div className="grid gap-6 lg:grid-cols-[1.15fr_1fr]">
              <div>
                <SectionLabel icon="clock">Latency history</SectionLabel>
                <Sparkline values={client.latencyHistory} label="Latency history" />
                <div className="num mt-1.5 flex justify-between font-mono text-[11px] text-faint">
                  <span>
                    {client.latencyHistory.length > 1
                      ? `${Math.min(...client.latencyHistory).toFixed(1)}–${Math.max(...client.latencyHistory).toFixed(1)} ms over ${client.latencyHistory.length} samples`
                      : "waiting for samples"}
                    {s.latency_uncertainty_ms != null && ` · ±${s.latency_uncertainty_ms.toFixed(1)} ms clock sync`}
                  </span>
                  <span>now</span>
                </div>
              </div>
              <div>
                <SectionLabel icon="gauge">Where the time goes</SectionLabel>
                <LatencyBar breakdown={s.latency_breakdown} />
              </div>
            </div>
            <div className="grid gap-5 md:grid-cols-2">
              <div>
                <SectionLabel icon="monitor">This device</SectionLabel>
                <div className="grid grid-cols-2 gap-2">
                  <Stat label="Decoder" value={s.decoder} />
                  <Stat label="Decode time" value={s.stats.decode_ms_avg.toFixed(1)} unit="ms" />
                  <Stat label="Dropped frames" value={s.stats.frames_dropped.toLocaleString()} hint="Frames replaced before they could be drawn" />
                  <Stat label="Jitter" value={s.stats.jitter_ms.toFixed(1)} unit="ms" />
                  <Stat label="Video bitrate" value={fmtKbps(s.video_bitrate_kbps)} />
                  <Stat label="Round trip" value={s.rtt_ms != null ? s.rtt_ms.toFixed(1) : "—"} unit={s.rtt_ms != null ? "ms" : undefined} />
                  <Stat label="Packets lost" value={s.stats.packets_lost.toLocaleString()} tone={s.stats.packets_lost > 0 && s.stats.packets_received > 0 && s.stats.packets_lost / s.stats.packets_received > 0.01 ? "warn" : undefined} />
                </div>
              </div>
              {s.host && (
                <div>
                  <SectionLabel icon="host">Host</SectionLabel>
                  <div className="grid grid-cols-2 gap-2">
                    <Stat label="Encoder" value={s.host.encoder} unit={`${s.host.encode_ms_avg.toFixed(1)} ms`} hint={s.host.capture_ms_avg != null ? `Screen grab ${s.host.capture_ms_avg.toFixed(1)} ms` : undefined} />
                    <Stat label="Stream size" value={`${s.host.width}×${s.host.height}`} />
                    <Stat label="Capture" value={s.host.capture_fps.toFixed(0)} unit="fps" />
                    <Stat label="Target bitrate" value={fmtKbps(s.host.target_bitrate_kbps)} hint="Adaptive bitrate controller output" />
                  </div>
                </div>
              )}
            </div>
            <Button size="sm" variant="ghost" icon="signal" disabled={!client.running} onClick={() => cmd("overlay")}>
              Show stats in the game window
            </Button>
          </div>
        </Disclosure>
      )}
    </div>
  );
}
