import { useEffect, useState } from "react";
import { Icon } from "../components/Icon";
import { Badge, Banner, Button, Card, EmptyState, Field, NumberInput, PageHeader, Select, Stat, StatusDot, Toggle } from "../components/ui";
import { useApp } from "../hooks/useAppState";
import { api, errorMessage } from "../services/api";
import { fmtAgo, fmtKbps, fmtMs, fmtPercent, sessionStateText } from "../services/format";
import type { CaptureBackendInfo } from "../types/api";

function useCountdown(until: number | null) {
  const [now, setNow] = useState(Date.now());
  useEffect(() => {
    if (!until) return;
    const t = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(t);
  }, [until]);
  return until ? Math.max(0, Math.round((until - now) / 1000)) : 0;
}

export function HostPage() {
  const { host, hostRunning, startHost, stopHost, hostStats, sessionRequests, pairingExpiresAt, config, saveConfig, notify } = useApp();
  const [busy, setBusy] = useState(false);
  const [displays, setDisplays] = useState<CaptureBackendInfo[] | null>(null);
  const remaining = useCountdown(pairingExpiresAt);

  useEffect(() => {
    api.listDisplays().then(setDisplays).catch((e) => notify("error", errorMessage(e)));
  }, [notify]);

  async function run(f: () => Promise<unknown>) {
    try {
      await f();
    } catch (e) {
      notify("error", errorMessage(e));
    }
  }

  async function toggleService() {
    setBusy(true);
    await run(() => (hostRunning ? stopHost() : startHost()));
    setBusy(false);
  }

  const sessions = host?.sessions ?? [];
  const live = sessions.filter((s) => s.state.state !== "ended");
  const allDisplays = (displays ?? []).flatMap((b) => b.displays.map((d) => ({ ...d, backend: b.backend })));
  const captureErrors = (displays ?? []).filter((b) => b.error);
  const code = host?.pairing_code;

  return (
    <div className="fade-in space-y-6">
      <PageHeader
        title="Host"
        subtitle="Share this PC with devices you have paired. Every new device and every session needs your approval."
        actions={
          <Button variant={hostRunning ? "secondary" : "primary"} icon="power" busy={busy} onClick={toggleService}>
            {hostRunning ? "Stop hosting" : "Start hosting"}
          </Button>
        }
      />

      {hostRunning && host && (
        <div className="flex flex-wrap items-center gap-3">
          <StatusDot tone={host.signaling_connected ? "ok" : "warn"} label={host.signaling_connected ? "Online" : "Offline"} />
          <span className="text-xs text-faint">{host.signaling_detail}</span>
        </div>
      )}

      {/* Consent prompts first: they are time-sensitive. */}
      {host?.pending_pairings.map((p) => (
        <Banner
          key={p.request_id}
          tone="warn"
          title={`“${p.client_name}” wants to pair with this PC`}
          action={
            <div className="flex gap-2">
              <Button size="sm" variant="primary" icon="check" onClick={() => run(() => api.hostDecidePairing(p.request_id, true))}>
                Approve
              </Button>
              <Button size="sm" variant="secondary" onClick={() => run(() => api.hostDecidePairing(p.request_id, false))}>
                Decline
              </Button>
            </div>
          }
        >
          Device fingerprint <span className="font-mono text-ink">{p.fingerprint}</span>. Only approve if you started this pairing yourself.
        </Banner>
      ))}
      {sessionRequests.map((r) => (
        <Banner
          key={r.session_id}
          tone="warn"
          title={`“${r.client_name}” wants to stream this PC`}
          action={
            <div className="flex gap-2">
              <Button size="sm" variant="primary" icon="check" onClick={() => run(() => api.hostDecideSession(r.session_id, true))}>
                Allow
              </Button>
              <Button size="sm" variant="secondary" onClick={() => run(() => api.hostDecideSession(r.session_id, false))}>
                Decline
              </Button>
            </div>
          }
        >
          {r.settings.width}×{r.settings.height} at {r.settings.fps} fps, up to {fmtKbps(r.settings.max_bitrate_kbps)} · fingerprint{" "}
          <span className="font-mono text-ink">{r.fingerprint}</span>
        </Banner>
      ))}

      <div className="grid grid-cols-2 gap-6">
        <Card title="Pairing">
          {!hostRunning ? (
            <p className="text-sm text-muted">Start hosting to create a pairing code.</p>
          ) : code ? (
            <div className="flex flex-col items-start gap-4">
              <div>
                <div className="font-mono text-4xl font-semibold tracking-[0.18em] text-ink" aria-label={`Pairing code ${code.split("").join(" ")}`}>
                  {code}
                </div>
                <p className="mt-2 text-sm text-muted">
                  Enter this code on the other device. Expires in {Math.floor(remaining / 60)}:{String(remaining % 60).padStart(2, "0")}. Single use.
                </p>
              </div>
              <Button size="sm" variant="ghost" icon="x" onClick={() => run(api.hostCancelPairing)}>
                Cancel code
              </Button>
            </div>
          ) : (
            <div className="flex flex-col items-start gap-3">
              <p className="text-sm text-muted">Create a short-lived code to pair a new device. You will be asked to approve it.</p>
              <Button variant="primary" icon="plus" onClick={() => run(api.hostGeneratePairingCode)} disabled={!host?.signaling_connected}>
                Create pairing code
              </Button>
              {!host?.signaling_connected && <p className="text-xs text-warn">Waiting for the signaling server connection.</p>}
            </div>
          )}
        </Card>

        <Card title="Remote control">
          <Toggle
            label="Allow keyboard, mouse and controller input"
            description="Turning this off takes effect immediately and releases every held key and button."
            checked={host?.input_allowed ?? config?.host.input_allowed ?? true}
            disabled={!hostRunning}
            onChange={(v) => run(() => api.hostSetInputAllowed(v))}
          />
          <div className="mt-4 flex gap-2">
            <Button variant="danger" icon="power" disabled={!hostRunning || live.length === 0} onClick={() => run(api.hostDisconnectAll)}>
              Disconnect everyone
            </Button>
          </div>
        </Card>
      </div>

      <Card title="Connected clients">
        {live.length === 0 ? (
          <EmptyState icon="monitor" title="Nobody is connected">
            {hostRunning ? "Paired devices can request a session; you will see the request above." : "Hosting is off."}
          </EmptyState>
        ) : (
          <ul className="divide-y divide-line">
            {live.map((s) => (
              <li key={s.session_id} className="flex items-center gap-4 py-3">
                <Icon name="monitor" className="text-muted" />
                <div className="flex-1">
                  <div className="text-sm font-medium">{s.client_name}</div>
                  <div className="text-xs text-faint">{sessionStateText(s.state)}</div>
                </div>
                <Badge tone={s.state.state === "connected" ? "ok" : s.state.state === "reconnecting" ? "warn" : "neutral"}>{s.state.state.replace(/_/g, " ")}</Badge>
                <Button size="sm" variant="danger" onClick={() => run(() => api.hostEndSession(s.session_id))}>
                  End
                </Button>
              </li>
            ))}
          </ul>
        )}
        {hostStats && (
          <div className="mt-4 grid grid-cols-4 gap-3">
            <Stat label="Encoder" value={`${hostStats.host.encoder}${hostStats.encoder_hardware ? " (HW)" : " (SW)"}`} />
            <Stat label="Resolution" value={`${hostStats.host.width}×${hostStats.host.height}`} />
            <Stat label="Capture" value={`${hostStats.host.capture_fps.toFixed(0)} fps`} />
            <Stat label="Encode time" value={fmtMs(hostStats.host.encode_ms_avg)} />
            <Stat label="Target bitrate" value={fmtKbps(hostStats.host.target_bitrate_kbps)} hint="Adaptive bitrate controller output" />
            <Stat label="RTT (RTCP)" value={fmtMs(hostStats.transport.remote_rtt_ms)} />
            <Stat label="Loss (RTCP)" value={fmtPercent(hostStats.transport.remote_fraction_lost)} />
            <Stat label="Path" value={hostStats.transport.local_candidate_type === "relay" || hostStats.transport.remote_candidate_type === "relay" ? "TURN relay" : hostStats.transport.local_candidate_type ?? "—"} />
            <Stat label="Client latency" value={fmtMs(hostStats.client?.latency_ms)} hint="Capture to client display, measured by the client" />
            <Stat label="Client fps" value={hostStats.client ? hostStats.client.rendered_fps.toFixed(0) : "—"} />
            <Stat label="Input" value={`${hostStats.input.applied} ok · ${hostStats.input.denied + hostStats.input.rate_limited + hostStats.input.invalid} rejected`} hint={`${hostStats.input_backend} / controllers: ${hostStats.controller_backend}`} />
            <Stat label="Dropped before encode" value={hostStats.host.frames_dropped_before_encode} />
          </div>
        )}
      </Card>

      <div className="grid grid-cols-2 gap-6">
        <Card title="Display">
          {displays === null ? (
            <p className="text-sm text-muted">Detecting displays…</p>
          ) : allDisplays.length === 0 ? (
            <Banner tone="error" title="No capture backend available">
              {captureErrors.map((b) => `${b.backend}: ${b.error}`).join("; ") || "No display server detected."}
            </Banner>
          ) : (
            config && (
              <Field label="Display to stream" hint={allDisplays[0].backend === "wayland_portal" ? "On Wayland the system asks which screen to share when a session starts." : undefined}>
                {(id) => (
                  <Select
                    id={id}
                    value={config.host.display_id ?? ""}
                    onChange={(v) => run(() => saveConfig({ ...config, host: { ...config.host, display_id: v || null } }))}
                    options={[
                      { value: "", label: "Primary display" },
                      ...allDisplays.map((d) => ({ value: d.id, label: `${d.name} — ${d.width}×${d.height}${d.primary ? " (primary)" : ""}` })),
                    ]}
                  />
                )}
              </Field>
            )
          )}
        </Card>

        <Card title="Stream limits">
          {config && (
            <div className="grid grid-cols-2 gap-4">
              <Field label="Max width">{(id) => <NumberInput id={id} value={config.host.max_width} min={320} max={7680} step={2} onChange={(v) => run(() => saveConfig({ ...config, host: { ...config.host, max_width: v } }))} />}</Field>
              <Field label="Max height">{(id) => <NumberInput id={id} value={config.host.max_height} min={240} max={4320} step={2} onChange={(v) => run(() => saveConfig({ ...config, host: { ...config.host, max_height: v } }))} />}</Field>
              <Field label="Max frame rate">{(id) => <NumberInput id={id} value={config.host.max_fps} min={10} max={240} onChange={(v) => run(() => saveConfig({ ...config, host: { ...config.host, max_fps: v } }))} />}</Field>
              <Field label="Max bitrate (kbps)">{(id) => <NumberInput id={id} value={config.host.max_bitrate_kbps} min={250} max={500000} step={500} onChange={(v) => run(() => saveConfig({ ...config, host: { ...config.host, max_bitrate_kbps: v } }))} />}</Field>
            </div>
          )}
        </Card>
      </div>

      <Card title="Paired devices">
        {(host?.paired_devices.length ?? 0) === 0 ? (
          <p className="text-sm text-muted">{hostRunning ? "No devices are paired with this host." : "Start hosting to manage paired devices."}</p>
        ) : (
          <ul className="divide-y divide-line">
            {host!.paired_devices.map((d) => (
              <li key={d.device_id} className="flex items-center gap-4 py-3">
                <div className="flex-1">
                  <div className="text-sm font-medium">{d.name}</div>
                  <div className="text-xs text-faint">
                    paired {fmtAgo(d.paired_at_ms)} · last session {fmtAgo(d.last_connected_ms)} · <span className="font-mono">{d.device_id.slice(0, 8)}</span>
                  </div>
                </div>
                <Button size="sm" variant="ghost" icon="x" onClick={() => run(() => api.hostRevokeDevice(d.device_id))}>
                  Revoke
                </Button>
              </li>
            ))}
          </ul>
        )}
      </Card>
    </div>
  );
}
