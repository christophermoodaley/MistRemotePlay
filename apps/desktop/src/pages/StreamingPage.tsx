import { useState } from "react";
import type { Page } from "../App";
import { Icon } from "../components/Icon";
import { Badge, Banner, Button, Card, EmptyState, Kbd, PageHeader, Slider, Stat } from "../components/ui";
import { useApp } from "../hooks/useAppState";
import { api, errorMessage } from "../services/api";
import { fmtKbps, fmtMs } from "../services/format";
import type { ClientState } from "../types/api";

const STEPS: { state: ClientState; label: string }[] = [
  { state: "connecting_signaling", label: "Signaling" },
  { state: "waiting_for_approval", label: "Host approval" },
  { state: "negotiating", label: "Secure connection" },
  { state: "streaming", label: "Streaming" },
];

function hint(detail: string): string | null {
  const d = detail.toLowerCase();
  if (d.includes("not paired")) return "Pair with this host again from Connect.";
  if (d.includes("signaling") && (d.includes("reach") || d.includes("timed out")))
    return "Check the signaling server URL in Settings → Network and that the server is running.";
  if (d.includes("ice") || d.includes("peer connection")) return "A direct path failed. Configure a TURN server (Settings → Network) and run the ICE check.";
  if (d.includes("approve")) return "Ask the person at the host to approve the request.";
  if (d.includes("rejected")) return "The host declined. Ask them to approve the next request.";
  return null;
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
        <PageHeader title="Streaming" />
        <EmptyState icon="play" title="No active session">
          Connect to a paired host from Home or Connect. Video plays in the Mist streaming window.
          <div className="mt-4">
            <Button variant="primary" onClick={() => go("connect")}>
              Go to Connect
            </Button>
          </div>
        </EmptyState>
      </div>
    );
  }

  const s = client.stats;
  const stepIndex = STEPS.findIndex((x) => x.state === client.state);
  const failed = client.state === "failed";
  const h = hint(client.detail);

  return (
    <div className="fade-in space-y-6">
      <PageHeader
        title={client.hostName}
        subtitle={client.running ? "The video plays in the Mist streaming window." : "Session finished."}
        actions={
          client.running && (
            <>
              <Button icon="maximize" onClick={() => cmd("fullscreen")}>
                Fullscreen
              </Button>
              <Button variant="danger" icon="power" onClick={() => cmd("disconnect")}>
                Disconnect
              </Button>
            </>
          )
        }
      />

      <Card title="Connection">
        <ol className="mb-4 grid grid-cols-4 gap-2" aria-label="Connection progress">
          {STEPS.map((step, i) => {
            const done = stepIndex > i || client.state === "streaming";
            const current = stepIndex === i;
            return (
              <li key={step.state} className={`rounded-lg border px-3 py-2 text-xs ${current ? "border-accent/60 text-ink" : done ? "border-line text-muted" : "border-line text-faint"}`} aria-current={current ? "step" : undefined}>
                <span className="mr-1.5">{done && !current ? "✓" : `${i + 1}.`}</span>
                {step.label}
              </li>
            );
          })}
        </ol>
        <div className="flex items-center gap-3">
          <Badge tone={client.state === "streaming" ? "ok" : failed ? "bad" : client.state === "reconnecting" ? "warn" : "neutral"}>{client.state.replace(/_/g, " ")}</Badge>
          <span className="text-sm text-muted">{client.detail}</span>
        </div>
        {(failed || (!client.running && client.state !== "streaming")) && h && (
          <div className="mt-4">
            <Banner tone={failed ? "error" : "warn"} title="What to try">
              {h}
            </Banner>
          </div>
        )}
        {client.inputAllowed === false && (
          <div className="mt-4">
            <Banner tone="warn" title="Remote input is disabled by the host">
              You can watch, but keyboard, mouse and controllers are ignored.
            </Banner>
          </div>
        )}
      </Card>

      <Card title="Performance" actions={s?.relayed != null && <Badge tone={s.relayed ? "warn" : "ok"}>{s.relayed ? "TURN relay" : "direct"}</Badge>}>
        {s ? (
          <div className="grid grid-cols-4 gap-3">
            <Stat label="Frame rate" value={`${s.stats.rendered_fps.toFixed(0)} fps`} />
            <Stat label="Latency" value={s.stats.latency_ms != null ? `${s.stats.latency_ms.toFixed(1)} ms` : "—"} hint={`Capture on the host to display here; clock-sync uncertainty ±${s.latency_uncertainty_ms?.toFixed(1) ?? "?"} ms`} />
            <Stat label="Round trip" value={fmtMs(s.rtt_ms)} />
            <Stat label="Video bitrate" value={fmtKbps(s.video_bitrate_kbps)} />
            <Stat label="Decode" value={`${fmtMs(s.stats.decode_ms_avg)} · ${s.decoder}`} />
            <Stat label="Packets lost" value={s.stats.packets_lost} />
            <Stat label="Jitter" value={fmtMs(s.stats.jitter_ms)} />
            <Stat label="Dropped frames" value={s.stats.frames_dropped} hint="Frames replaced before they could be drawn" />
            {s.host && (
              <>
                <Stat label="Host encoder" value={`${s.host.encoder} · ${fmtMs(s.host.encode_ms_avg)}`} />
                <Stat label="Stream size" value={`${s.host.width}×${s.host.height}`} />
                <Stat label="Target bitrate" value={fmtKbps(s.host.target_bitrate_kbps)} />
                <Stat label="Host capture" value={`${s.host.capture_fps.toFixed(0)} fps`} />
              </>
            )}
          </div>
        ) : (
          <p className="text-sm text-muted">Statistics appear once the stream is running.</p>
        )}
      </Card>

      <div className="grid grid-cols-2 gap-6">
        <Card title="Audio">
          <div className="space-y-3">
            <div className="flex items-center gap-3">
              <Icon name="volume" className="text-muted" />
              <div className="flex-1">
                <Slider value={volume} min={0} max={2} step={0.05} format={(v) => `${Math.round(v * 100)}%`} onChange={(v) => { setVolume(v); cmd(`volume ${v.toFixed(2)}`); }} />
              </div>
            </div>
            <div className="flex gap-2">
              <Button size="sm" icon="mute" disabled={!client.running} onClick={() => cmd("mute")}>
                Mute / unmute
              </Button>
              <Button size="sm" variant="ghost" disabled={!client.running} onClick={() => cmd("overlay")}>
                Toggle stats overlay
              </Button>
            </div>
          </div>
        </Card>

        <Card title="Controllers">
          {client.controllers.length === 0 ? (
            <p className="text-sm text-muted">{config?.client.controllers_enabled === false ? "Controller forwarding is off in Settings." : "No controllers detected. Connect one any time; it is forwarded automatically."}</p>
          ) : (
            <ul className="space-y-2">
              {client.controllers.map((c) => (
                <li key={c.slot} className="flex items-center gap-3 text-sm">
                  <Icon name="gamepad" className="text-accent" />
                  <span className="flex-1">{c.name}</span>
                  <Badge>Player {c.slot + 1}</Badge>
                </li>
              ))}
            </ul>
          )}
        </Card>
      </div>

      <Card title="Shortcuts in the streaming window">
        <div className="grid grid-cols-2 gap-2 text-sm text-muted">
          {[
            ["Q", "disconnect"],
            ["F", "fullscreen"],
            ["M", "capture / release mouse"],
            ["S", "statistics overlay"],
            ["A", "mute"],
          ].map(([k, label]) => (
            <div key={k} className="flex items-center gap-2">
              <Kbd>Ctrl</Kbd>+<Kbd>Alt</Kbd>+<Kbd>Shift</Kbd>+<Kbd>{k}</Kbd>
              <span>{label}</span>
            </div>
          ))}
        </div>
      </Card>
    </div>
  );
}
