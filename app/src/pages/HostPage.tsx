import { useEffect, useState } from "react";
import { Icon, type IconName } from "../components/Icon";
import { Avatar, Badge, Banner, Button, Card, CountdownRing, EmptyState, Field, LiveDot, NumberInput, PageHeader, SectionLabel, Select, Stat, Toggle } from "../components/ui";
import { useApp } from "../hooks/useAppState";
import { api, errorMessage } from "../services/api";
import { fmtAgo, fmtFingerprint, fmtKbps, sessionStateText } from "../services/format";
import type { CaptureBackendInfo, ConnectionTest, HostStatsEvent, ServerInfo, SessionInfo, StepStatus } from "../types/api";

// Matches DEFAULT_CODE_TTL in core/src/sessions/pairing.rs.
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

function ConsentCard({
  title,
  children,
  onApprove,
  onDecline,
  approveLabel,
  approveIcon = "check",
  alternative,
}: {
  title: string;
  children: React.ReactNode;
  onApprove: () => void;
  onDecline: () => void;
  approveLabel: string;
  approveIcon?: IconName;
  /** A second way to approve (e.g. "Watch only"). */
  alternative?: { label: string; icon: IconName; onClick: () => void };
}) {
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
        {alternative && (
          <Button variant="secondary" icon={alternative.icon} onClick={alternative.onClick}>
            {alternative.label}
          </Button>
        )}
        <Button variant="primary" icon={approveIcon} onClick={onApprove}>
          {approveLabel}
        </Button>
      </div>
    </div>
  );
}

function CopyChip({ text }: { text: string }) {
  const [copied, setCopied] = useState(false);
  return (
    <button
      type="button"
      onClick={() => navigator.clipboard?.writeText(text).then(() => { setCopied(true); setTimeout(() => setCopied(false), 1500); })}
      className="inline-flex items-center gap-2 rounded-[10px] border border-line-strong bg-sunken px-3 py-1.5 font-mono text-sm text-ink transition-colors hover:border-accent/60"
      title="Copy"
    >
      {text}
      <Icon name={copied ? "check" : "copy"} size={14} className={copied ? "text-ok" : "text-faint"} />
    </button>
  );
}

/** What another PC needs to reach this host's built-in server. */
const STEP_ICON: Record<StepStatus, { icon: IconName; className: string }> = {
  ok: { icon: "check", className: "text-ok" },
  warn: { icon: "alert", className: "text-warn" },
  fail: { icon: "x", className: "text-bad" },
  skipped: { icon: "chevron", className: "text-faint" },
};

function TestResults({ result }: { result: ConnectionTest }) {
  return (
    <ul className="mt-3 space-y-2 rounded-[14px] border border-line bg-sunken/60 p-3.5">
      {result.steps.map((s) => (
        <li key={s.label} className="flex gap-2.5 text-sm">
          <Icon name={STEP_ICON[s.status].icon} size={16} className={`mt-0.5 shrink-0 ${STEP_ICON[s.status].className}`} />
          <div className="min-w-0">
            <div className="font-medium text-ink">{s.label}</div>
            <div className="text-muted">{s.detail}</div>
          </div>
        </li>
      ))}
    </ul>
  );
}

function HowToConnect({ server }: { server: ServerInfo }) {
  const { notify } = useApp();
  const [testing, setTesting] = useState(false);
  const [result, setResult] = useState<ConnectionTest | null>(null);
  const lanIp = server.lan_addresses[0]?.split(":")[0];
  const mapping = server.port_mapping;
  const open = mapping?.state === "open" ? mapping.external_address : null;
  // Where the port is open on the router; else the address the internet sees.
  const share = open ?? result?.share_address ?? (server.public_ip ? `${server.public_ip}:${server.port}` : null);
  const routerIp = open?.split(":")[0];
  const twoLines = routerIp && server.public_ip && routerIp !== server.public_ip;

  async function test() {
    setTesting(true);
    setResult(null);
    try {
      setResult(await api.hostTestConnection());
    } catch (e) {
      notify("error", errorMessage(e));
    } finally {
      setTesting(false);
    }
  }

  return (
    <Card title="How others connect" icon="globe" subtitle="This PC runs Mist's server itself; nothing else needs to be installed.">
      <SectionLabel icon="globe">Over the internet</SectionLabel>
      <p className="mb-3 text-sm text-muted">On the other PC, type this address under Connect:</p>
      <div className="flex flex-wrap items-center gap-3">
        {share ? (
          <CopyChip text={share} />
        ) : (
          <span className="flex items-center gap-2 text-sm text-muted">
            <span className="h-3.5 w-3.5 animate-spin rounded-full border-2 border-accent border-t-transparent" aria-hidden="true" />
            Finding your public address…
          </span>
        )}
        <Button size="sm" icon="radar" busy={testing} onClick={test}>
          Test connection
        </Button>
      </div>
      <div className="mt-3 text-sm text-muted">
        {open && <p>Port {server.port} was opened on your router automatically (UPnP).</p>}
        {twoLines && (
          <p className="mt-1 text-warn">
            Your internet traffic leaves through {server.public_ip}, but the router opened the port on {routerIp} (two internet lines, or another router in front). Share the address above.
          </p>
        )}
        {mapping?.state === "pending" && <p>Asking your router to open port {server.port}…</p>}
        {(mapping?.state === "failed" || mapping?.state === "off") && (
          <>
            {mapping.state === "failed" && <p className="mb-2 text-warn">{mapping.reason}</p>}
            <p>
              In your router, forward <span className="font-medium text-ink">TCP port {server.port}</span> to this PC{lanIp ? <> (<span className="font-mono text-ink">{lanIp}</span>)</> : null}.
            </p>
          </>
        )}
      </div>
      {result && <TestResults result={result} />}
      <p className="mt-3 text-xs text-faint">Video then flows directly between the PCs. Some networks block that; a relay (TURN) server helps there.</p>

      <div className="mt-5 border-t border-line pt-4">
        <SectionLabel icon="monitor">On the same network</SectionLabel>
        <p className="mb-2 text-sm text-muted">The other PC finds this one under Connect → On this network, or by this PC's local address:</p>
        <div className="flex flex-wrap gap-2">
          {server.lan_addresses.length > 0 ? server.lan_addresses.map((a) => <CopyChip key={a} text={a} />) : <span className="text-sm text-warn">No network connection found.</span>}
        </div>
      </div>
      <div className="mt-5 rounded-[14px] border border-line bg-sunken/60 p-4">
        <SectionLabel icon="shield">Certificate fingerprint</SectionLabel>
        <p className="mb-3 text-sm text-muted">The first time another PC connects, it shows a fingerprint. It must match this one exactly.</p>
        <div className="grid grid-cols-4 gap-1.5 font-mono text-xs text-ink sm:grid-cols-8">
          {fmtFingerprint(server.fingerprint)
            .split(" ")
            .map((g, i) => (
              <span key={i} className="rounded-md border border-line-strong bg-sunken px-1.5 py-1 text-center">
                {g}
              </span>
            ))}
        </div>
      </div>
    </Card>
  );
}

/** One connected person: role, controllers, keyboard/mouse, connection. */
function PersonRow({ session: s, stats, run }: { session: SessionInfo; stats: HostStatsEvent | undefined; run: (f: () => Promise<unknown>) => Promise<void> }) {
  const playing = s.role === "play";
  const relayed = stats && (stats.transport.local_candidate_type === "relay" || stats.transport.remote_candidate_type === "relay");
  const rtt = stats?.transport.remote_rtt_ms;
  const loss = stats?.transport.remote_fraction_lost;
  const latency = stats?.client?.latency_ms;
  const refused = stats?.input.pads_refused ?? 0;
  const details = [
    playing ? (s.controllers.length > 0 ? `Playing · controller ${s.controllers.map((c) => `P${c + 1}`).join(", ")}` : "Playing · no controller yet") : "Watching",
    playing && s.keyboard_mouse ? "keyboard & mouse" : null,
    latency != null ? `${latency.toFixed(0)} ms` : null,
    rtt != null ? `RTT ${rtt.toFixed(0)} ms` : null,
    loss != null && loss > 0.005 ? `${(loss * 100).toFixed(1)}% loss` : null,
    relayed ? "via relay" : null,
  ].filter(Boolean);
  return (
    <li className="flex flex-wrap items-center gap-x-4 gap-y-2 rounded-[14px] border border-line bg-sunken/60 px-4 py-3">
      <Avatar name={s.client_name} id={s.client_id} />
      <div className="min-w-0 flex-1">
        <div className="flex items-center gap-2">
          <span className="truncate text-sm font-semibold">{s.client_name}</span>
          <Badge tone={s.state.state === "connected" ? "ok" : s.state.state === "reconnecting" ? "warn" : "neutral"}>
            {s.state.state === "connected" && <LiveDot tone="ok" size={6} live />}
            {sessionStateText(s.state)}
          </Badge>
        </div>
        <div className="mt-0.5 truncate text-xs text-muted">{details.join(" · ")}</div>
        {refused > 0 && <div className="mt-0.5 text-xs text-warn">All 4 host controllers are in use; one of their controllers was not added.</div>}
      </div>
      <div className="flex items-center gap-1.5">
        <div className="flex overflow-hidden rounded-[10px] border border-line-strong" role="group" aria-label={`Role of ${s.client_name}`}>
          {(["play", "watch"] as const).map((r) => (
            <button
              key={r}
              type="button"
              aria-pressed={s.role === r}
              onClick={() => s.role !== r && run(() => api.hostSetRole(s.session_id, r))}
              className={`inline-flex items-center gap-1.5 px-2.5 py-1 text-xs font-medium transition-colors ${s.role === r ? "bg-accent/15 text-accent" : "text-muted hover:text-ink"}`}
            >
              <Icon name={r === "play" ? "gamepad" : "eye"} size={14} />
              {r === "play" ? "Play" : "Watch"}
            </button>
          ))}
        </div>
        {playing && (
          <button
            type="button"
            aria-pressed={s.keyboard_mouse}
            title={s.keyboard_mouse ? "Keyboard and mouse allowed; click to take them away" : "Allow keyboard and mouse"}
            onClick={() => run(() => api.hostSetKeyboardMouse(s.session_id, !s.keyboard_mouse))}
            className={`inline-flex items-center gap-1.5 rounded-[10px] border px-2.5 py-1 text-xs font-medium transition-colors ${s.keyboard_mouse ? "border-accent/50 bg-accent/15 text-accent" : "border-line-strong text-muted hover:text-ink"}`}
          >
            <Icon name="keyboard" size={14} />
            Keys & mouse
          </button>
        )}
        <Button size="sm" variant="danger" onClick={() => run(() => api.hostEndSession(s.session_id))}>
          End
        </Button>
      </div>
    </li>
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
  // Encoder, resolution and bitrate are the same in everyone's stats.
  const stream = live.map((s) => hostStats[s.session_id]).find((s) => s !== undefined);

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
          title={`“${r.client_name}” wants to join`}
          approveLabel="Let them play"
          approveIcon="gamepad"
          alternative={{ label: "Watch only", icon: "eye", onClick: () => run(() => api.hostDecideSession(r.session_id, true, "watch")) }}
          onApprove={() => run(() => api.hostDecideSession(r.session_id, true, "play"))}
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
            description="For everyone at once. Turning this off takes effect immediately and releases every held key and button. Set each person's role under Connected people."
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

      {hostRunning && host?.server && <HowToConnect server={host.server} />}

      <Card
        title="Connected people"
        icon="users"
        subtitle="Everyone sees the same stream. Players use their own controller; watchers only see and hear."
        actions={live.length > 0 && <Badge tone="ok">{live.length} of {host?.max_clients ?? config?.host.max_clients ?? 8}</Badge>}
      >
        {live.length === 0 ? (
          <EmptyState icon="monitor" title="Nobody is connected">
            {hostRunning ? "Paired devices can request a session; you will see the request above." : "Hosting is off."}
          </EmptyState>
        ) : (
          <ul className="space-y-2">
            {live.map((s) => (
              <PersonRow key={s.session_id} session={s} stats={hostStats[s.session_id]} run={run} />
            ))}
          </ul>
        )}
        {stream && (
          <div className="mt-5">
            <SectionLabel icon="cpu">Shared stream</SectionLabel>
            <div className="grid grid-cols-2 gap-2 md:grid-cols-5">
              <Stat label="Encoder" value={stream.host.encoder} unit={stream.encoder_hardware ? "HW" : "SW"} hint={`input: ${stream.input_backend} / controllers: ${stream.controller_backend}`} />
              <Stat label="Resolution" value={`${stream.host.width}×${stream.host.height}`} />
              <Stat label="Capture" value={stream.host.capture_fps.toFixed(0)} unit="fps" />
              <Stat label="Encode time" value={stream.host.encode_ms_avg.toFixed(1)} unit="ms" hint={stream.host.capture_ms_avg != null ? `Screen grab ${stream.host.capture_ms_avg.toFixed(1)} ms` : undefined} />
              <Stat label="Bitrate" value={fmtKbps(stream.host.target_bitrate_kbps)} hint="Follows the person with the weakest connection" />
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
              {captureErrors.map((b) => `${b.backend}: ${b.error}`).join("; ") || "No display found."}
            </Banner>
          ) : (
            config && (
              <Field label="Display to stream">
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
