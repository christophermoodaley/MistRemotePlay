// "Host this PC": a short guided setup the first time (name, start, pair a
// device), then a simple view of who is connected. Expert options live in
// Settings → Hosting.
import { useEffect, useRef, useState } from "react";
import type { Page } from "../App";
import { Icon, type IconName } from "../components/Icon";
import { Avatar, Badge, Button, CopyChip, CountdownRing, Disclosure, FingerprintGrid, LiveDot, SectionLabel, Spinner, Stat, Steps, TextInput, Toggle } from "../components/ui";
import { useApp } from "../hooks/useAppState";
import type { SettingsTab } from "./SettingsPage";
import { api, errorMessage } from "../services/api";
import { fmtAgo, fmtKbps, sessionStateText } from "../services/format";
import type { ConnectionTest, HostStatsEvent, ServerInfo, SessionInfo, StepStatus } from "../types/api";

// Matches DEFAULT_CODE_TTL in core/src/sessions/pairing.rs.
const CODE_TTL_SECS = 300;
const SETUP_STEPS = ["Name this PC", "Start sharing", "Add a device"];
/** Remembers that sharing was set up once, so the guide isn't shown again. */
const SETUP_DONE_KEY = "mist.host.setupDone";

function readSetupDone(): boolean {
  try {
    return localStorage.getItem(SETUP_DONE_KEY) === "1";
  } catch {
    return false;
  }
}

function writeSetupDone() {
  try {
    localStorage.setItem(SETUP_DONE_KEY, "1");
  } catch {
    // Only a convenience: the guide just shows again.
  }
}

function useCountdown(until: number | null) {
  const [now, setNow] = useState(Date.now());
  useEffect(() => {
    if (!until) return;
    const t = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(t);
  }, [until]);
  return until ? Math.max(0, Math.round((until - now) / 1000)) : 0;
}

export function HostPage({ go, openSettings }: { go: (p: Page) => void; openSettings: (t: SettingsTab) => void }) {
  const { host, hostRunning, startHost, stopHost, notify } = useApp();
  const [busy, setBusy] = useState(false);
  const [setupDone, setSetupDone] = useState(readSetupDone);
  const paired = host?.paired_devices.length ?? 0;

  useEffect(() => {
    if (paired > 0 && !setupDone) {
      writeSetupDone();
      setSetupDone(true);
    }
  }, [paired, setupDone]);

  async function run(f: () => Promise<unknown>) {
    try {
      await f();
    } catch (e) {
      notify("error", errorMessage(e));
    }
  }

  async function toggle() {
    setBusy(true);
    await run(() => (hostRunning ? stopHost() : startHost()));
    setBusy(false);
  }

  const finish = () => {
    writeSetupDone();
    setSetupDone(true);
  };

  // First time: the guide.
  if (!setupDone) {
    const step = !hostRunning ? 0 : 2;
    return (
      <div className="fade-in mx-auto max-w-2xl space-y-6">
        <div>
          <h1 className="text-[28px] font-semibold leading-tight tracking-[-0.02em]">Share this PC</h1>
          <p className="mt-1.5 text-sm text-muted">Let your other PCs or friends play the games on this one. It takes a minute.</p>
        </div>
        <div className="card rounded-[var(--radius-card)] border border-line px-5 py-4">
          <Steps steps={SETUP_STEPS} current={step} />
        </div>
        {!hostRunning ? <NameAndStart busy={busy} onStart={toggle} /> : <PairFirstDevice onSkip={finish} />}
        <p className="text-center text-xs text-faint">
          Nobody can join without your approval.{" "}
          <button type="button" className="text-muted underline-offset-2 hover:text-ink hover:underline" onClick={() => go("home")}>
            Not now
          </button>
        </p>
      </div>
    );
  }

  const sessions = (host?.sessions ?? []).filter((s) => s.state.state !== "ended");
  const online = hostRunning && !!host?.signaling_connected;

  return (
    <div className="fade-in space-y-6">
      <div className="flex flex-wrap items-end justify-between gap-4">
        <div>
          <h1 className="text-[28px] font-semibold leading-tight tracking-[-0.02em]">Host this PC</h1>
          <p className="mt-1.5 text-sm text-muted">Let your paired devices play this PC.</p>
        </div>
        <Button size="sm" variant="ghost" icon="sliders" onClick={() => openSettings("host")}>
          Hosting settings
        </Button>
      </div>

      {/* The one switch that matters. */}
      <section className={`card relative overflow-hidden rounded-[22px] border p-6 ${online ? "border-ok/30" : hostRunning ? "border-warn/30" : "border-line"}`}>
        {online && <div className="pointer-events-none absolute -left-20 -top-28 h-64 w-64 rounded-full bg-ok/10 blur-3xl" aria-hidden="true" />}
        <div className="relative flex flex-wrap items-center gap-5">
          <span className={`flex h-14 w-14 shrink-0 items-center justify-center rounded-full border-2 ${hostRunning ? "border-ok/60 bg-ok/12 text-ok" : "border-line-strong bg-raised text-muted"}`}>
            <Icon name="power" size={24} />
          </span>
          <div className="min-w-0 flex-1">
            <div className="flex items-center gap-2.5">
              <h2 className="text-xl font-semibold tracking-tight">{!hostRunning ? "Sharing is off" : online ? "This PC is shared" : "Starting…"}</h2>
              {hostRunning && <LiveDot tone={online ? "ok" : "warn"} live={online} />}
            </div>
            <p className="mt-1 text-sm text-muted">
              {!hostRunning ? "Turn it on so your devices can play this PC." : online ? "Your paired devices can ask to join. You approve each one." : "Getting ready…"}
            </p>
          </div>
          <Button variant={hostRunning ? "secondary" : "primary"} size="lg" icon="power" busy={busy} onClick={toggle}>
            {hostRunning ? "Stop sharing" : "Start sharing"}
          </Button>
        </div>
      </section>

      {hostRunning && (
        <>
          <PeopleCard sessions={sessions} run={run} />
          <AddDeviceCard />
          {host?.server && <AddressesAndSecurity server={host.server} />}
          <PairedDevices run={run} />
        </>
      )}
    </div>
  );
}

/** Setup step 1 and 2: pick the name others see, then switch on. */
function NameAndStart({ busy, onStart }: { busy: boolean; onStart: () => void }) {
  const { config, saveConfig, notify } = useApp();
  const [name, setName] = useState(config?.device_name ?? "");
  useEffect(() => {
    if (config && !name) setName(config.device_name);
  }, [config]);

  async function start() {
    if (config && name.trim() && name.trim() !== config.device_name) {
      try {
        await saveConfig({ ...config, device_name: name.trim() });
      } catch (e) {
        notify("error", errorMessage(e));
        return;
      }
    }
    onStart();
  }

  return (
    <section className="card rounded-[var(--radius-card)] border border-line p-6">
      <label htmlFor="host-name" className="text-sm font-medium text-ink">
        What should this PC be called?
      </label>
      <p className="mt-0.5 text-xs text-muted">Others see this name when they add it.</p>
      <TextInput id="host-name" value={name} maxLength={64} onChange={(e) => setName(e.target.value)} className="mt-3 h-12 text-base" placeholder="Living room PC" />
      <ul className="mt-6 space-y-2.5 text-sm text-muted">
        <Point icon="monitor">People who join see your screen and hear its sound.</Point>
        <Point icon="gamepad">Their controllers work as if plugged into this PC.</Point>
        <Point icon="shield">Nobody joins without you clicking Approve.</Point>
      </ul>
      <Button variant="primary" size="lg" icon="power" busy={busy} disabled={!name.trim()} onClick={start} className="mt-6 w-full">
        Start sharing
      </Button>
    </section>
  );
}

function Point({ icon, children }: { icon: IconName; children: React.ReactNode }) {
  return (
    <li className="flex items-center gap-3">
      <span className="flex h-8 w-8 shrink-0 items-center justify-center rounded-[10px] bg-accent/12 text-accent">
        <Icon name={icon} size={16} />
      </span>
      {children}
    </li>
  );
}

/** Setup step 3: a code is made for you; the other PC types it in. */
function PairFirstDevice({ onSkip }: { onSkip: () => void }) {
  const { host, info } = useApp();
  const connected = !!host?.signaling_connected;

  return (
    <section className="card rounded-[var(--radius-card)] border border-line p-6">
      <h2 className="text-lg font-semibold tracking-tight">Add your first device</h2>
      {!connected ? (
        <div className="mt-4 flex items-center gap-3 text-sm text-muted">
          <Spinner /> Getting ready…
        </div>
      ) : (
        <>
          <ol className="mt-3 space-y-1.5 text-sm text-muted">
            <li>
              1. On the other PC, install Mist and choose <span className="text-ink">Play on this PC</span>.
            </li>
            <li>
              2. Pick <span className="text-ink">{info?.device_name ?? "this PC"}</span> from the list (or type the address below).
            </li>
            <li>3. Type in this code, then click Approve when it pops up here.</li>
          </ol>
          <div className="mt-5">
            <PairingCode auto />
          </div>
          {host?.server && <ShareAddress server={host.server} />}
        </>
      )}
      <div className="mt-6 border-t border-line pt-4 text-right">
        <button type="button" onClick={onSkip} className="text-sm text-muted hover:text-ink">
          Skip for now
        </button>
      </div>
    </section>
  );
}

/** The current pairing code, or a button to make one. `auto` makes it unasked. */
function PairingCode({ auto = false }: { auto?: boolean }) {
  const { host, pairingExpiresAt, notify } = useApp();
  const expiresAt = pairingExpiresAt ?? host?.pairing_expires_unix_ms ?? null;
  const remaining = useCountdown(expiresAt);
  const code = host?.pairing_code ?? null;
  const connected = !!host?.signaling_connected;
  const failed = useRef(false);

  async function create() {
    try {
      await api.hostGeneratePairingCode();
    } catch (e) {
      failed.current = true;
      notify("error", errorMessage(e));
    }
  }

  // A fresh code whenever there is none (also after one expires).
  useEffect(() => {
    if (auto && connected && !code && !failed.current) create();
  }, [auto, connected, code]);

  if (!code) {
    return auto ? (
      <div className="flex items-center gap-3 text-sm text-muted">
        <Spinner /> Making a code…
      </div>
    ) : (
      <Button variant="primary" icon="plus" onClick={create} disabled={!connected}>
        Get a code
      </Button>
    );
  }

  return (
    <div className="flex flex-wrap items-center gap-5">
      <div>
        <span className="sr-only">Pairing code {code.split("").join(" ")}</span>
        <div className="flex flex-wrap items-center gap-1.5" aria-hidden="true">
          {code.split("").map((ch, i) =>
            ch === "-" ? (
              <span key={i} className="mx-1 h-0.5 w-3 rounded bg-line-strong" />
            ) : (
              <span key={i} className="flex h-14 w-10 items-center justify-center rounded-[11px] border border-line-strong bg-sunken font-mono text-2xl font-semibold text-ink shadow-[inset_0_-2px_0_rgb(60_203_180/0.35)]">
                {ch}
              </span>
            ),
          )}
        </div>
      </div>
      <div className="flex items-center gap-3">
        {expiresAt && <CountdownRing fraction={remaining / CODE_TTL_SECS} label={`${Math.floor(remaining / 60)}:${String(remaining % 60).padStart(2, "0")}`} size={48} />}
        {!auto && (
          <Button size="sm" variant="ghost" icon="x" onClick={() => api.hostCancelPairing().catch((e) => notify("error", errorMessage(e)))}>
            Cancel
          </Button>
        )}
      </div>
    </div>
  );
}

/** The one address someone outside this network types in. */
function shareAddress(server: ServerInfo, test?: ConnectionTest | null): string | null {
  const open = server.port_mapping?.state === "open" ? server.port_mapping.external_address : null;
  return open ?? test?.share_address ?? (server.public_ip ? `${server.public_ip}:${server.port}` : null);
}

function ShareAddress({ server }: { server: ServerInfo }) {
  const share = shareAddress(server);
  return (
    <div className="mt-5 rounded-[14px] border border-line bg-sunken/60 p-4 text-sm">
      <div className="text-muted">Not on the same network? On the other PC, type this address:</div>
      <div className="mt-2">{share ? <CopyChip text={share} /> : <span className="text-faint">Finding your address…</span>}</div>
    </div>
  );
}

function AddDeviceCard() {
  return (
    <section className="card rounded-[var(--radius-card)] border border-line p-5">
      <div className="mb-4 flex items-start gap-3">
        <span className="flex h-8 w-8 shrink-0 items-center justify-center rounded-[10px] border border-line bg-raised text-muted">
          <Icon name="plus" size={16} />
        </span>
        <div>
          <h2 className="text-[15px] font-semibold tracking-tight">Add someone</h2>
          <p className="mt-0.5 text-xs text-muted">On their PC: Mist → Add a host → pick this PC → type the code. Codes work once, for 5 minutes.</p>
        </div>
      </div>
      <PairingCode />
    </section>
  );
}

const STEP_ICON: Record<StepStatus, { icon: IconName; className: string }> = {
  ok: { icon: "check", className: "text-ok" },
  warn: { icon: "alert", className: "text-warn" },
  fail: { icon: "x", className: "text-bad" },
  skipped: { icon: "chevron", className: "text-faint" },
};

function AddressesAndSecurity({ server }: { server: ServerInfo }) {
  const { notify } = useApp();
  const [testing, setTesting] = useState(false);
  const [result, setResult] = useState<ConnectionTest | null>(null);
  const lanIp = server.lan_addresses[0]?.split(":")[0];
  const mapping = server.port_mapping;
  const share = shareAddress(server, result);
  const routerIp = mapping?.state === "open" ? mapping.external_address.split(":")[0] : null;
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
    <Disclosure title="Addresses and security" subtitle="For connecting over the internet, and checking it's really this PC.">
      <SectionLabel icon="globe">Over the internet</SectionLabel>
      <div className="flex flex-wrap items-center gap-3">
        {share ? <CopyChip text={share} /> : <span className="text-sm text-muted">Finding your public address…</span>}
        <Button size="sm" icon="radar" busy={testing} onClick={test}>
          Test it
        </Button>
      </div>
      <div className="mt-3 text-sm text-muted">
        {mapping?.state === "open" && <p>Your router was set up automatically.</p>}
        {twoLines && <p className="mt-1 text-warn">Your traffic leaves through {server.public_ip}, but the router opened the port on {routerIp}. Share the address above.</p>}
        {mapping?.state === "pending" && <p>Setting up your router…</p>}
        {(mapping?.state === "failed" || mapping?.state === "off") && (
          <>
            {mapping.state === "failed" && <p className="mb-2 text-warn">{mapping.reason}</p>}
            <p>
              Your router needs <span className="font-medium text-ink">TCP port {server.port}</span> forwarded to this PC{lanIp ? <> (<span className="font-mono text-ink">{lanIp}</span>)</> : null}.
            </p>
          </>
        )}
      </div>
      {result && (
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
      )}

      <div className="mt-5 border-t border-line pt-4">
        <SectionLabel icon="monitor">On the same network</SectionLabel>
        <p className="mb-2 text-sm text-muted">This PC shows up by itself. Its local address, if needed:</p>
        <div className="flex flex-wrap gap-2">
          {server.lan_addresses.length > 0 ? server.lan_addresses.map((a) => <CopyChip key={a} text={a} />) : <span className="text-sm text-warn">No network connection found.</span>}
        </div>
      </div>
      <div className="mt-5 border-t border-line pt-4">
        <SectionLabel icon="shield">Fingerprint</SectionLabel>
        <p className="mb-3 text-sm text-muted">The first time another PC connects over the internet it shows a fingerprint. It must match this one.</p>
        <FingerprintGrid hex={server.fingerprint} />
      </div>
    </Disclosure>
  );
}

function PeopleCard({ sessions, run }: { sessions: SessionInfo[]; run: (f: () => Promise<unknown>) => Promise<void> }) {
  const { host, hostStats, config } = useApp();
  const stream = sessions.map((s) => hostStats[s.session_id]).find((s) => s !== undefined);
  const max = host?.max_clients ?? config?.host.max_clients ?? 8;
  return (
    <section className="card rounded-[var(--radius-card)] border border-line p-5">
      <header className="mb-4 flex items-start justify-between gap-3">
        <div className="flex items-center gap-3">
          <span className="flex h-8 w-8 shrink-0 items-center justify-center rounded-[10px] border border-line bg-raised text-muted">
            <Icon name="users" size={16} />
          </span>
          <h2 className="text-[15px] font-semibold tracking-tight">Who's connected</h2>
        </div>
        {sessions.length > 0 && (
          <div className="flex items-center gap-2">
            <Badge tone="ok">
              {sessions.length} of {max}
            </Badge>
            <Button size="sm" variant="danger" onClick={() => run(api.hostDisconnectAll)}>
              Disconnect everyone
            </Button>
          </div>
        )}
      </header>
      {sessions.length === 0 ? (
        <p className="rounded-[14px] border border-dashed border-line-strong/70 bg-sunken/40 px-4 py-6 text-center text-sm text-muted">Nobody yet. When someone asks to join, you'll see it at the top of the screen.</p>
      ) : (
        <ul className="space-y-2">
          {sessions.map((s) => (
            <PersonRow key={s.session_id} session={s} stats={hostStats[s.session_id]} run={run} />
          ))}
        </ul>
      )}
      <div className="mt-3 border-t border-line pt-1">
        <Toggle
          label="Let people control this PC"
          description="Keyboard, mouse and controllers. Turn off to let everyone only watch."
          checked={host?.input_allowed ?? config?.host.input_allowed ?? true}
          onChange={(v) => run(() => api.hostSetInputAllowed(v))}
        />
      </div>
      {stream && (
        <details className="mt-2 text-sm">
          <summary className="cursor-pointer text-xs text-faint hover:text-muted">Stream details</summary>
          <div className="mt-3 grid grid-cols-2 gap-2 md:grid-cols-5">
            <Stat label="Encoder" value={stream.host.encoder} unit={stream.encoder_hardware ? "HW" : "SW"} hint={`input: ${stream.input_backend} / controllers: ${stream.controller_backend}`} />
            <Stat label="Resolution" value={`${stream.host.width}×${stream.host.height}`} />
            <Stat label="Capture" value={stream.host.capture_fps.toFixed(0)} unit="fps" />
            <Stat label="Encode time" value={stream.host.encode_ms_avg.toFixed(1)} unit="ms" />
            <Stat label="Bitrate" value={fmtKbps(stream.host.target_bitrate_kbps)} hint="Follows the person with the weakest connection" />
          </div>
        </details>
      )}
    </section>
  );
}

/** One connected person: what they can do, and how their connection is. */
function PersonRow({ session: s, stats, run }: { session: SessionInfo; stats: HostStatsEvent | undefined; run: (f: () => Promise<unknown>) => Promise<void> }) {
  const playing = s.role === "play";
  const relayed = stats && (stats.transport.local_candidate_type === "relay" || stats.transport.remote_candidate_type === "relay");
  const latency = stats?.client?.latency_ms;
  const loss = stats?.transport.remote_fraction_lost;
  const refused = stats?.input.pads_refused ?? 0;
  const details = [
    playing ? (s.controllers.length > 0 ? `Controller ${s.controllers.map((c) => `P${c + 1}`).join(", ")}` : "No controller yet") : "Watching",
    playing && s.keyboard_mouse ? "keyboard & mouse" : null,
    latency != null ? `${latency.toFixed(0)} ms` : null,
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
        {refused > 0 && <div className="mt-0.5 text-xs text-warn">All 4 controller slots are in use; one of their controllers wasn't added.</div>}
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
          Remove
        </Button>
      </div>
    </li>
  );
}

function PairedDevices({ run }: { run: (f: () => Promise<unknown>) => Promise<void> }) {
  const { host } = useApp();
  const devices = host?.paired_devices ?? [];
  return (
    <Disclosure title={`Paired devices (${devices.length})`} subtitle="Devices that may ask to join. Removing one takes effect immediately.">
      {devices.length === 0 ? (
        <p className="text-sm text-muted">No devices yet.</p>
      ) : (
        <ul className="divide-y divide-line">
          {devices.map((d) => (
            <li key={d.device_id} className="flex items-center gap-4 py-3">
              <Avatar name={d.name} id={d.device_id} size={32} />
              <div className="min-w-0 flex-1">
                <div className="truncate text-sm font-medium">{d.name}</div>
                <div className="truncate text-xs text-faint">
                  Added {fmtAgo(d.paired_at_ms)} · last played {fmtAgo(d.last_connected_ms)}
                </div>
              </div>
              <Button size="sm" variant="ghost" icon="x" onClick={() => run(() => api.hostRevokeDevice(d.device_id))}>
                Remove
              </Button>
            </li>
          ))}
        </ul>
      )}
    </Disclosure>
  );
}
