// Time-sensitive prompts for the host: new pairings and join requests.
// Shown above every page so nobody has to find the Host page to answer them.
import { Icon, type IconName } from "./Icon";
import { Button } from "./ui";
import { useApp } from "../hooks/useAppState";
import { api, errorMessage } from "../services/api";
import { fmtKbps } from "../services/format";

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

export function HostRequests() {
  const { host, sessionRequests, notify } = useApp();
  const pairings = host?.pending_pairings ?? [];
  if (pairings.length === 0 && sessionRequests.length === 0) return null;

  async function run(f: () => Promise<unknown>) {
    try {
      await f();
    } catch (e) {
      notify("error", errorMessage(e));
    }
  }

  return (
    <div className="mb-6 space-y-3">
      {pairings.map((p) => (
        <ConsentCard
          key={p.request_id}
          title={`“${p.client_name}” wants to pair with this PC`}
          approveLabel="Approve"
          onApprove={() => run(() => api.hostDecidePairing(p.request_id, true))}
          onDecline={() => run(() => api.hostDecidePairing(p.request_id, false))}
        >
          Only approve if you just gave them the code. Their screen shows this fingerprint: <FingerprintChips value={p.fingerprint} />
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
          {r.settings.width}×{r.settings.height} at {r.settings.fps} fps, up to {fmtKbps(r.settings.max_bitrate_kbps)}
        </ConsentCard>
      ))}
    </div>
  );
}
