import { useEffect, useState } from "react";
import { Icon } from "../components/Icon";
import { Avatar, Badge, Banner, Button, Card, CountdownRing, EmptyState, Field, LiveDot, NumberInput, PageHeader, SectionLabel, Select, Stat, Toggle } from "../components/ui";
import { useApp } from "../hooks/useAppState";
import { api, errorMessage } from "../services/api";
import { fmtAgo, fmtKbps, sessionStateText } from "../services/format";
import type { CaptureBackendInfo } from "../types/api";

// Matches DEFAULT_CODE_TTL in crates/session-manager/src/pairing.rs.
const CODE_TTL_SECS = 300;

function useCountdown(until: number | null) {
  const [now, setNow] = useState(Date.now());
  useEffect(() => {
    if (!until) return;
    const t = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(t);
  }, [until]);
  return until ? Math.max(0, Math.round((until - now) / 1000)) : 0;
}

/** Splits a fingerprint like "9F2C·41AB·E07D" into readable groups. */
function FingerprintChips({ value }: { value: string }) {
  const groups = value.split(/[·:\s-]+/).filter(Boolean);
  return (
    <span className="inline-flex flex-wrap gap-1 align-middle" aria-label={`fingerprint ${value}`}>
      {groups.map((g, i) => (
        <span key={i} className="rounded-md border border-line-strong bg-sunken px-1.5 py-0.5 font-mono text-xs text-ink">
          {g}
        </span>
      ))}
    </span>
  );
}

function ConsentCard({ title, children, onApprove, onDecline, approveLabel }: { title: string; children: React.ReactNode; onApprove: () => void; onDecline: () => void; approveLabel: string }) {
  return (
    <div role="alert" className="card fade-in flex flex-wrap items-center gap-4 rounded-[var(--radius-card)] border border-warn/45 p-5 shadow-[0_0_0_4px_rgb(233_180_76/0.06)]">
      <span className="flex h-11 w-11 shrink-0 items-center justify-center rounded-[13px] bg-warn/12 text-warn">
        <Icon name="shield" size={21} />
      </span>
      <div className="min-w-0 flex-1">
        <div className="text-[15px] font-semibold">{title}</div>
        <div className="mt-1 text-sm text-muted">{children}</div>
      </div>
      <div className="flex gap-2">
        <Button variant="secondary" onClick={onDecline}>
          Decline
        </Button>
        <Button variant="primary" icon="check" onClick={onApprove}>
          {approveLabel}
        </Button>
      </div>
    </div>
  );
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
  const online = hostRunning && !!host?.signaling_connected;
  const relayed = hostStats && (hostStats.transport.local_candidate_type === "relay" || hostStats.transport.remote_candidate_type === "relay");

  return (
    <div className="fade-in space-y-6">
      <PageHeader title="Host" subtitle="Share this PC with devices you have paired. Every new device and every session needs your approval." />

      {/* Service status */}
      <section className={`card relative overflow-hidden rounded-[22px] border p-6 ${online ? "border-ok/30" : hostRunning ? "border-warn/30" : "border-line"}`}>
        {online && <div className="pointer-events-none absolute -left-20 -top-28 h-64 w-64 rounded-full bg-ok/10 blur-3xl" aria-hidden="true" />}
        <div className="relative flex flex-wrap items-center gap-5">
          <button
            type="button"
            onClick={toggleService}
            disabled={busy}
            aria-label={hostRunning ? "Stop hosting" : "Start hosting"}
            className={`flex h-16 w-16 shrink-0 items-center justify-center rounded-full border-2 transition-all disabled:opacity-60 ${hostRunning ? "border-ok/60 bg-ok/12 text-ok hover:bg-ok/20" : "border-line-strong bg-raised text-muted hover:border-accent/60 hover:text-accent"}`}
          >
            {busy ? <span className="h-6 w-6 animate-spin rounded-full border-2 border-current border-t-transparent" /> : <Icon name="power" size={26} />}
          </button>
          <div className="min-w-0 flex-1">
            <div className="flex items-center gap-2.5">
              <h2 className="text-xl font-semibold tracking-tight">{hostRunning ? (online ? "Hosting is on" : "Hosting is starting") : "Hosting is off"}</h2>
              {hostRunning && <LiveDot tone={online ? "ok" : "warn"} live={online} />}
            </div>
            <p className="mt-1 text-sm text-muted">
              {!hostRunning ? "Turn it on to let paired devices request a session." : online ? "Paired devices can find this PC and request a session." : "Connecting to the signaling server…"}
            </p>
            {hostRunning && host?.signaling_detail && <p className="mt-1 truncate font-mono text-[11px] text-faint">{host.signaling_detail}</p>}
          </div>
          <div className="flex gap-2">
            {hostRunning && (
              <Button variant="danger" icon="x" disabled={live.length === 0} onClick={() => run(api.hostDisconnectAll)}>
                Disconnect everyone
              </Button>
            )}
            <Button variant={hostRunning ? "secondary" : "primary"} icon="power" busy={busy} onClick={toggleService}>
              {hostRunning ? "Stop hosting" : "Start hosting"}
            </Button>
          </div>
        </div>
      </section>

      {/* Consent prompts first: they are time-sensitive. */}
      {host?.pending_pairings.map((p) => (
        <ConsentCard
          key={p.request_id}
          title={`“${p.client_name}” wants to pair with this PC`}
          approveLabel="Approve"
          onApprove={() => run(() => api.hostDecidePairing(p.request_id, true))}
          onDecline={() => run(() => api.hostDecidePairing(p.request_id, false))}
        >
          Check that the fingerprint matches the one shown on that device: <FingerprintChips value={p.fingerprint} />. Only approve if you started this pairing yourself.
        </ConsentCard>
      ))}
      {sessionRequests.map((r) => (
        <ConsentCard
          key={r.session_id}
          title={`“${r.client_name}” wants to stream this PC`}
          approveLabel="Allow"
          onApprove={() => run(() => api.hostDecideSession(r.session_id, true))}
          onDecline={() => run(() => api.hostDecideSession(r.session_id, false))}
        >
          {r.settings.width}×{r.settings.height} at {r.settings.fps} fps, up to {fmtKbps(r.settings.max_bitrate_kbps)} · <FingerprintChips value={r.fingerprint} />
        </ConsentCard>
      ))}

      <div className="grid gap-6 lg:grid-cols-[1.25fr_1fr]">
        <Card
          title="Pairing"
          icon="link"
          subtitle="Pair a new device with a one-time code."
          actions={hostRunning && code && <CountdownRing fraction={remaining / CODE_TTL_SECS} label={`${Math.floor(remaining / 60)}:${String(remaining % 60).padStart(2, "0")}`} size={48} />}
        >
          {!hostRunning ? (
            <p className="text-sm text-muted">Start hosting to create a pairing code.</p>
          ) : code ? (
            <div className="space-y-4">
              <div>
                <span className="sr-only">Pairing code {code.split("").join(" ")}</span>
                <div className="flex flex-wrap items-center gap-1.5" aria-hidden="true">
                  {code.split("").map((ch, i) =>
                    ch === "-" ? (
                      <span key={i} className="mx-1 h-0.5 w-3 rounded bg-line-strong" aria-hidden="true" />
                    ) : (
                      <span key={i} aria-hidden="true" className="flex h-14 w-10 items-center justify-center rounded-[11px] border border-line-strong bg-sunken font-mono text-2xl font-semibold text-ink shadow-[inset_0_-2px_0_rgb(60_203_180/0.35)]">
                        {ch}
                      </span>
                    ),
                  )}
                </div>
              </div>
              <p className="text-sm text-muted">Enter this code on the other device under Connect. It works once, and you approve the device here.</p>
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

        <Card title="Remote control" icon="keyboard" subtitle="What connected devices may do.">
          <Toggle
            label="Allow keyboard, mouse and controller input"
            description="Turning this off takes effect immediately and releases every held key and button."
            checked={host?.input_allowed ?? config?.host.input_allowed ?? true}
            disabled={!hostRunning}
            onChange={(v) => run(() => api.hostSetInputAllowed(v))}
          />
          {config && (
            <Toggle
              label="Approve sessions from paired devices automatically"
              description="Off by default. New pairings always need your approval."
              checked={config.host.trust_paired_sessions}
              onChange={(v) => run(() => saveConfig({ ...config, host: { ...config.host, trust_paired_sessions: v } }))}
            />
          )}
        </Card>
      </div>

      <Card title="Connected clients" icon="user" actions={live.length > 0 && <Badge tone="ok">{live.length} connected</Badge>}>
        {live.length === 0 ? (
          <EmptyState icon="monitor" title="Nobody is connected">
            {hostRunning ? "Paired devices can request a session; you will see the request above." : "Hosting is off."}
          </EmptyState>
        ) : (
          <ul className="space-y-2">
            {live.map((s) => (
              <li key={s.session_id} className="flex items-center gap-4 rounded-[14px] border border-line bg-sunken/60 px-4 py-3">
                <Avatar name={s.client_name} id={s.client_id} />
                <div className="min-w-0 flex-1">
                  <div className="truncate text-sm font-semibold">{s.client_name}</div>
                  <div className="text-xs text-muted">{sessionStateText(s.state)}</div>
                </div>
                {hostStats?.session_id === s.session_id && hostStats.client?.latency_ms != null && (
                  <span className="num hidden font-mono text-sm sm:inline">
                    {hostStats.client.latency_ms.toFixed(1)} <span className="text-muted">ms</span>
                  </span>
                )}
                <Badge tone={s.state.state === "connected" ? "ok" : s.state.state === "reconnecting" ? "warn" : "neutral"}>
                  {s.state.state === "connected" && <LiveDot tone="ok" size={6} live />}
                  {s.state.state.replace(/_/g, " ")}
                </Badge>
                <Button size="sm" variant="danger" onClick={() => run(() => api.hostEndSession(s.session_id))}>
                  End
                </Button>
              </li>
            ))}
          </ul>
        )}
        {hostStats && (
          <div className="mt-5 grid gap-5 md:grid-cols-2">
            <div>
              <SectionLabel icon="cpu">Video</SectionLabel>
              <div className="grid grid-cols-2 gap-2">
                <Stat label="Encoder" value={hostStats.host.encoder} unit={hostStats.encoder_hardware ? "HW" : "SW"} />
                <Stat label="Resolution" value={`${hostStats.host.width}×${hostStats.host.height}`} />
                <Stat label="Capture" value={hostStats.host.capture_fps.toFixed(0)} unit="fps" />
                <Stat label="Encode time" value={hostStats.host.encode_ms_avg.toFixed(1)} unit="ms" hint={hostStats.host.capture_ms_avg != null ? `Screen grab ${hostStats.host.capture_ms_avg.toFixed(1)} ms` : undefined} />
              </div>
            </div>
            <div>
              <SectionLabel icon="globe">Network</SectionLabel>
              <div className="grid grid-cols-2 gap-2">
                <Stat label="Target bitrate" value={fmtKbps(hostStats.host.target_bitrate_kbps)} hint="Adaptive bitrate controller output" />
                <Stat label="Path" value={relayed ? "TURN relay" : hostStats.transport.local_candidate_type ? "Direct" : "—"} tone={relayed ? "warn" : undefined} />
                <Stat label="Round trip" value={hostStats.transport.remote_rtt_ms != null ? hostStats.transport.remote_rtt_ms.toFixed(1) : "—"} unit={hostStats.transport.remote_rtt_ms != null ? "ms" : undefined} />
                <Stat
                  label="Loss"
                  value={hostStats.transport.remote_fraction_lost != null ? (hostStats.transport.remote_fraction_lost * 100).toFixed(1) : "—"}
                  unit={hostStats.transport.remote_fraction_lost != null ? "%" : undefined}
                  tone={(hostStats.transport.remote_fraction_lost ?? 0) > 0.02 ? "warn" : undefined}
                />
              </div>
            </div>
            <div>
              <SectionLabel icon="monitor">Client</SectionLabel>
              <div className="grid grid-cols-2 gap-2">
                <Stat label="Latency" value={hostStats.client?.latency_ms != null ? hostStats.client.latency_ms.toFixed(1) : "—"} unit={hostStats.client?.latency_ms != null ? "ms" : undefined} hint="Capture to client display, measured by the client" />
                <Stat label="Frame rate" value={hostStats.client ? hostStats.client.rendered_fps.toFixed(0) : "—"} unit={hostStats.client ? "fps" : undefined} />
              </div>
            </div>
            <div>
              <SectionLabel icon="gamepad">Input</SectionLabel>
              <div className="grid grid-cols-2 gap-2">
                <Stat
                  label="Events"
                  value={hostStats.input.applied.toLocaleString()}
                  unit={`${hostStats.input.denied + hostStats.input.rate_limited + hostStats.input.invalid} rejected`}
                  hint={`${hostStats.input_backend} / controllers: ${hostStats.controller_backend}`}
                />
                <Stat label="Rumble sent" value={(hostStats.input.rumble ?? 0).toLocaleString()} hint="Force feedback forwarded from games to the client's controller" />
              </div>
            </div>
          </div>
        )}
      </Card>

      <div className="grid gap-6 lg:grid-cols-2">
        <Card title="Display" icon="monitor">
          {displays === null ? (
            <p className="text-sm text-muted">Detecting displays…</p>
          ) : allDisplays.length === 0 ? (
            <Banner tone="error" title="No capture backend available">
              {captureErrors.map((b) => `${b.backend}: ${b.error}`).join("; ") || "No display server detected."}
            </Banner>
          ) : (
            config && (
              <Field label="Display to stream" hint={allDisplays[0]!.backend === "wayland_portal" ? "On Wayland the system asks which screen to share when a session starts." : undefined}>
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

        <Card title="Stream limits" icon="sliders" subtitle="Upper bounds for what clients may request.">
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

      <Card title="Paired devices" icon="lock" subtitle="Devices allowed to request a session. Revoking takes effect immediately.">
        {(host?.paired_devices.length ?? 0) === 0 ? (
          <p className="text-sm text-muted">{hostRunning ? "No devices are paired with this host." : "Start hosting to manage paired devices."}</p>
        ) : (
          <ul className="divide-y divide-line">
            {host!.paired_devices.map((d) => (
              <li key={d.device_id} className="flex items-center gap-4 py-3">
                <Avatar name={d.name} id={d.device_id} size={32} />
                <div className="min-w-0 flex-1">
                  <div className="truncate text-sm font-medium">{d.name}</div>
                  <div className="truncate text-xs text-faint">
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
