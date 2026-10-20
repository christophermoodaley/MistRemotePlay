import type { EndReason, SessionState } from "../types/api";

export function fmtMs(v: number | null | undefined, digits = 1): string {
  return v == null || !Number.isFinite(v) ? "—" : `${v.toFixed(digits)} ms`;
}

export function fmtKbps(kbps: number | null | undefined): string {
  if (kbps == null || !Number.isFinite(kbps)) return "—";
  return kbps >= 1000 ? `${(kbps / 1000).toFixed(1)} Mbps` : `${Math.round(kbps)} kbps`;
}

export function fmtPercent(fraction: number | null | undefined): string {
  return fraction == null ? "—" : `${(fraction * 100).toFixed(1)} %`;
}

export function fmtAgo(unixMs: number | null | undefined, now = Date.now()): string {
  if (!unixMs) return "never";
  const s = Math.max(0, Math.round((now - unixMs) / 1000));
  if (s < 60) return "just now";
  if (s < 3600) return `${Math.floor(s / 60)} min ago`;
  if (s < 86400) return `${Math.floor(s / 3600)} h ago`;
  return `${Math.floor(s / 86400)} d ago`;
}

export function endReasonText(r: EndReason): string {
  if (typeof r === "string") {
    return {
      rejected_by_host: "declined by the host",
      approval_timeout: "approval timed out",
      client_disconnected: "client disconnected",
      host_stopped: "host stopped",
      kicked_by_host: "disconnected by the host",
      expired: "session expired",
    }[r];
  }
  if ("connection_failed" in r) return `connection failed: ${r.connection_failed}`;
  return `error: ${r.error}`;
}

export function sessionStateText(s: SessionState): string {
  switch (s.state) {
    case "requested":
      return "Requested";
    case "awaiting_approval":
      return "Waiting for your approval";
    case "negotiating":
      return "Connecting";
    case "connected":
      return "Streaming";
    case "reconnecting":
      return "Reconnecting";
    case "ended":
      return `Ended (${endReasonText(s.detail)})`;
  }
}

export function shortId(id: string): string {
  return id.slice(0, 8);
}

/** A certificate fingerprint in groups of four, for people to compare. */
export function fmtFingerprint(hex: string): string {
  return (hex.toUpperCase().match(/.{1,4}/g) ?? []).join(" ");
}
