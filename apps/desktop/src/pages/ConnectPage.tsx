import { useState } from "react";
import type { Page } from "../App";
import { Icon } from "../components/Icon";
import { Avatar, Banner, Button, Card, EmptyState, Field, LiveDot, PageHeader, TextInput } from "../components/ui";
import { useApp } from "../hooks/useAppState";
import { api, errorMessage } from "../services/api";
import { fmtAgo } from "../services/format";
import type { DiscoveredHost, PairedDevice } from "../types/api";

const CODE_RE = /^[0-9A-HJKMNP-TV-Z]{4}-?[0-9A-HJKMNP-TV-Z]{4}$/i;

export function ConnectPage({ go }: { go: (p: Page) => void }) {
  const { config, devices, refreshDevices, client, connect, notify } = useApp();
  const [code, setCode] = useState("");
  const [server, setServer] = useState<string>("");
  const [showServer, setShowServer] = useState(false);
  const [pairing, setPairing] = useState(false);
  const [pairError, setPairError] = useState<string | null>(null);
  const [lan, setLan] = useState<DiscoveredHost[] | null>(null);
  const [scanning, setScanning] = useState(false);
  const [connecting, setConnecting] = useState<string | null>(null);

  const serverUrl = server || config?.network.signaling_url || "";
  const codeOk = CODE_RE.test(code.trim());
  const serverHost = (() => {
    try {
      return new URL(serverUrl).host;
    } catch {
      return serverUrl;
    }
  })();

  async function pair() {
    setPairing(true);
    setPairError(null);
    try {
      const d = await api.pairWithCode(code.trim(), serverUrl);
      notify("info", `Paired with ${d.name}.`);
      setCode("");
      await refreshDevices();
    } catch (e) {
      setPairError(errorMessage(e));
    } finally {
      setPairing(false);
    }
  }

  async function scan() {
    setScanning(true);
    try {
      setLan(await api.discoverLan(3));
    } catch (e) {
      notify("error", errorMessage(e));
    } finally {
      setScanning(false);
    }
  }

  async function start(d: PairedDevice) {
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

  /** Format as the user types: uppercase, a dash after four characters. */
  function onCode(v: string) {
    const raw = v.toUpperCase().replace(/[^0-9A-Z]/g, "").slice(0, 8);
    setCode(raw.length > 4 ? `${raw.slice(0, 4)}-${raw.slice(4)}` : raw);
  }

  return (
    <div className="fade-in space-y-6">
      <PageHeader title="Connect" subtitle="Pair once with a code, then connect any time. The host must approve each session." />

      {client && (client.running || client.state === "failed") && (
        <Banner
          tone={client.state === "failed" ? "error" : "info"}
          title={`${client.hostName}: ${client.state.replace(/_/g, " ")}`}
          action={
            <Button size="sm" onClick={() => go("streaming")}>
              Details
            </Button>
          }
        >
          {client.detail}
        </Banner>
      )}

      <div className="grid gap-6 lg:grid-cols-[1.2fr_1fr]">
        <Card title="Pair with a code" icon="link" subtitle="Shown on the host under Host → Pairing.">
          <form
            className="space-y-5"
            onSubmit={(e) => {
              e.preventDefault();
              if (codeOk) pair();
            }}
          >
            <Field label="Pairing code" hint="8 characters. Letters are not case-sensitive." error={code.length >= 9 && !codeOk ? "That doesn't look like a pairing code. Codes look like ABCD-1234." : null}>
              {(id) => (
                <input
                  id={id}
                  value={code}
                  autoComplete="off"
                  spellCheck={false}
                  placeholder="XXXX-XXXX"
                  inputMode="text"
                  onChange={(e) => onCode(e.target.value)}
                  className={`h-16 w-full rounded-[14px] border bg-sunken px-4 text-center font-mono text-[28px] font-semibold uppercase tracking-[0.32em] text-ink placeholder:text-line-strong focus:bg-bg focus:outline-none ${codeOk ? "border-accent/70" : "border-line focus:border-accent"}`}
                />
              )}
            </Field>

            <div className="rounded-[12px] border border-line bg-sunken/50 px-3.5 py-2.5">
              <div className="flex items-center gap-2 text-xs">
                <Icon name="globe" size={14} className="text-faint" />
                <span className="text-muted">Signaling server</span>
                <span className="min-w-0 flex-1 truncate font-mono text-ink">{serverHost || "not set"}</span>
                <button type="button" className="text-accent hover:text-accent-strong" onClick={() => setShowServer((v) => !v)} aria-expanded={showServer}>
                  {showServer ? "Done" : "Change"}
                </button>
              </div>
              {showServer && (
                <div className="mt-3">
                  <Field label="Server URL" hint="Must be the same server the host uses.">
                    {(id) => <TextInput id={id} value={serverUrl} onChange={(e) => setServer(e.target.value)} spellCheck={false} />}
                  </Field>
                </div>
              )}
            </div>

            {pairError && (
              <Banner tone="error" title="Pairing failed">
                {pairError}
              </Banner>
            )}
            <Button type="submit" variant="primary" size="lg" icon="link" busy={pairing} disabled={!codeOk} className="w-full">
              {pairing ? "Waiting for the host to approve…" : "Pair with this host"}
            </Button>
          </form>
        </Card>

        <Card
          title="On this network"
          icon="radar"
          subtitle="Hosts advertising over mDNS."
          actions={
            <Button size="sm" variant="ghost" icon="refresh" busy={scanning} onClick={scan}>
              Scan
            </Button>
          }
        >
          {lan === null ? (
            <div className="flex flex-col items-center gap-3 py-6 text-center">
              <div className="relative flex h-20 w-20 items-center justify-center" aria-hidden="true">
                <span className={`absolute inset-0 rounded-full border border-accent/20 ${scanning ? "animate-ping" : ""}`} />
                <span className="absolute inset-3 rounded-full border border-accent/30" />
                <span className="absolute inset-6 rounded-full border border-accent/45 bg-accent/10" />
                <Icon name="radar" size={18} className="relative text-accent" />
              </div>
              <p className="max-w-xs text-sm text-muted">Find hosts on your local network. You still need their pairing code the first time.</p>
              <Button size="sm" icon="radar" busy={scanning} onClick={scan}>
                Scan the network
              </Button>
            </div>
          ) : lan.length === 0 ? (
            <EmptyState icon="signal" title="No hosts found">
              Hosts appear here when hosting is on and LAN discovery is enabled. Some networks block multicast.
            </EmptyState>
          ) : (
            <ul className="space-y-2">
              {lan.map((h) => {
                const paired = devices.find((d) => d.device_id === h.device_id);
                return (
                  <li key={h.device_id} className="flex items-center gap-3 rounded-[14px] border border-line bg-sunken/60 px-3.5 py-3">
                    <Avatar name={h.name} id={h.device_id} size={32} />
                    <div className="min-w-0 flex-1">
                      <div className="truncate text-sm font-medium">{h.name}</div>
                      <div className="truncate font-mono text-[11px] text-faint">{h.addresses.slice(0, 2).join(", ")}</div>
                    </div>
                    {paired ? (
                      <Button size="sm" variant="primary" icon="play" busy={connecting === paired.device_id} disabled={client?.running} onClick={() => start(paired)}>
                        Connect
                      </Button>
                    ) : (
                      <Button
                        size="sm"
                        onClick={() => {
                          setServer(h.signaling_url);
                          setShowServer(true);
                        }}
                        title={`Use ${h.signaling_url} for pairing`}
                      >
                        Use server
                      </Button>
                    )}
                  </li>
                );
              })}
            </ul>
          )}
        </Card>
      </div>

      <Card title="Saved hosts" icon="monitor" subtitle="Hosts this device is paired with.">
        {devices.length === 0 ? (
          <EmptyState icon="link" title="No paired hosts">
            Pair with a code to save a host here.
          </EmptyState>
        ) : (
          <ul className="divide-y divide-line">
            {devices.map((d) => {
              const here = client?.running && client.deviceId === d.device_id;
              return (
                <li key={d.device_id} className="flex items-center gap-4 py-3">
                  <Avatar name={d.name} id={d.device_id} />
                  <div className="min-w-0 flex-1">
                    <div className="flex items-center gap-2 text-sm font-medium">
                      <span className="truncate">{d.name}</span>
                      {here && (
                        <span className="inline-flex items-center gap-1.5 text-xs font-normal text-ok">
                          <LiveDot tone="ok" size={6} live /> in session
                        </span>
                      )}
                    </div>
                    <div className="truncate text-xs text-faint">
                      {d.signaling_url ?? "default server"} · last used {fmtAgo(d.last_connected_ms)}
                    </div>
                  </div>
                  <Button size="sm" variant="ghost" onClick={() => api.forgetDevice(d.device_id).then(refreshDevices).catch((e) => notify("error", errorMessage(e)))}>
                    Forget
                  </Button>
                  <Button size="sm" variant="primary" icon="play" busy={connecting === d.device_id} disabled={client?.running} onClick={() => start(d)}>
                    Connect
                  </Button>
                </li>
              );
            })}
          </ul>
        )}
      </Card>
    </div>
  );
}
