import { useState } from "react";
import type { Page } from "../App";
import { Icon } from "../components/Icon";
import { Avatar, Banner, Button, Card, EmptyState, Field, LiveDot, PageHeader, TextInput } from "../components/ui";
import { useApp } from "../hooks/useAppState";
import { api, errorMessage, untrustedServer, type UntrustedServer } from "../services/api";
import { fmtAgo, fmtFingerprint } from "../services/format";
import type { DiscoveredHost, PairedDevice } from "../types/api";

const CODE_RE = /^[0-9A-HJKMNP-TV-Z]{4}-?[0-9A-HJKMNP-TV-Z]{4}$/i;
/** The configuration default, which only works with a server on this PC. */
const DEFAULT_SERVER = "ws://127.0.0.1:8443";

export function ConnectPage({ go }: { go: (p: Page) => void }) {
  const { config, devices, refreshDevices, client, connect, notify } = useApp();
  const [code, setCode] = useState("");
  const configured = config?.network.signaling_url && config.network.signaling_url !== DEFAULT_SERVER ? config.network.signaling_url : "";
  const [address, setAddress] = useState<string | null>(null);
  const [pairing, setPairing] = useState(false);
  const [pairError, setPairError] = useState<string | null>(null);
  const [untrusted, setUntrusted] = useState<UntrustedServer | null>(null);
  const [lan, setLan] = useState<DiscoveredHost[] | null>(null);
  const [scanning, setScanning] = useState(false);
  const [connecting, setConnecting] = useState<string | null>(null);

  const hostAddress = address ?? configured;
  const codeOk = CODE_RE.test(code.trim());
  const addressOk = hostAddress.trim().length > 0;

  async function pair() {
    setPairing(true);
    setPairError(null);
    setUntrusted(null);
    try {
      const d = await api.pairWithCode(code.trim(), hostAddress.trim());
      notify("info", `Paired with ${d.name}.`);
      setCode("");
      await refreshDevices();
    } catch (e) {
      const u = untrustedServer(e);
      if (u) setUntrusted(u);
      else setPairError(errorMessage(e));
    } finally {
      setPairing(false);
    }
  }

  async function trustAndPair(u: UntrustedServer) {
    try {
      await api.trustServer(u.url, u.fingerprint);
      setUntrusted(null);
      await pair();
    } catch (e) {
      setPairError(errorMessage(e));
    }
  }

  /** A host found on this network: its address and certificate are known. */
  async function useLanHost(h: DiscoveredHost) {
    try {
      if (h.fingerprint) await api.trustServer(h.signaling_url, h.fingerprint);
      setAddress(h.signaling_url);
      setPairError(null);
      setUntrusted(null);
      document.getElementById("pair-code")?.focus();
    } catch (e) {
      notify("error", errorMessage(e));
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
    setPairError(null);
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
        <Card title="Pair with a host" icon="link" subtitle="Both are shown on the host PC under Host.">
          <form
            className="space-y-5"
            onSubmit={(e) => {
              e.preventDefault();
              if (codeOk && addressOk) pair();
            }}
          >
            <Field label="Host's address" hint="On the same network: pick it under “On this network”. Over the internet: the host's public address (see Host → How others connect).">
              {(id) => (
                <TextInput
                  id={id}
                  value={hostAddress}
                  placeholder="192.168.1.40  or  myhome.example.org"
                  spellCheck={false}
                  autoComplete="off"
                  onChange={(e) => {
                    setAddress(e.target.value);
                    setPairError(null);
                    setUntrusted(null);
                  }}
                  className="font-mono"
                />
              )}
            </Field>
            <Field label="Pairing code" hint="8 characters. Letters are not case-sensitive." error={code.length >= 9 && !codeOk ? "That doesn't look like a pairing code. Codes look like ABCD-1234." : null}>
              {(id) => (
                <input
                  id="pair-code"
                  aria-describedby={id}
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

            {untrusted && (
              <div role="alert" className="fade-in rounded-[14px] border border-warn/45 bg-warn/[0.06] p-4">
                <div className="flex items-start gap-3">
                  <Icon name="shield" className="mt-0.5 shrink-0 text-warn" />
                  <div className="min-w-0 flex-1">
                    <div className="text-sm font-semibold">Check the host's fingerprint</div>
                    <p className="mt-1 text-sm text-muted">
                      This is the first connection to <span className="font-mono text-ink">{untrusted.url.replace(/^wss?:\/\//, "")}</span>. On the host PC, open
                      Host → How others connect. The fingerprint there must match this one exactly:
                    </p>
                    <div className="mt-3 grid grid-cols-4 gap-1.5 font-mono text-xs text-ink sm:grid-cols-8">
                      {fmtFingerprint(untrusted.fingerprint)
                        .split(" ")
                        .map((g, i) => (
                          <span key={i} className="rounded-md border border-line-strong bg-sunken px-1.5 py-1 text-center">
                            {g}
                          </span>
                        ))}
                    </div>
                    <div className="mt-4 flex flex-wrap gap-2">
                      <Button size="sm" variant="primary" icon="check" busy={pairing} onClick={() => trustAndPair(untrusted)}>
                        It matches — trust and pair
                      </Button>
                      <Button size="sm" onClick={() => setUntrusted(null)}>
                        Cancel
                      </Button>
                    </div>
                  </div>
                </div>
              </div>
            )}

            {pairError && (
              <Banner tone="error" title="Pairing failed">
                {pairError}
              </Banner>
            )}
            <Button type="submit" variant="primary" size="lg" icon="link" busy={pairing} disabled={!codeOk || !addressOk} className="w-full">
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
              <p className="max-w-xs text-sm text-muted">Find PCs that are hosting on your network. Pairing the first time still needs the code the host shows.</p>
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
                      <Button size="sm" icon="link" onClick={() => useLanHost(h)} title={`Pair through ${h.signaling_url}`}>
                        Pair
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
