// Four tabs, one question each: playing, sharing, updates, troubleshooting.
// Each tab shows the few things people change, with its expert options
// folded underneath. Changes save by themselves.
import { useEffect, useRef, useState, type ReactNode } from "react";
import { Icon } from "../components/Icon";
import type { IconName } from "../components/Icon";
import { Badge, Banner, Button, Disclosure, Field, NumberInput, PageHeader, Segmented, Select, Slider, Spinner, TextInput, Toggle } from "../components/ui";
import { ControllerEditor } from "../components/ControllerEditor";
import { useApp } from "../hooks/useAppState";
import { api, errorMessage } from "../services/api";
import { fmtKbps } from "../services/format";
import { applyPreset, FRAME_RATES, PRESET_LABELS, RESOLUTIONS, validateStream } from "../services/presets";
import type { AppConfig, AudioApp, AudioChoice, CaptureBackendInfo, IceCheckReport, InputBackendKind, LogLine, QualityPreset, Renderer, SystemReport } from "../types/api";

export type SettingsTab = "stream" | "host" | "updates" | "troubleshoot";
const TABS: { id: SettingsTab; label: string; icon: IconName }[] = [
  { id: "stream", label: "Playing", icon: "play" },
  { id: "host", label: "Sharing this PC", icon: "host" },
  { id: "updates", label: "Updates", icon: "download" },
  { id: "troubleshoot", label: "Troubleshooting", icon: "sliders" },
];

const PRESET_INFO: Record<QualityPreset, { icon: IconName; text: string }> = {
  low_latency: { icon: "zap", text: "Quickest response. Best for most games." },
  balanced: { icon: "gauge", text: "A cleaner picture, a little more delay." },
  high_quality: { icon: "sparkle", text: "Sharpest picture. Needs fast internet." },
  low_bandwidth: { icon: "signal", text: "For slow Wi-Fi or mobile data." },
  custom: { icon: "sliders", text: "Your own resolution, frame rate and bitrate." },
};
// "Custom" is not offered as a card: editing a value under More options switches to it.
const PRESET_ORDER: QualityPreset[] = ["low_latency", "balanced", "high_quality", "low_bandwidth"];

/** Wait after the last change before saving, so typing doesn't save every key. */
const SAVE_DELAY_MS = 600;

type Set = (f: (c: AppConfig) => AppConfig) => void;

export function SettingsPage({ initialTab = "stream" }: { initialTab?: SettingsTab }) {
  const { config, saveConfig, info, notify } = useApp();
  const [tab, setTab] = useState<SettingsTab>(initialTab);
  useEffect(() => setTab(initialTab), [initialTab]);
  const [draft, setDraft] = useState<AppConfig | null>(config);
  const [saveState, setSaveState] = useState<"idle" | "saving" | "saved">("idle");
  const [report, setReport] = useState<SystemReport | null>(null);
  /** The last config this page saved, to tell our own saves from outside changes. */
  const saved = useRef<string | null>(config ? JSON.stringify(config) : null);
  const pending = useRef<AppConfig | null>(null);

  // Take outside changes (e.g. the name set during host setup), not our own echo.
  useEffect(() => {
    if (!config) return;
    const json = JSON.stringify(config);
    if (json !== saved.current) {
      saved.current = json;
      setDraft(config);
    }
  }, [config]);

  useEffect(() => {
    api.systemReport().then(setReport).catch(() => {});
  }, []);

  const streamError = draft ? validateStream(draft.client.stream) : null;

  // Save shortly after each change, unless a value is invalid.
  useEffect(() => {
    if (!draft || streamError) return;
    const json = JSON.stringify(draft);
    if (json === saved.current) return;
    pending.current = draft;
    const t = setTimeout(async () => {
      pending.current = null;
      setSaveState("saving");
      try {
        saved.current = json;
        await saveConfig(draft);
        setSaveState("saved");
      } catch (e) {
        saved.current = null;
        setSaveState("idle");
        notify("error", errorMessage(e));
      }
    }, SAVE_DELAY_MS);
    return () => clearTimeout(t);
  }, [draft, streamError, saveConfig, notify]);

  // Leaving the page right after a change still saves it.
  useEffect(
    () => () => {
      if (pending.current) saveConfig(pending.current).catch((e) => notify("error", errorMessage(e)));
    },
    [saveConfig, notify],
  );

  if (!draft) return <p className="text-sm text-muted">Loading settings…</p>;

  const set: Set = (f) => setDraft((d) => (d ? f(structuredClone(d)) : d));
  const st = draft.client.stream;
  const encoders = report?.encoders.filter((e) => e.available) ?? [];
  const decoders = report?.decoders.filter((d) => d.available) ?? [];

  return (
    <div className="fade-in mx-auto max-w-3xl space-y-6 pb-10">
      <PageHeader
        title="Settings"
        subtitle="The defaults work for most people. Changes save by themselves."
        actions={
          <span className="inline-flex items-center gap-2 text-sm text-muted" aria-live="polite">
            {streamError ? (
              <span className="text-bad">Not saved: {streamError}</span>
            ) : saveState === "saving" ? (
              <>
                <Spinner size={13} /> Saving…
              </>
            ) : saveState === "saved" ? (
              <>
                <Icon name="check" size={15} className="text-ok" /> Saved
              </>
            ) : null}
          </span>
        }
      />
      <Segmented label="Settings sections" items={TABS} value={tab} onChange={setTab} />

      {tab === "stream" && (
        <div className="space-y-3" role="tabpanel">
        <Section id="settings-stream" icon="play" title="When you play another PC">
          <div className="mb-1 text-sm font-medium text-ink">Picture quality</div>
          <p className="mb-3 text-xs text-muted">Choose what suits your connection. You can switch any time.</p>
          <div className="grid grid-cols-2 gap-2.5 md:grid-cols-4">
            {PRESET_ORDER.map((p) => {
              const active = st.preset === p;
              const sample = applyPreset(st, p);
              return (
                <button
                  key={p}
                  type="button"
                  aria-pressed={active}
                  onClick={() => set((c) => ({ ...c, client: { ...c.client, stream: applyPreset(c.client.stream, p) } }))}
                  className={`relative flex flex-col items-start rounded-[15px] border p-3.5 text-left transition-colors ${active ? "border-accent/70 bg-accent/[0.08] shadow-[0_0_0_3px_rgb(60_203_180/0.08)]" : "border-line bg-sunken/50 hover:border-line-strong"}`}
                >
                  <span className={`mb-2.5 flex h-8 w-8 items-center justify-center rounded-[10px] ${active ? "bg-accent text-accent-ink" : "bg-raised text-muted"}`}>
                    <Icon name={PRESET_INFO[p].icon} size={16} />
                  </span>
                  <span className="text-sm font-semibold text-ink">{PRESET_LABELS[p]}</span>
                  <span className="mt-1 text-xs leading-snug text-muted">{PRESET_INFO[p].text}</span>
                  <span className="num mt-2.5 font-mono text-[11px] text-faint">
                    {sample.height}p · {sample.fps} fps
                  </span>
                </button>
              );
            })}
          </div>
          {st.preset === "custom" && (
            <p className="mt-2 text-xs text-muted">
              <Badge tone="accent">Custom</Badge> You've set your own values under More picture and sound options. Pick a card to go back to a preset.
            </p>
          )}
          <div className="mt-4 border-t border-line pt-1">
            <Toggle label="Open the game full screen" checked={draft.client.fullscreen} onChange={(v) => set((c) => ({ ...c, client: { ...c.client, fullscreen: v } }))} />
            <Toggle label="Lock the mouse to the game" description="Best for games where the mouse turns the camera." checked={draft.client.capture_mouse} onChange={(v) => set((c) => ({ ...c, client: { ...c.client, capture_mouse: v } }))} />
            <Toggle label="Use my controllers" description="Up to four, plugged in or wireless." checked={draft.client.controllers_enabled} onChange={(v) => set((c) => ({ ...c, client: { ...c.client, controllers_enabled: v } }))} />
            <Toggle
              label="Play the host's sound"
              checked={st.audio.enabled}
              onChange={(v) => set((c) => ({ ...c, client: { ...c.client, stream: { ...c.client.stream, audio: { ...c.client.stream.audio, enabled: v } } } }))}
            />
            {st.audio.enabled && (
              <div className="flex items-center gap-4 py-2.5">
                <span className="w-24 shrink-0 text-sm text-ink">Volume</span>
                <div className="flex-1">
                  <Slider label="Volume" value={draft.client.volume} min={0} max={2} step={0.05} format={(v) => `${Math.round(v * 100)}%`} onChange={(v) => set((c) => ({ ...c, client: { ...c.client, volume: v } }))} />
                </div>
              </div>
            )}
          </div>
        </Section>

        <Disclosure title="More picture and sound options" subtitle="Exact resolution, frame rate, bitrate, decoder and audio quality.">
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
            <Field label="Maximum bitrate (kbps)" hint={fmtKbps(st.max_bitrate_kbps)}>
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
            <Field label="Renderer" hint="Linux: GPU scales and converts video on the graphics card (used when it measures fast). Windows always uses software.">
              {(id) => (
                <Select<Renderer>
                  id={id}
                  value={draft.client.renderer ?? "auto"}
                  onChange={(v) => set((c) => ({ ...c, client: { ...c.client, renderer: v } }))}
                  options={[
                    { value: "auto", label: "Automatic" },
                    { value: "gpu", label: "GPU (always)" },
                    { value: "software", label: "Software" },
                  ]}
                />
              )}
            </Field>
            <Field label="Jitter buffer (ms)" hint="Longest wait for a lost packet to be resent. Adds no delay while nothing is lost.">
              {(id) => <NumberInput id={id} value={draft.client.jitter_buffer_ms} min={0} max={500} step={10} onChange={(v) => set((c) => ({ ...c, client: { ...c.client, jitter_buffer_ms: v } }))} />}
            </Field>
            <Field label="Audio bitrate (kbps)">
              {(id) => <NumberInput id={id} value={st.audio.bitrate_kbps} min={16} max={510} step={16} onChange={(v) => set((c) => ({ ...c, client: { ...c.client, stream: { ...c.client.stream, audio: { ...c.client.stream.audio, bitrate_kbps: v } } } }))} />}
            </Field>
            <Field label="Audio channels">
              {(id) => (
                <Select id={id} value={String(st.audio.channels)} onChange={(v) => set((c) => ({ ...c, client: { ...c.client, stream: { ...c.client.stream, audio: { ...c.client.stream.audio, channels: Number(v) } } } }))} options={[{ value: "2", label: "Stereo" }, { value: "1", label: "Mono" }]} />
              )}
            </Field>
          </div>
          {streamError && <p role="alert" className="mt-3 text-sm text-bad">{streamError}</p>}
          <div className="mt-3">
            <Toggle label="Adaptive bitrate" description="Lower the bitrate automatically on packet loss or congestion." checked={st.adaptive_bitrate} onChange={(v) => set((c) => ({ ...c, client: { ...c.client, stream: { ...c.client.stream, adaptive_bitrate: v } } }))} />
            <Toggle label="Start muted" checked={draft.client.muted} onChange={(v) => set((c) => ({ ...c, client: { ...c.client, muted: v } }))} />
            <Toggle label="Show statistics in the game window" description="Costs a little latency while visible." checked={draft.client.show_stats_overlay} onChange={(v) => set((c) => ({ ...c, client: { ...c.client, show_stats_overlay: v } }))} />
          </div>
        </Disclosure>

        <Disclosure title="Controller tuning" subtitle="Stick deadzones, inverted sticks and button remapping.">
          <ControllerEditor mapping={draft.client.controller_mapping} onChange={(m) => set((c) => ({ ...c, client: { ...c.client, controller_mapping: m } }))} />
        </Disclosure>

        </div>
      )}

      {tab === "host" && (
        <div className="space-y-3" role="tabpanel">
        <Section id="settings-host" icon="host" title="When others play this PC">
          <Field label="This PC's name" hint="What others see when they add it.">
            {(id) => <TextInput id={id} value={draft.device_name} maxLength={64} onChange={(e) => set((c) => ({ ...c, device_name: e.target.value }))} />}
          </Field>
          <div className="mt-4 grid gap-4 md:grid-cols-2">
            <DisplayPicker value={draft.host.display_id} onChange={(v) => set((c) => ({ ...c, host: { ...c.host, display_id: v } }))} />
            <Field label="Sound to share" hint="The microphone is never shared.">
              {(id) => (
                <Select
                  id={id}
                  value={audioSourceValue(draft.host.audio)}
                  onChange={(v) => set((c) => ({ ...c, host: { ...c.host, audio: audioSourceChoice(v, c.host.audio) } }))}
                  options={[
                    { value: "default", label: "Everything this PC plays" },
                    { value: "except", label: "Everything except one app (e.g. Discord)" },
                    { value: "apps", label: "Only chosen apps" },
                    ...("Ok" in (report?.audio_devices ?? { Err: "" })
                      ? (report!.audio_devices as { Ok: { id: string | null; name: string }[] }).Ok.filter((d) => d.id).map((d) => ({ value: `device:${d.id}`, label: `Only ${d.name}` }))
                      : []),
                    { value: "off", label: "No sound" },
                  ]}
                />
              )}
            </Field>
          </div>
          {typeof draft.host.audio === "object" && ("except" in draft.host.audio || "apps" in draft.host.audio) && (
            <AppAudioPicker audio={draft.host.audio} onChange={(audio) => set((c) => ({ ...c, host: { ...c.host, audio } }))} />
          )}
          <div className="mt-4 border-t border-line pt-1">
            <Toggle
              label="Let people control this PC"
              description="Keyboard, mouse and controllers. Turn off and people can only watch."
              checked={draft.host.input_allowed}
              onChange={(v) => set((c) => ({ ...c, host: { ...c.host, input_allowed: v } }))}
            />
            <Toggle
              label="Let friends' controllers work in games"
              description="Needs the free ViGEmBus driver on Windows."
              checked={draft.host.controllers === "platform"}
              onChange={(v) => set((c) => ({ ...c, host: { ...c.host, controllers: v ? "platform" : "off" } }))}
            />
            <Toggle
              label="Let paired devices join without asking"
              description="Adding a new device always needs your approval."
              checked={draft.host.trust_paired_sessions}
              onChange={(v) => set((c) => ({ ...c, host: { ...c.host, trust_paired_sessions: v } }))}
            />
            <Toggle
              label="Show me who's connected"
              description="A small notice on this PC's screen. Only you see it."
              checked={draft.host.connection_overlay ?? true}
              onChange={(v) => set((c) => ({ ...c, host: { ...c.host, connection_overlay: v } }))}
            />
            <Toggle
              label="Fit the picture to the player's screen"
              description="While someone plays, this PC's display switches to their screen's size, so it fills their monitor. Changed back when they leave."
              checked={draft.host.match_client_display ?? true}
              onChange={(v) => set((c) => ({ ...c, host: { ...c.host, match_client_display: v } }))}
            />
          </div>
        </Section>

        <Disclosure title="More sharing options" subtitle="Encoder, how many people, limits, and how other PCs reach this one.">
          <div className="grid grid-cols-2 gap-4">
            <Field label="Video encoder" hint="Automatic uses Media Foundation (works on every GPU), then other hardware encoders, then software. NVENC can be faster on NVIDIA cards; after switching to it from Media Foundation, restart Mist.">
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
            <Field label="People at once" hint="1–8. Each person needs their own share of your upload speed. Takes effect when sharing starts.">
              {(id) => <NumberInput id={id} value={draft.host.max_clients ?? 8} min={1} max={8} onChange={(v) => set((c) => ({ ...c, host: { ...c.host, max_clients: v } }))} />}
            </Field>
          </div>

          <div className="mt-5 border-t border-line pt-4">
            <div className="mb-3 text-[13px] font-medium text-muted">Limits for what others may ask for</div>
            <div className="grid grid-cols-2 gap-4">
              <Field label="Max width">{(id) => <NumberInput id={id} value={draft.host.max_width} min={320} max={7680} step={2} onChange={(v) => set((c) => ({ ...c, host: { ...c.host, max_width: v } }))} />}</Field>
              <Field label="Max height">{(id) => <NumberInput id={id} value={draft.host.max_height} min={240} max={4320} step={2} onChange={(v) => set((c) => ({ ...c, host: { ...c.host, max_height: v } }))} />}</Field>
              <Field label="Max frame rate">{(id) => <NumberInput id={id} value={draft.host.max_fps} min={10} max={240} onChange={(v) => set((c) => ({ ...c, host: { ...c.host, max_fps: v } }))} />}</Field>
              <Field label="Max bitrate (kbps)">{(id) => <NumberInput id={id} value={draft.host.max_bitrate_kbps} min={250} max={500000} step={500} onChange={(v) => set((c) => ({ ...c, host: { ...c.host, max_bitrate_kbps: v } }))} />}</Field>
            </div>
          </div>

          <div className="mt-5 border-t border-line pt-2">
            <Toggle
              label="Run Mist's server on this PC"
              description="Other PCs connect to this one directly. Turn off only to use your own server (Troubleshooting → Network)."
              checked={draft.host.builtin_server ?? true}
              onChange={(v) => set((c) => ({ ...c, host: { ...c.host, builtin_server: v } }))}
            />
            {(draft.host.builtin_server ?? true) && (
              <>
                <Toggle
                  label="Open the port on my router automatically (UPnP)"
                  description="So other PCs can connect over the internet without a manual port forward. Takes effect when sharing starts."
                  checked={draft.host.upnp ?? true}
                  onChange={(v) => set((c) => ({ ...c, host: { ...c.host, upnp: v } }))}
                />
                <div className="mt-2 max-w-xs">
                  <Field label="Server port (TCP)" hint="Forward this port on your router to host over the internet. Takes effect when sharing starts.">
                    {(id) => <NumberInput id={id} value={draft.host.server_port ?? 47900} min={1024} max={65535} onChange={(v) => set((c) => ({ ...c, host: { ...c.host, server_port: v } }))} />}
                  </Field>
                </div>
              </>
            )}
          </div>
        </Disclosure>

        </div>
      )}

      {tab === "updates" && (
        <div className="space-y-3" role="tabpanel">
          <UpdatesTab draft={draft} set={set} />
        </div>
      )}

      {tab === "troubleshoot" && (
        <div className="space-y-3" role="tabpanel">
          <p className="px-1 pb-1 text-sm text-muted">Only needed when something doesn't work, or for unusual networks.</p>
        <Section id="settings-network" icon="globe" title="Network">
          <NetworkTab draft={draft} set={set} hasToken={info?.has_access_token ?? false} />
        </Section>

        <Section id="settings-diagnostics" icon="cpu" title="Diagnostics">
          <DiagnosticsTab report={report} refresh={() => api.systemReport().then(setReport)} />
        </Section>
        </div>
      )}
    </div>
  );
}

/** A top-level group of settings. */
function Section({ id, icon, title, children }: { id: string; icon: IconName; title: string; children: ReactNode }) {
  return (
    <section id={id} className="card scroll-mt-6 rounded-[var(--radius-card)] border border-line p-6">
      <h2 className="mb-5 flex items-center gap-3 text-lg font-semibold tracking-tight">
        <span className="flex h-9 w-9 items-center justify-center rounded-[11px] bg-accent/12 text-accent">
          <Icon name={icon} size={18} />
        </span>
        {title}
      </h2>
      {children}
    </section>
  );
}

/** A heading inside an Advanced section. */
function Sub({ title, actions, children }: { title: string; actions?: ReactNode; children: ReactNode }) {
  return (
    <div className="border-t border-line pt-4 first:border-t-0 first:pt-0">
      <div className="mb-3 flex items-center justify-between gap-3">
        <h3 className="text-sm font-semibold">{title}</h3>
        {actions}
      </div>
      {children}
    </div>
  );
}

/** Which screen the host streams. */
function DisplayPicker({ value, onChange }: { value: string | null; onChange: (v: string | null) => void }) {
  const [displays, setDisplays] = useState<CaptureBackendInfo[] | null>(null);
  useEffect(() => {
    api.listDisplays().then(setDisplays).catch(() => setDisplays([]));
  }, []);
  const all = (displays ?? []).flatMap((b) => b.displays);
  if (displays === null) return <p className="text-sm text-muted">Finding screens…</p>;
  if (all.length === 0) {
    return (
      <Banner tone="error" title="No screen can be captured">
        {displays.filter((b) => b.error).map((b) => `${b.backend}: ${b.error}`).join("; ") || "No display found."}
      </Banner>
    );
  }
  return (
    <Field label="Screen to share">
      {(id) => (
        <Select
          id={id}
          value={value ?? ""}
          onChange={(v) => onChange(v || null)}
          options={[{ value: "", label: "Main screen" }, ...all.map((d) => ({ value: d.id, label: `${d.name} — ${d.width}×${d.height}${d.primary ? " (main)" : ""}` }))]}
        />
      )}
    </Field>
  );
}

function NetworkTab({ draft, set, hasToken }: { draft: AppConfig; set: Set; hasToken: boolean }) {
  const { notify } = useApp();
  const [token, setToken] = useState("");
  const [newUrl, setNewUrl] = useState("");
  const [newUser, setNewUser] = useState("");
  const [newCred, setNewCred] = useState("");
  const [check, setCheck] = useState<IceCheckReport | null>(null);
  const [checking, setChecking] = useState(false);
  const urlOk = /^(stun|stuns|turn|turns):/.test(newUrl);

  return (
    <div className="space-y-6">
      <Sub title="Signaling server">
        <div className="grid grid-cols-2 gap-4">
          <Field label="Server URL" hint="Only needed for your own server (e.g. on a VPS). Hosts normally run the server themselves.">
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
      </Sub>

      <Sub
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
      </Sub>
    </div>
  );
}

const TOKEN_URL = "https://github.com/settings/personal-access-tokens/new";
const CLASSIC_TOKEN_URL = "https://github.com/settings/tokens/new?scopes=repo&description=Mist%20Remote%20Play%20updates";

function fmtSize(bytes: number): string {
  return bytes >= 1e6 ? `${(bytes / 1e6).toFixed(0)} MB` : `${Math.max(1, Math.round(bytes / 1e3))} kB`;
}

function UpdatesTab({ draft, set }: { draft: AppConfig; set: Set }) {
  const { info, update, checkForUpdate, installUpdate, refreshInfo, notify } = useApp();
  const [token, setToken] = useState("");
  const [savingToken, setSavingToken] = useState(false);
  const busy = update.progress !== null;
  const a = update.available;
  const pct = update.progress && update.progress.total > 0 ? Math.min(100, Math.round((update.progress.done / update.progress.total) * 100)) : 0;
  const windows = info?.platform === "windows";

  async function storeToken(value: string | null) {
    setSavingToken(true);
    try {
      await api.setGithubToken(value);
      setToken("");
      await refreshInfo();
      notify("info", value ? "Token stored. Checking for updates…" : "Token removed.");
      if (value) await checkForUpdate();
    } catch (e) {
      notify("error", errorMessage(e));
    } finally {
      setSavingToken(false);
    }
  }

  return (
    <Section id="settings-updates-section" icon="download" title="Updates">
      <div className="mb-4 flex items-center justify-between gap-3">
        <span className="text-sm text-muted">You have version {info?.version ?? "…"}.</span>
        <Button size="sm" icon="refresh" busy={update.checking} disabled={busy} onClick={checkForUpdate}>
          Check now
        </Button>
      </div>
        {a ? (
          <div className="space-y-4">
            <div className="flex flex-wrap items-center gap-3">
              <span className="flex h-10 w-10 items-center justify-center rounded-[12px] bg-accent/12 text-accent">
                <Icon name="sparkle" size={20} />
              </span>
              <div className="min-w-0 flex-1">
                <div className="text-[15px] font-semibold">
                  Version {a.version} is available {a.prerelease && <Badge tone="warn">pre-release</Badge>}
                </div>
                <div className="text-xs text-muted">
                  {a.published_at ? `Published ${new Date(a.published_at).toLocaleDateString()}` : a.name}
                  {a.installer ? ` · ${fmtSize(a.installer.size)}` : ""}
                </div>
              </div>
              {windows && a.installer ? (
                <Button variant="primary" icon="download" busy={busy} onClick={installUpdate}>
                  Download and install
                </Button>
              ) : (
                <Button size="sm" icon="globe" onClick={() => api.openExternal(a.html_url).catch((e) => notify("error", errorMessage(e)))}>
                  Open release page
                </Button>
              )}
            </div>
            {update.progress && (
              <div aria-live="polite">
                <div className="mb-1.5 flex justify-between text-xs text-muted">
                  <span>{update.progress.stage === "installing" ? "Starting the installer… Mist will restart." : "Downloading and verifying…"}</span>
                  <span className="num font-mono">{update.progress.stage === "installing" ? "100 %" : `${pct} %`}</span>
                </div>
                <div className="h-2 overflow-hidden rounded-full bg-sunken" role="progressbar" aria-valuenow={pct} aria-valuemin={0} aria-valuemax={100}>
                  <div className="h-full rounded-full bg-accent transition-[width] duration-150" style={{ width: `${update.progress.stage === "installing" ? 100 : pct}%` }} />
                </div>
              </div>
            )}
            {a.notes && (
              <details className="rounded-[12px] border border-line bg-sunken/60 px-4 py-3 text-sm">
                <summary className="cursor-pointer text-muted">What's new</summary>
                <pre className="mt-3 max-h-64 overflow-auto whitespace-pre-wrap font-sans text-[13px] leading-relaxed text-muted">{a.notes}</pre>
              </details>
            )}
            <p className="text-xs text-faint">
              The installer is checked against the SHA-256 GitHub publishes for it before it runs. Windows asks for administrator approval; hosting and streaming end during the update.
            </p>
          </div>
        ) : update.checking ? (
          <p className="text-sm text-muted">Checking for updates…</p>
        ) : update.error ? (
          <Banner tone="warn" title="Could not check for updates">
            {update.error}
          </Banner>
        ) : update.checkedAt ? (
          <p className="flex items-center gap-2 text-sm text-muted">
            <Icon name="check" className="text-ok" /> That's the latest version.
          </p>
        ) : null}
        {update.error && a && (
          <div className="mt-4">
            <Banner tone="error" title="The update did not install">
              {update.error}
            </Banner>
          </div>
        )}
        <div className="mt-4 border-t border-line pt-2">
          <Toggle
            label="Check for updates when Mist starts"
            checked={draft.updates?.auto_check ?? true}
            onChange={(v) => set((c) => ({ ...c, updates: { ...(c.updates ?? { auto_check: true }), auto_check: v } }))}
          />
        </div>

      <details open={!info?.has_github_token} className="group mt-2 border-t border-line pt-4">
        <summary className="flex cursor-pointer list-none items-center gap-2 text-sm [&::-webkit-details-marker]:hidden">
          <Icon name="chevron" size={15} className="text-muted transition-transform group-open:rotate-90" />
          <span className="flex-1 font-medium">GitHub access</span>
          {info?.has_github_token ? <Badge tone="ok">set up</Badge> : <Badge tone="warn">needed for updates</Badge>}
        </summary>
        <p className="mb-3 mt-3 text-sm text-muted">Updates come from the project's private GitHub repository, so Mist needs a token to read it.</p>
        <ol className="mb-4 list-decimal space-y-1 pl-5 text-sm text-muted">
          <li>
            Open{" "}
            <button type="button" onClick={() => api.openExternal(TOKEN_URL).catch((e) => notify("error", errorMessage(e)))} className="text-accent underline-offset-2 hover:text-accent-strong hover:underline">
              GitHub → Fine-grained tokens → Generate new token
            </button>{" "}
            (signed in with an account that can see the repository).
          </li>
          <li>Repository access: <span className="text-ink">Only select repositories → MistRemotePlay</span>. Permissions: <span className="text-ink">Contents → Read-only</span>. Nothing else.</li>
          <li>
            Not your repository (you were added as a collaborator)? Fine-grained tokens can't reach it: create a{" "}
            <button type="button" onClick={() => api.openExternal(CLASSIC_TOKEN_URL).catch((e) => notify("error", errorMessage(e)))} className="text-accent underline-offset-2 hover:text-accent-strong hover:underline">
              classic token
            </button>{" "}
            with the <span className="text-ink">repo</span> scope instead.
          </li>
          <li>Copy the token and paste it here. It is stored encrypted on this PC and only sent to GitHub.</li>
        </ol>
        <div className="flex gap-2">
          <TextInput type="password" value={token} placeholder={info?.has_github_token ? "Paste a new token to replace the stored one" : "github_pat_… or ghp_…"} autoComplete="off" spellCheck={false} onChange={(e) => setToken(e.target.value)} aria-label="GitHub token" />
          <Button variant="primary" disabled={!token.trim()} busy={savingToken} onClick={() => storeToken(token.trim())}>
            Store
          </Button>
          {info?.has_github_token && (
            <Button variant="ghost" disabled={savingToken} onClick={() => storeToken(null)}>
              Remove
            </Button>
          )}
        </div>
        <p className="mt-2 text-xs text-faint">Fine-grained tokens expire (you choose when). When it does, updates show an error here; create a new one the same way.</p>
      </details>
    </Section>
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
    <div className="space-y-6">
      <Sub title="This computer" actions={<Button size="sm" variant="ghost" icon="refresh" onClick={refresh}>Re-check</Button>}>
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
      </Sub>
      <Sub title="Recent logs">
        <pre className="max-h-80 overflow-auto rounded-lg bg-bg p-3 font-mono text-[11px] leading-relaxed text-muted select-text">
          {logs
            .slice(-300)
            .map((l) => `${new Date(l.unix_ms).toLocaleTimeString()} ${l.level.padEnd(5)} ${l.message}`)
            .join("\n") || "No log lines yet."}
        </pre>
      </Sub>
    </div>
  );
}

function audioSourceValue(a: AudioChoice): string {
  if (typeof a === "string") return a;
  if ("device" in a) return `device:${a.device}`;
  return "except" in a ? "except" : "apps";
}

function audioSourceChoice(v: string, prev: AudioChoice): AudioChoice {
  if (v.startsWith("device:")) return { device: v.slice(7) };
  if (v === "except") return { except: typeof prev === "object" && "except" in prev ? prev.except : "Discord.exe" };
  if (v === "apps") return { apps: typeof prev === "object" && "apps" in prev ? prev.apps : [] };
  return v as AudioChoice;
}

/** Picks the app left out of the stream, or the apps streamed. */
function AppAudioPicker({ audio, onChange }: { audio: { except: string } | { apps: string[] }; onChange: (a: AudioChoice) => void }) {
  const [running, setRunning] = useState<AudioApp[]>([]);
  const [adding, setAdding] = useState("");
  const refresh = () => api.audioApps().then(setRunning).catch(() => setRunning([]));
  useEffect(() => {
    refresh();
  }, []);
  const suggestions = (
    <datalist id="mist-audio-apps">
      {running.map((a) => (
        <option key={a.exe} value={a.exe}>
          {a.name}
        </option>
      ))}
    </datalist>
  );
  const refreshButton = (
    <Button size="sm" variant="ghost" icon="refresh" onClick={refresh}>
      Refresh
    </Button>
  );
  if ("except" in audio) {
    return (
      <div className="mt-3">
        <Field label="App to leave out" hint="Its sound isn't streamed, so friends in a call with you (Discord, for example) don't hear themselves. Pick an app that is playing sound, or type its program name. Needs Windows 11.">
          {(id) => (
            <div className="flex items-center gap-2">
              <TextInput id={id} list="mist-audio-apps" value={audio.except} placeholder="Discord.exe" onChange={(e) => onChange({ except: e.target.value })} />
              {refreshButton}
            </div>
          )}
        </Field>
        {suggestions}
      </div>
    );
  }
  const add = (exe: string) => {
    const name = exe.trim();
    if (name && !audio.apps.some((a) => a.toLowerCase() === name.toLowerCase())) onChange({ apps: [...audio.apps, name] });
    setAdding("");
  };
  return (
    <div className="mt-3">
      <Field label="Apps to stream" hint="Only these apps' sound is streamed. Pick apps that are playing sound, or type a program name. Needs Windows 11.">
        {(id) => (
          <div className="flex items-center gap-2">
            <TextInput
              id={id}
              list="mist-audio-apps"
              value={adding}
              placeholder="game.exe"
              onChange={(e) => setAdding(e.target.value)}
              onKeyDown={(e) => e.key === "Enter" && add(adding)}
            />
            <Button size="sm" icon="plus" onClick={() => add(adding)} disabled={!adding.trim()}>
              Add
            </Button>
            {refreshButton}
          </div>
        )}
      </Field>
      {suggestions}
      <div className="mt-2 flex flex-wrap gap-2">
        {audio.apps.length === 0 && <p className="text-xs text-warn">Add at least one app.</p>}
        {audio.apps.map((a) => (
          <span key={a} className="inline-flex items-center gap-1 rounded-full border border-line px-2.5 py-0.5 text-xs">
            {a}
            <button type="button" aria-label={`Remove ${a}`} className="text-muted hover:text-ink" onClick={() => onChange({ apps: audio.apps.filter((x) => x !== a) })}>
              <Icon name="x" size={12} />
            </button>
          </span>
        ))}
      </div>
    </div>
  );
}
