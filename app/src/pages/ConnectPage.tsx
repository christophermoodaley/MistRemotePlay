// "Add a host": find the host, enter its code, done.
import { useEffect, useRef, useState } from "react";
import type { Page } from "../App";
import { Icon } from "../components/Icon";
import { Avatar, Banner, Button, FingerprintGrid, Spinner, Steps, TextInput } from "../components/ui";
import { useApp } from "../hooks/useAppState";
import { api, errorMessage, untrustedServer, type UntrustedServer } from "../services/api";
import type { DiscoveredHost, Invite, PairedDevice } from "../types/api";

const CODE_RE = /^[0-9A-HJKMNP-TV-Z]{4}-?[0-9A-HJKMNP-TV-Z]{4}$/i;
/** The configuration default, which only works with a server on this PC. */
const DEFAULT_SERVER = "ws://127.0.0.1:8443";
const STEPS = ["Find the host", "Enter the code", "Done"];

/** The host being added: picked from the network, typed in, or from an invite. */
interface Target {
  address: string;
  name: string | null;
  /** Pasted invite text: pairs in one step, no code to type. */
  invite?: string;
}

/** An invite on the clipboard, if any (checked when the page opens). */
async function clipboardInvite(): Promise<{ text: string; invite: Invite } | null> {
  try {
    const text = await api.readClipboard();
    if (!text) return null;
    const invite = await api.parseInvite(text);
    return invite ? { text, invite } : null;
  } catch {
    return null;
  }
}

export function ConnectPage({ go }: { go: (p: Page) => void }) {
  const [target, setTarget] = useState<Target | null>(null);
  const [paired, setPaired] = useState<PairedDevice | null>(null);
  const step = paired ? 2 : target ? 1 : 0;

  return (
    <div className="fade-in mx-auto max-w-2xl space-y-6">
      <div>
        <button type="button" onClick={() => go("home")} className="mb-4 inline-flex items-center gap-1.5 text-sm text-muted hover:text-ink">
          <Icon name="chevron" size={15} className="rotate-180" /> Home
        </button>
        <h1 className="text-[28px] font-semibold leading-tight tracking-[-0.02em]">Add a host</h1>
        <p className="mt-1.5 text-sm text-muted">Connect this PC to one that shares its games. You only do this once per host.</p>
      </div>
      <div className="card rounded-[var(--radius-card)] border border-line px-5 py-4">
        <Steps steps={STEPS} current={step} />
      </div>
      {step === 0 && <FindStep onPick={setTarget} go={go} />}
      {step === 1 && target?.invite && <InviteStep target={target} onBack={() => setTarget(null)} onPaired={setPaired} />}
      {step === 1 && target && !target.invite && <CodeStep target={target} onBack={() => setTarget(null)} onPaired={setPaired} />}
      {step === 2 && paired && <DoneStep device={paired} go={go} />}
    </div>
  );
}

function FindStep({ onPick, go }: { onPick: (t: Target) => void; go: (p: Page) => void }) {
  const { config, devices, client, connect, notify } = useApp();
  const configured = config?.network.signaling_url && config.network.signaling_url !== DEFAULT_SERVER ? config.network.signaling_url : "";
  const [address, setAddress] = useState(configured);
  const [lan, setLan] = useState<DiscoveredHost[] | null>(null);
  const [scanning, setScanning] = useState(false);
  const [connecting, setConnecting] = useState<string | null>(null);
  const [found, setFound] = useState<{ text: string; invite: Invite } | null>(null);
  const [pasteFailed, setPasteFailed] = useState(false);

  // An invite someone sent is most likely on the clipboard already.
  useEffect(() => {
    clipboardInvite().then(setFound);
  }, []);

  function addFromInvite(text: string, invite: Invite) {
    onPick({ address: invite.addresses[0] ?? "", name: invite.name, invite: text });
  }

  async function pasteInvite() {
    const f = await clipboardInvite();
    setPasteFailed(!f);
    if (f) addFromInvite(f.text, f.invite);
  }

  /** Typed or pasted into the address box: an invite works there too. */
  async function submitAddress() {
    const text = address.trim();
    if (!text) return;
    const invite = text.includes("mist://pair/") ? await api.parseInvite(text).catch(() => null) : null;
    if (invite) addFromInvite(text, invite);
    else onPick({ address: text, name: null });
  }

  async function scan() {
    setScanning(true);
    try {
      setLan(await api.discoverLan(3));
    } catch (e) {
      setLan([]);
      notify("error", errorMessage(e));
    } finally {
      setScanning(false);
    }
  }

  // Look right away: most hosts are on the same network.
  useEffect(() => {
    scan();
  }, []);

  /** A host found on this network: its address and certificate are known. */
  async function pick(h: DiscoveredHost) {
    try {
      if (h.fingerprint) await api.trustServer(h.signaling_url, h.fingerprint);
      onPick({ address: h.signaling_url, name: h.name });
    } catch (e) {
      notify("error", errorMessage(e));
    }
  }

  async function play(d: PairedDevice) {
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
    <>
      <section className={`card rounded-[var(--radius-card)] border p-5 ${found ? "border-accent/60" : "border-line"}`}>
        <div className="flex flex-wrap items-center justify-between gap-3">
          <div className="min-w-0">
            <h2 className="text-[15px] font-semibold tracking-tight">{found ? `Invite from ${found.invite.name ?? "a host"} found` : "Got an invite?"}</h2>
            <p className="mt-0.5 text-xs text-muted">
              {found ? "It was on your clipboard. One click adds this host." : "Copy the invite the host sent you, then paste it here. No code to type."}
            </p>
          </div>
          {found ? (
            <Button variant="primary" icon="link" onClick={() => addFromInvite(found.text, found.invite)}>
              Add {found.invite.name ?? "host"}
            </Button>
          ) : (
            <Button variant="primary" icon="copy" onClick={pasteInvite}>
              Paste invite
            </Button>
          )}
        </div>
        {pasteFailed && !found && <p className="mt-2 text-xs text-warn">There's no invite on the clipboard. On the host, click Copy invite and send it to this PC.</p>}
      </section>

      <section className="card rounded-[var(--radius-card)] border border-line p-5">
        <div className="mb-4 flex items-center justify-between gap-3">
          <div>
            <h2 className="text-[15px] font-semibold tracking-tight">On your network</h2>
            <p className="mt-0.5 text-xs text-muted">PCs nearby that are sharing with Mist.</p>
          </div>
          <Button size="sm" variant="ghost" icon="refresh" busy={scanning} onClick={scan}>
            Look again
          </Button>
        </div>
        {lan === null || (scanning && lan.length === 0) ? (
          <div className="flex items-center gap-3 rounded-[14px] border border-line bg-sunken/60 px-4 py-5 text-sm text-muted">
            <Spinner /> Looking for hosts…
          </div>
        ) : lan.length === 0 ? (
          <div className="rounded-[14px] border border-dashed border-line-strong/70 bg-sunken/40 px-4 py-5 text-sm text-muted">
            None found. Make sure the other PC has <span className="text-ink">Host this PC</span> turned on, or type its address below.
          </div>
        ) : (
          <ul className="space-y-2">
            {lan.map((h) => {
              const known = devices.find((d) => d.device_id === h.device_id);
              return (
                <li key={h.device_id} className="flex items-center gap-3 rounded-[14px] border border-line bg-sunken/60 px-3.5 py-3">
                  <Avatar name={h.name} id={h.device_id} size={36} />
                  <div className="min-w-0 flex-1">
                    <div className="truncate text-sm font-medium">{h.name}</div>
                    <div className="truncate text-xs text-faint">{known ? "Already added" : h.addresses[0]}</div>
                  </div>
                  {known ? (
                    <Button size="sm" icon="play" busy={connecting === known.device_id} disabled={client?.running} onClick={() => play(known)}>
                      Play
                    </Button>
                  ) : (
                    <Button size="sm" variant="primary" onClick={() => pick(h)}>
                      Choose
                    </Button>
                  )}
                </li>
              );
            })}
          </ul>
        )}
      </section>

      <section className="card rounded-[var(--radius-card)] border border-line p-5">
        <h2 className="text-[15px] font-semibold tracking-tight">Somewhere else?</h2>
        <p className="mt-0.5 text-xs text-muted">Over the internet, type the address shown on the host under Host this PC (or paste an invite).</p>
        <form
          className="mt-4 flex gap-2"
          onSubmit={(e) => {
            e.preventDefault();
            submitAddress();
          }}
        >
          <TextInput value={address} placeholder="e.g. 203.0.113.7:47900" spellCheck={false} autoComplete="off" onChange={(e) => setAddress(e.target.value)} className="font-mono" aria-label="Host's address" />
          <Button type="submit" variant="primary" disabled={!address.trim()}>
            Next
          </Button>
        </form>
      </section>
    </>
  );
}

function CodeStep({ target, onBack, onPaired }: { target: Target; onBack: () => void; onPaired: (d: PairedDevice) => void }) {
  const { refreshDevices } = useApp();
  const [code, setCode] = useState("");
  const [pairing, setPairing] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [untrusted, setUntrusted] = useState<UntrustedServer | null>(null);
  const input = useRef<HTMLInputElement>(null);
  const codeOk = CODE_RE.test(code.trim());
  const label = target.name ?? target.address.replace(/^wss?:\/\//, "");

  useEffect(() => input.current?.focus(), []);

  async function pair(c = code) {
    setPairing(true);
    setError(null);
    setUntrusted(null);
    try {
      const d = await api.pairWithCode(c.trim(), target.address);
      await refreshDevices();
      onPaired(d);
    } catch (e) {
      const u = untrustedServer(e);
      if (u) setUntrusted(u);
      else setError(errorMessage(e));
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
      setError(errorMessage(e));
    }
  }

  /** Format as the user types: uppercase, a dash after four characters. */
  function format(v: string): string {
    const raw = v.toUpperCase().replace(/[^0-9A-Z]/g, "").slice(0, 8);
    return raw.length > 4 ? `${raw.slice(0, 4)}-${raw.slice(4)}` : raw;
  }

  function onCode(v: string) {
    setError(null);
    setCode(format(v));
  }

  /** A pasted code (or invite) is complete: add the host right away. */
  async function usePasted(text: string): Promise<boolean> {
    const invite = text.includes("mist://pair/") ? await api.parseInvite(text).catch(() => null) : null;
    const c = format(invite ? invite.code : text);
    setError(null);
    setCode(c);
    if (CODE_RE.test(c) && !pairing) {
      pair(c);
      return true;
    }
    return false;
  }

  async function pasteCode() {
    const text = await api.readClipboard().catch(() => null);
    if (!text || !(await usePasted(text))) setError("There's no code on the clipboard. Copy it on the host first.");
  }

  return (
    <section className="card rounded-[var(--radius-card)] border border-line p-6">
      <div className="mb-5 flex items-center justify-between gap-3">
        <div className="min-w-0">
          <div className="text-xs text-muted">Adding</div>
          <div className="truncate text-[15px] font-semibold">{label}</div>
        </div>
        <Button size="sm" variant="ghost" onClick={onBack} disabled={pairing}>
          Change
        </Button>
      </div>
      <form
        className="space-y-5"
        onSubmit={(e) => {
          e.preventDefault();
          if (codeOk) pair();
        }}
      >
        <div>
          <label htmlFor="pair-code" className="text-sm font-medium text-ink">
            Enter the code shown on {target.name ?? "the host"}
          </label>
          <p className="mt-0.5 text-xs text-muted">On that PC, open Mist → Host this PC. The code is there. Pasting it adds the host straight away.</p>
          <input
            id="pair-code"
            ref={input}
            value={code}
            autoComplete="off"
            spellCheck={false}
            placeholder="XXXX-XXXX"
            onChange={(e) => onCode(e.target.value)}
            onPaste={(e) => {
              e.preventDefault();
              usePasted(e.clipboardData.getData("text"));
            }}
            className={`mt-3 h-16 w-full rounded-[14px] border bg-sunken px-4 text-center font-mono text-[28px] font-semibold uppercase tracking-[0.32em] text-ink placeholder:text-line-strong focus:bg-bg focus:outline-none ${codeOk ? "border-accent/70" : "border-line focus:border-accent"}`}
          />
          {code.length >= 9 && !codeOk && <p className="mt-1.5 text-xs text-bad">That doesn't look right. Codes look like ABCD-1234.</p>}
          {!pairing && (
            <div className="mt-2 text-right">
              <Button size="sm" variant="ghost" icon="copy" onClick={pasteCode}>
                Paste code
              </Button>
            </div>
          )}
        </div>

        {untrusted && (
          <div role="alert" className="fade-in rounded-[14px] border border-warn/45 bg-warn/[0.06] p-4">
            <div className="flex items-start gap-3">
              <Icon name="shield" className="mt-0.5 shrink-0 text-warn" />
              <div className="min-w-0 flex-1">
                <div className="text-sm font-semibold">First time connecting: check it's really your host</div>
                <p className="mt-1 text-sm text-muted">On the host, open Host this PC → Addresses and security. The fingerprint there must match this one:</p>
                <div className="mt-3">
                  <FingerprintGrid hex={untrusted.fingerprint} />
                </div>
                <div className="mt-4 flex flex-wrap gap-2">
                  <Button size="sm" variant="primary" icon="check" busy={pairing} onClick={() => trustAndPair(untrusted)}>
                    It matches
                  </Button>
                  <Button size="sm" onClick={() => setUntrusted(null)}>
                    Cancel
                  </Button>
                </div>
              </div>
            </div>
          </div>
        )}

        {error && (
          <Banner tone="error" title="That didn't work">
            {error}
          </Banner>
        )}
        <Button type="submit" variant="primary" size="lg" icon={pairing ? undefined : "link"} busy={pairing} disabled={!codeOk} className="w-full">
          {pairing ? `Waiting for ${target.name ?? "the host"} to approve…` : "Add this host"}
        </Button>
        {pairing && <p className="-mt-2 text-center text-xs text-muted">Someone at the host needs to click Approve.</p>}
      </form>
    </section>
  );
}

/** Adding a host from an invite: everything is in it, so it just runs. */
function InviteStep({ target, onBack, onPaired }: { target: Target; onBack: () => void; onPaired: (d: PairedDevice) => void }) {
  const { refreshDevices } = useApp();
  const [error, setError] = useState<string | null>(null);
  const [attempt, setAttempt] = useState(0);
  const name = target.name ?? "the host";

  useEffect(() => {
    let live = true;
    setError(null);
    api
      .pairWithInvite(target.invite ?? "")
      .then(async (d) => {
        await refreshDevices();
        if (live) onPaired(d);
      })
      .catch((e) => {
        if (live) setError(errorMessage(e));
      });
    return () => {
      live = false;
    };
  }, [attempt]);

  return (
    <section className="card rounded-[var(--radius-card)] border border-line p-6">
      <div className="mb-5 flex items-center justify-between gap-3">
        <div className="min-w-0">
          <div className="text-xs text-muted">Adding from an invite</div>
          <div className="truncate text-[15px] font-semibold">{name}</div>
        </div>
        <Button size="sm" variant="ghost" onClick={onBack}>
          Change
        </Button>
      </div>
      {error ? (
        <div className="space-y-4">
          <Banner tone="error" title="That didn't work">
            {error}
          </Banner>
          <Button variant="primary" icon="refresh" onClick={() => setAttempt((n) => n + 1)}>
            Try again
          </Button>
        </div>
      ) : (
        <div className="flex items-center gap-3 rounded-[14px] border border-line bg-sunken/60 px-4 py-5 text-sm text-muted">
          <Spinner /> Waiting for {name} to approve. Someone at the host needs to click Approve.
        </div>
      )}
    </section>
  );
}

function DoneStep({ device, go }: { device: PairedDevice; go: (p: Page) => void }) {

  const { connect, client, notify } = useApp();
  const [busy, setBusy] = useState(false);

  async function play() {
    setBusy(true);
    try {
      await connect(device);
      go("streaming");
    } catch (e) {
      notify("error", errorMessage(e));
      setBusy(false);
    }
  }

  return (
    <section className="card fade-in flex flex-col items-center rounded-[var(--radius-card)] border border-ok/30 px-6 py-10 text-center">
      <span className="flex h-14 w-14 items-center justify-center rounded-full bg-ok/12 text-ok">
        <Icon name="check" size={28} />
      </span>
      <h2 className="mt-4 text-xl font-semibold tracking-tight">{device.name} is added</h2>
      <p className="mt-1 max-w-sm text-sm text-muted">From now on it's on your Home screen. Press Play any time; the host approves each session.</p>
      <div className="mt-6 flex flex-wrap justify-center gap-3">
        <Button variant="primary" size="lg" icon="play" busy={busy} disabled={client?.running} onClick={play}>
          Play now
        </Button>
        <Button size="lg" onClick={() => go("home")}>
          Back to Home
        </Button>
      </div>
    </section>
  );
}
