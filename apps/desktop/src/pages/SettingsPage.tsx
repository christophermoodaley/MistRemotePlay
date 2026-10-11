import { useEffect, useMemo, useState } from "react";
import { Badge, Banner, Button, Card, Field, NumberInput, PageHeader, Select, Slider, TextInput, Toggle } from "../components/ui";
import { ControllerEditor } from "../components/ControllerEditor";
import { useApp } from "../hooks/useAppState";
import { api, errorMessage } from "../services/api";
import { applyPreset, FRAME_RATES, PRESET_LABELS, RESOLUTIONS, validateStream } from "../services/presets";
import type { AppConfig, AudioChoice, IceCheckReport, InputBackendKind, LogLine, QualityPreset, SystemReport } from "../types/api";

type Tab = "stream" | "audio" | "controllers" | "network" | "host" | "diagnostics";
const TABS: { id: Tab; label: string }[] = [
  { id: "stream", label: "Stream" },
  { id: "audio", label: "Audio" },
  { id: "controllers", label: "Controllers" },
  { id: "network", label: "Network" },
  { id: "host", label: "Host" },
  { id: "diagnostics", label: "Diagnostics" },
];

export function SettingsPage() {
  const { config, saveConfig, info, notify } = useApp();
  const [tab, setTab] = useState<Tab>("stream");
  const [draft, setDraft] = useState<AppConfig | null>(config);
  const [saving, setSaving] = useState(false);
  const [report, setReport] = useState<SystemReport | null>(null);

  useEffect(() => setDraft(config), [config]);
  useEffect(() => {
    api.systemReport().then(setReport).catch(() => {});
  }, []);

  const streamError = draft ? validateStream(draft.client.stream) : null;
  const dirty = useMemo(() => JSON.stringify(draft) !== JSON.stringify(config), [draft, config]);
  if (!draft || !config) return <p className="text-sm text-muted">Loading settings…</p>;

  const set = (f: (c: AppConfig) => AppConfig) => setDraft((d) => (d ? f(structuredClone(d)) : d));

  async function save() {
    if (!draft) return;
    setSaving(true);
    try {
      await saveConfig(draft);
      notify("info", "Settings saved.");
    } catch (e) {
      notify("error", errorMessage(e));
    } finally {
      setSaving(false);
    }
  }

  const st = draft.client.stream;
  const encoders = report?.encoders.filter((e) => e.available) ?? [];
  const decoders = report?.decoders.filter((d) => d.available) ?? [];

  return (
    <div className="fade-in">
      <PageHeader
        title="Settings"
        actions={
          <>
            <Button variant="ghost" disabled={!dirty} onClick={() => setDraft(config)}>
              Revert
            </Button>
            <Button variant="primary" busy={saving} disabled={!dirty || !!streamError} onClick={save}>
              Save changes
            </Button>
          </>
        }
      />
      <div role="tablist" aria-label="Settings sections" className="mb-6 flex gap-1 border-b border-line">
        {TABS.map((t) => (
          <button
            key={t.id}
            role="tab"
            type="button"
            aria-selected={tab === t.id}
            onClick={() => setTab(t.id)}
            className={`-mb-px border-b-2 px-4 py-2.5 text-sm transition-colors ${tab === t.id ? "border-accent text-ink" : "border-transparent text-muted hover:text-ink"}`}
          >
            {t.label}
          </button>
        ))}
      </div>

      {tab === "stream" && (
        <div className="space-y-6" role="tabpanel">
          <Card title="Quality">
            <div className="mb-5 grid grid-cols-4 gap-2">
              {(Object.keys(PRESET_LABELS) as QualityPreset[]).map((p) => (
                <button
                  key={p}
                  type="button"
                  aria-pressed={st.preset === p}
                  onClick={() => set((c) => ({ ...c, client: { ...c.client, stream: applyPreset(c.client.stream, p) } }))}
                  className={`rounded-xl border px-3 py-3 text-sm transition-colors ${st.preset === p ? "border-accent bg-accent/10 text-ink" : "border-line text-muted hover:border-line-strong"}`}
                >
                  {PRESET_LABELS[p]}
                </button>
              ))}
            </div>
            <div className="grid grid-cols-2 gap-4">
              <Field label="Resolution">
                {(id) => (
                  <Select
                    id={id}
                    value={`${st.width}x${st.height}`}
                    onChange={(v) => {
                      const [w, h] = v.split("x").map(Number);
                      set((c) => ({ ...c, client: { ...c.client, stream: { ...c.client.stream, preset: "custom", width: w, height: h } } }));
                    }}
                    options={[
                      ...RESOLUTIONS.map(([w, h]) => ({ value: `${w}x${h}`, label: `${w} × ${h}` })),
                      ...(RESOLUTIONS.some(([w, h]) => w === st.width && h === st.height) ? [] : [{ value: `${st.width}x${st.height}`, label: `${st.width} × ${st.height}` }]),
                    ]}
                  />
                )}
              </Field>
              <Field label="Frame rate">
                {(id) => (
                  <Select
                    id={id}
                    value={String(st.fps)}
                    onChange={(v) => set((c) => ({ ...c, client: { ...c.client, stream: { ...c.client.stream, preset: "custom", fps: Number(v) } } }))}
                    options={[...new Set([...FRAME_RATES, st.fps])].sort((a, b) => a - b).map((f) => ({ value: String(f), label: `${f} fps` }))}
                  />
                )}
              </Field>
              <Field label="Maximum bitrate (kbps)">
                {(id) => <NumberInput id={id} value={st.max_bitrate_kbps} min={250} step={500} onChange={(v) => set((c) => ({ ...c, client: { ...c.client, stream: { ...c.client.stream, preset: "custom", max_bitrate_kbps: v } } }))} />}
              </Field>
              <Field label="Minimum bitrate (kbps)" hint="Lower bound for adaptive bitrate.">
                {(id) => <NumberInput id={id} value={st.min_bitrate_kbps} min={100} step={250} onChange={(v) => set((c) => ({ ...c, client: { ...c.client, stream: { ...c.client.stream, preset: "custom", min_bitrate_kbps: v } } }))} />}
              </Field>
              <Field label="Encoder quality">
                {(id) => (
                  <Select
                    id={id}
                    value={st.encoder_quality}
                    onChange={(v) => set((c) => ({ ...c, client: { ...c.client, stream: { ...c.client.stream, preset: "custom", encoder_quality: v } } }))}
                    options={[
                      { value: "fastest", label: "Fastest (lowest latency)" },
                      { value: "balanced", label: "Balanced" },
                      { value: "quality", label: "Quality" },
                    ]}
                  />
                )}
              </Field>
              <Field label="Codec">{(id) => <Select id={id} value="h264" onChange={() => {}} options={[{ value: "h264", label: "H.264" }]} />}</Field>
            </div>
            {streamError && <p role="alert" className="mt-3 text-sm text-bad">{streamError}</p>}
            <div className="mt-4">
              <Toggle label="Adaptive bitrate" description="Lower the bitrate automatically on packet loss or congestion." checked={st.adaptive_bitrate} onChange={(v) => set((c) => ({ ...c, client: { ...c.client, stream: { ...c.client.stream, adaptive_bitrate: v } } }))} />
            </div>
          </Card>
          <Card title="Playback">
            <div className="grid grid-cols-2 gap-4">
              <Field label="Video decoder" hint="Automatic picks hardware decoding when it works.">
                {(id) => (
                  <Select
                    id={id}
                    value={draft.client.decoder ?? ""}
                    onChange={(v) => set((c) => ({ ...c, client: { ...c.client, decoder: v || null } }))}
                    options={[{ value: "", label: "Automatic" }, ...decoders.map((d) => ({ value: d.element, label: `${d.label}${d.hardware ? " (hardware)" : ""}` }))]}
                  />
                )}
              </Field>
              <Field label="Jitter buffer (ms)" hint="Lower is snappier; higher tolerates network jitter.">
                {(id) => <NumberInput id={id} value={draft.client.jitter_buffer_ms} min={0} max={500} step={10} onChange={(v) => set((c) => ({ ...c, client: { ...c.client, jitter_buffer_ms: v } }))} />}
              </Field>
            </div>
            <div className="mt-2">
              <Toggle label="Start in fullscreen" checked={draft.client.fullscreen} onChange={(v) => set((c) => ({ ...c, client: { ...c.client, fullscreen: v } }))} />
              <Toggle label="Capture the mouse (relative motion for games)" checked={draft.client.capture_mouse} onChange={(v) => set((c) => ({ ...c, client: { ...c.client, capture_mouse: v } }))} />
              <Toggle label="Show statistics overlay in the stream window (costs a little latency while visible)" checked={draft.client.show_stats_overlay} onChange={(v) => set((c) => ({ ...c, client: { ...c.client, show_stats_overlay: v } }))} />
            </div>
          </Card>
        </div>
      )}

      {tab === "audio" && (
        <div className="space-y-6" role="tabpanel">
          <Card title="Streamed audio">
            <Toggle label="Stream host audio" checked={st.audio.enabled} onChange={(v) => set((c) => ({ ...c, client: { ...c.client, stream: { ...c.client.stream, audio: { ...c.client.stream.audio, enabled: v } } } }))} />
            <div className="mt-3 grid grid-cols-2 gap-4">
              <Field label="Opus bitrate (kbps)">
                {(id) => <NumberInput id={id} value={st.audio.bitrate_kbps} min={16} max={510} step={16} onChange={(v) => set((c) => ({ ...c, client: { ...c.client, stream: { ...c.client.stream, audio: { ...c.client.stream.audio, bitrate_kbps: v } } } }))} />}
              </Field>
              <Field label="Channels">
                {(id) => (
                  <Select id={id} value={String(st.audio.channels)} onChange={(v) => set((c) => ({ ...c, client: { ...c.client, stream: { ...c.client.stream, audio: { ...c.client.stream.audio, channels: Number(v) } } } }))} options={[{ value: "2", label: "Stereo" }, { value: "1", label: "Mono" }]} />
                )}
              </Field>
            </div>
            <div className="mt-4">
              <Field label="Default volume">{(id) => <Slider id={id} value={draft.client.volume} min={0} max={2} step={0.05} format={(v) => `${Math.round(v * 100)}%`} onChange={(v) => set((c) => ({ ...c, client: { ...c.client, volume: v } }))} />}</Field>
              <Toggle label="Start muted" checked={draft.client.muted} onChange={(v) => set((c) => ({ ...c, client: { ...c.client, muted: v } }))} />
            </div>
          </Card>
          <Card title="When this PC is the host">
            <Field label="System audio source" hint="Captures what this PC plays (loopback). The microphone is never captured.">
              {(id) => (
                <Select
                  id={id}
                  value={typeof draft.host.audio === "string" ? draft.host.audio : `device:${draft.host.audio.device}`}
                  onChange={(v) => set((c) => ({ ...c, host: { ...c.host, audio: (v.startsWith("device:") ? { device: v.slice(7) } : v) as AudioChoice } }))}
                  options={[
                    { value: "default", label: "Default output device" },
                    ...("Ok" in (report?.audio_devices ?? { Err: "" })
                      ? (report!.audio_devices as { Ok: { id: string | null; name: string }[] }).Ok.filter((d) => d.id).map((d) => ({ value: `device:${d.id}`, label: d.name }))
                      : []),
                    { value: "off", label: "Off" },
                  ]}
                />
              )}
            </Field>
            {report && "Err" in report.audio_devices && <p className="mt-2 text-xs text-warn">Audio devices: {report.audio_devices.Err}</p>}
          </Card>
        </div>
      )}

      {tab === "controllers" && (
        <div role="tabpanel">
          <ControllerEditor
            enabled={draft.client.controllers_enabled}
            mapping={draft.client.controller_mapping}
            onEnabled={(v) => set((c) => ({ ...c, client: { ...c.client, controllers_enabled: v } }))}
            onChange={(m) => set((c) => ({ ...c, client: { ...c.client, controller_mapping: m } }))}
          />
        </div>
      )}

      {tab === "network" && <NetworkTab draft={draft} set={set} hasToken={info?.has_access_token ?? false} />}

      {tab === "host" && (
        <div className="space-y-6" role="tabpanel">
          <Card title="Hosting">
            <Field label="Device name" hint="Shown to devices you pair with.">
              {(id) => <TextInput id={id} value={draft.device_name} maxLength={64} onChange={(e) => set((c) => ({ ...c, device_name: e.target.value }))} />}
            </Field>
            <div className="mt-4 grid grid-cols-2 gap-4">
              <Field label="Video encoder" hint="Automatic tries hardware encoders first, then software.">
                {(id) => (
                  <Select
                    id={id}
                    value={draft.host.encoder ?? ""}
                    onChange={(v) => set((c) => ({ ...c, host: { ...c.host, encoder: v || null } }))}
                    options={[{ value: "", label: "Automatic" }, ...encoders.map((e) => ({ value: e.element, label: `${e.label}${e.hardware ? " (hardware)" : ""}` }))]}
                  />
                )}
              </Field>
              <Field label="Input injection">
                {(id) => (
                  <Select<InputBackendKind>
                    id={id}
                    value={draft.host.input_backend}
                    onChange={(v) => set((c) => ({ ...c, host: { ...c.host, input_backend: v } }))}
                    options={[
                      { value: "auto", label: "Automatic" },
                      { value: "x11_xtest", label: "X11 XTEST (Linux/X11)" },
                      { value: "uinput", label: "uinput (Linux, needs permission)" },
                      { value: "win32_send_input", label: "SendInput (Windows)" },
                      { value: "mock", label: "Test mode (no injection)" },
                    ]}
                  />
                )}
              </Field>
            </div>
            <div className="mt-3">
              <Toggle label="Allow remote input when a session starts" checked={draft.host.input_allowed} onChange={(v) => set((c) => ({ ...c, host: { ...c.host, input_allowed: v } }))} />
              <Toggle
                label="Create virtual controllers"
                description="Requires uinput access on Linux or the ViGEmBus driver on Windows."
                checked={draft.host.controllers === "platform"}
                onChange={(v) => set((c) => ({ ...c, host: { ...c.host, controllers: v ? "platform" : "off" } }))}
              />
              <Toggle
                label="Approve sessions from paired devices automatically"
                description="Off by default. New pairings always need your approval."
                checked={draft.host.trust_paired_sessions}
                onChange={(v) => set((c) => ({ ...c, host: { ...c.host, trust_paired_sessions: v } }))}
              />
            </div>
          </Card>
        </div>
      )}

      {tab === "diagnostics" && <DiagnosticsTab report={report} refresh={() => api.systemReport().then(setReport)} />}
    </div>
  );
}

function NetworkTab({ draft, set, hasToken }: { draft: AppConfig; set: (f: (c: AppConfig) => AppConfig) => void; hasToken: boolean }) {
  const { notify } = useApp();
  const [token, setToken] = useState("");
  const [newUrl, setNewUrl] = useState("");
  const [newUser, setNewUser] = useState("");
  const [newCred, setNewCred] = useState("");
  const [check, setCheck] = useState<IceCheckReport | null>(null);
  const [checking, setChecking] = useState(false);
  const urlOk = /^(stun|stuns|turn|turns):/.test(newUrl);

  return (
    <div className="space-y-6" role="tabpanel">
      <Card title="Signaling server">
        <div className="grid grid-cols-2 gap-4">
          <Field label="Server URL" hint="wss:// is required except for a server on this computer.">
            {(id) => <TextInput id={id} value={draft.network.signaling_url} spellCheck={false} onChange={(e) => set((c) => ({ ...c, network: { ...c.network, signaling_url: e.target.value } }))} />}
          </Field>
          <Field label="Extra CA certificate (PEM path)" hint="Only for self-signed/self-hosted servers.">
            {(id) => <TextInput id={id} value={draft.network.extra_ca_pem ?? ""} spellCheck={false} onChange={(e) => set((c) => ({ ...c, network: { ...c.network, extra_ca_pem: e.target.value || null } }))} />}
          </Field>
          <Field label="Access token" hint={hasToken ? "A token is stored. Enter a new one to replace it." : "Only needed for private servers."}>
            {(id) => (
              <div className="flex gap-2">
                <TextInput id={id} type="password" value={token} autoComplete="off" onChange={(e) => setToken(e.target.value)} />
                <Button onClick={() => api.setAccessToken(token || null).then(() => { setToken(""); notify("info", token ? "Access token stored." : "Access token cleared."); }).catch((e) => notify("error", errorMessage(e)))}>
                  {token ? "Store" : "Clear"}
                </Button>
              </div>
            )}
          </Field>
        </div>
        <div className="mt-3">
          <Toggle label="Advertise on the local network (mDNS)" checked={draft.network.lan_discovery} onChange={(v) => set((c) => ({ ...c, network: { ...c.network, lan_discovery: v } }))} />
        </div>
      </Card>

      <Card
        title="STUN / TURN"
        actions={
          <Button
            size="sm"
            icon="signal"
            busy={checking}
            onClick={async () => {
              setChecking(true);
              try {
                setCheck(await api.iceCheck());
              } catch (e) {
                notify("error", errorMessage(e));
              } finally {
                setChecking(false);
              }
            }}
          >
            Test connectivity
          </Button>
        }
      >
        <p className="mb-4 text-sm text-muted">
          The signaling server provides STUN and short-lived TURN credentials. Add servers here only if you run your own. A relay adds latency and uses the relay's bandwidth.
        </p>
        {check && (
          <div className="mb-4">
            <Banner tone={check.verdict.includes("WORKS") ? "info" : check.verdict.includes("NOT") ? "error" : "warn"} title={check.verdict}>
              host {check.result.host_candidates} · STUN {check.result.srflx_candidates} · relay {check.result.relay_candidates}
              {check.result.relay_transports.length > 0 && ` (${check.result.relay_transports.join(", ")})`} · {check.result.elapsed_ms} ms
            </Banner>
          </div>
        )}
        <ul className="mb-4 space-y-2">
          {draft.network.ice_servers.map((s, i) => (
            <li key={i} className="flex items-center gap-3 rounded-lg border border-line px-3 py-2 text-sm">
              <span className="flex-1 font-mono text-xs">{s.urls.join(", ")}</span>
              {s.username && <Badge>credentials</Badge>}
              <Button size="sm" variant="ghost" icon="x" aria-label="Remove server" onClick={() => set((c) => ({ ...c, network: { ...c.network, ice_servers: c.network.ice_servers.filter((_, j) => j !== i) } }))} />
            </li>
          ))}
        </ul>
        <div className="grid grid-cols-[2fr_1fr_1fr_auto] items-end gap-2">
          <Field label="URL">{(id) => <TextInput id={id} placeholder="turn:turn.example.org:3478?transport=udp" value={newUrl} onChange={(e) => setNewUrl(e.target.value)} />}</Field>
          <Field label="Username">{(id) => <TextInput id={id} value={newUser} onChange={(e) => setNewUser(e.target.value)} />}</Field>
          <Field label="Credential">{(id) => <TextInput id={id} type="password" value={newCred} onChange={(e) => setNewCred(e.target.value)} />}</Field>
          <Button
            icon="plus"
            disabled={!urlOk}
            onClick={() => {
              set((c) => ({ ...c, network: { ...c.network, ice_servers: [...c.network.ice_servers, { urls: [newUrl], username: newUser || null, credential: newCred || null }] } }));
              setNewUrl("");
              setNewUser("");
              setNewCred("");
            }}
          >
            Add
          </Button>
        </div>
        <div className="mt-4">
          <Toggle label="Always use the relay (TURN only)" description="Hides your IP address from the peer; adds latency." checked={draft.network.relay_only} onChange={(v) => set((c) => ({ ...c, network: { ...c.network, relay_only: v } }))} />
        </div>
      </Card>
    </div>
  );
}

function DiagnosticsTab({ report, refresh }: { report: SystemReport | null; refresh: () => void }) {
  const [logs, setLogs] = useState<LogLine[]>([]);
  useEffect(() => {
    api.logs().then(setLogs);
    const t = setInterval(() => api.logs().then(setLogs), 3000);
    return () => clearInterval(t);
  }, []);
  const ok = (r: { Ok: null } | { Err: string }) => ("Ok" in r ? <Badge tone="ok">available</Badge> : <Badge tone="bad">{r.Err}</Badge>);

  return (
    <div className="space-y-6" role="tabpanel">
      <Card title="This computer" actions={<Button size="sm" variant="ghost" icon="refresh" onClick={refresh}>Re-check</Button>}>
        {!report ? (
          <p className="text-sm text-muted">Collecting…</p>
        ) : (
          <div className="space-y-4 text-sm">
            <div className="text-muted">
              {report.os} {report.arch} {report.session_type ? `· ${report.session_type}` : ""} · {report.gstreamer} · Mist {report.app_version}
            </div>
            <div>
              <div className="mb-2 font-medium">Video encoders</div>
              <div className="flex flex-wrap gap-2">
                {report.encoders.filter((e) => e.present).map((e) => (
                  <Badge key={e.element} tone={e.available ? "ok" : "bad"}>
                    {e.label} {e.available ? "" : `— ${e.error}`}
                  </Badge>
                ))}
              </div>
            </div>
            <div>
              <div className="mb-2 font-medium">Video decoders</div>
              <div className="flex flex-wrap gap-2">
                {report.decoders.filter((e) => e.present).map((e) => (
                  <Badge key={e.element} tone={e.available ? "ok" : "bad"}>
                    {e.label}
                  </Badge>
                ))}
              </div>
            </div>
            <div>
              <div className="mb-2 font-medium">Screen capture</div>
              {report.capture.map((c) => (
                <div key={c.backend} className="text-muted">
                  {c.backend}: {c.error ?? c.displays.map((d) => `${d.name} ${d.width}×${d.height}`).join(", ")}
                </div>
              ))}
            </div>
            <div>
              <div className="mb-2 font-medium">Input injection</div>
              {report.keyboard_mouse_injection.map(([name, r]) => (
                <div key={name} className="flex items-center gap-2">
                  <span className="w-36 text-muted">{name}</span>
                  {ok(r)}
                </div>
              ))}
              <div className="mt-1 flex items-center gap-2">
                <span className="w-36 text-muted">virtual controllers</span>
                {ok(report.virtual_controllers)}
              </div>
            </div>
            <div className="text-xs text-faint">Config: {report.config_dir}</div>
          </div>
        )}
      </Card>
      <Card title="Recent logs">
        <pre className="max-h-80 overflow-auto rounded-lg bg-bg p-3 font-mono text-[11px] leading-relaxed text-muted select-text">
          {logs
            .slice(-300)
            .map((l) => `${new Date(l.unix_ms).toLocaleTimeString()} ${l.level.padEnd(5)} ${l.message}`)
            .join("\n") || "No log lines yet."}
        </pre>
      </Card>
    </div>
  );
}
