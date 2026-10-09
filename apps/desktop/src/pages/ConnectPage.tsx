import { useState } from "react";
import type { Page } from "../App";
import { Banner, Button, Card, EmptyState, Field, PageHeader, StatusDot, TextInput } from "../components/ui";
import { useApp } from "../hooks/useAppState";
import { api, errorMessage } from "../services/api";
import { fmtAgo } from "../services/format";
import type { DiscoveredHost, PairedDevice } from "../types/api";

const CODE_RE = /^[0-9A-HJKMNP-TV-Z]{4}-?[0-9A-HJKMNP-TV-Z]{4}$/i;

export function ConnectPage({ go }: { go: (p: Page) => void }) {
  const { config, devices, refreshDevices, client, connect, notify } = useApp();
  const [code, setCode] = useState("");
  const [server, setServer] = useState<string>("");
  const [pairing, setPairing] = useState(false);
  const [pairError, setPairError] = useState<string | null>(null);
  const [lan, setLan] = useState<DiscoveredHost[] | null>(null);
  const [scanning, setScanning] = useState(false);
  const [connecting, setConnecting] = useState<string | null>(null);

  const serverUrl = server || config?.network.signaling_url || "";
  const codeOk = CODE_RE.test(code.trim());

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

  return (
    <div className="fade-in space-y-6">
      <PageHeader title="Connect" subtitle="Pair once with a code, then connect any time. The host must approve each session." />

      {client && (client.running || client.state === "failed") && (
        <Banner
          tone={client.state === "failed" ? "error" : "info"}
          title={`${client.hostName}: ${client.state.replace(/_/g, " ")}`}
          action={<Button size="sm" onClick={() => go("streaming")}>Details</Button>}
        >
          {client.detail}
        </Banner>
      )}

      <div className="grid grid-cols-2 gap-6">
        <Card title="Pair with a code">
          <form
            className="space-y-4"
            onSubmit={(e) => {
              e.preventDefault();
              if (codeOk) pair();
            }}
          >
            <Field label="Pairing code" hint="Shown on the host under Host → Pairing. Letters are not case-sensitive." error={code && !codeOk ? "Codes look like ABCD-1234 (8 characters)." : null}>
              {(id) => (
                <TextInput
                  id={id}
                  value={code}
                  autoComplete="off"
                  spellCheck={false}
                  placeholder="XXXX-XXXX"
                  className="font-mono text-lg tracking-[0.2em] uppercase"
                  onChange={(e) => setCode(e.target.value.toUpperCase().slice(0, 9))}
                />
              )}
            </Field>
            <Field label="Signaling server" hint="Must be the same server the host uses.">
              {(id) => <TextInput id={id} value={serverUrl} onChange={(e) => setServer(e.target.value)} spellCheck={false} />}
            </Field>
            {pairError && (
              <Banner tone="error" title="Pairing failed">
                {pairError}
              </Banner>
            )}
            <Button type="submit" variant="primary" icon="link" busy={pairing} disabled={!codeOk}>
              {pairing ? "Waiting for the host to approve…" : "Pair"}
            </Button>
          </form>
        </Card>

        <Card
          title="On this network"
          actions={
            <Button size="sm" variant="ghost" icon="refresh" busy={scanning} onClick={scan}>
              Scan
            </Button>
          }
        >
          {lan === null ? (
            <p className="text-sm text-muted">Find hosts that advertise themselves on the local network (mDNS). You still need their pairing code.</p>
          ) : lan.length === 0 ? (
            <EmptyState icon="signal" title="No hosts found">
              Hosts appear here when hosting is on and LAN discovery is enabled. Some networks block multicast.
            </EmptyState>
          ) : (
            <ul className="divide-y divide-line">
              {lan.map((h) => {
                const paired = devices.find((d) => d.device_id === h.device_id);
                return (
                  <li key={h.device_id} className="flex items-center gap-3 py-3">
                    <div className="flex-1">
                      <div className="text-sm font-medium">{h.name}</div>
                      <div className="text-xs text-faint">
                        {h.addresses.slice(0, 2).join(", ")} · via {h.signaling_url}
                      </div>
                    </div>
                    {paired ? (
                      <Button size="sm" variant="primary" icon="play" busy={connecting === paired.device_id} disabled={client?.running} onClick={() => start(paired)}>
                        Connect
                      </Button>
                    ) : (
                      <Button size="sm" onClick={() => setServer(h.signaling_url)}>
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

      <Card title="Saved hosts">
        {devices.length === 0 ? (
          <EmptyState icon="link" title="No paired hosts">Pair with a code to save a host here.</EmptyState>
        ) : (
          <ul className="divide-y divide-line">
            {devices.map((d) => (
              <li key={d.device_id} className="flex items-center gap-4 py-3">
                <div className="flex-1">
                  <div className="text-sm font-medium">{d.name}</div>
                  <div className="text-xs text-faint">
                    {d.signaling_url ?? "default server"} · last used {fmtAgo(d.last_connected_ms)}
                  </div>
                </div>
                {client?.running && client.deviceId === d.device_id && <StatusDot tone="ok" label="in session" />}
                <Button size="sm" variant="ghost" onClick={() => api.forgetDevice(d.device_id).then(refreshDevices).catch((e) => notify("error", errorMessage(e)))}>
                  Forget
                </Button>
                <Button size="sm" variant="primary" icon="play" busy={connecting === d.device_id} disabled={client?.running} onClick={() => start(d)}>
                  Connect
                </Button>
              </li>
            ))}
          </ul>
        )}
      </Card>
    </div>
  );
}
