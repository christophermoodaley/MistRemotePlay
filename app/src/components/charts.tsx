// Small live charts for the streaming statistics. Colours come from the
// --color-s1..s5 tokens (validated categorical slots for the dark surface).
import { useRef, useState } from "react";
import type { LatencyBreakdown } from "../types/api";

/** Line of recent values with a zero baseline and a hover readout. */
export function Sparkline({ values, height = 56, unit = "ms", label }: { values: number[]; height?: number; unit?: string; label: string }) {
  const ref = useRef<HTMLDivElement>(null);
  const [hover, setHover] = useState<number | null>(null);
  const W = 300;
  const n = values.length;
  if (n < 2) {
    return (
      <div className="flex items-center text-xs text-faint" style={{ height }}>
        Collecting samples…
      </div>
    );
  }
  // Scale to the data, but never to less than MIN_SPAN so 1 ms of jitter
  // doesn't look dramatic. The range is printed by the caller.
  const MIN_SPAN = 10;
  const lo0 = Math.min(...values);
  const hi0 = Math.max(...values);
  const pad = Math.max(0, MIN_SPAN - (hi0 - lo0)) / 2;
  const lo = Math.max(0, lo0 - pad - (hi0 - lo0) * 0.1);
  const top = Math.max(lo + MIN_SPAN, hi0 + pad + (hi0 - lo0) * 0.1);
  const x = (i: number) => (i / (n - 1)) * W;
  const y = (v: number) => height - 3 - ((v - lo) / (top - lo)) * (height - 8);
  const line = values.map((v, i) => `${i ? "L" : "M"}${x(i).toFixed(1)} ${y(v).toFixed(1)}`).join(" ");
  const area = `${line} L${W} ${height} L0 ${height} Z`;
  const hi = hover ?? n - 1;

  function onMove(e: React.PointerEvent) {
    const r = ref.current?.getBoundingClientRect();
    if (!r) return;
    const i = Math.round(((e.clientX - r.left) / r.width) * (n - 1));
    setHover(Math.max(0, Math.min(n - 1, i)));
  }

  return (
    <div ref={ref} className="relative" style={{ height }} onPointerMove={onMove} onPointerLeave={() => setHover(null)}>
      <svg role="img" aria-label={`${label}: last ${n} samples, latest ${values[n - 1]!.toFixed(1)} ${unit}`} viewBox={`0 0 ${W} ${height}`} preserveAspectRatio="none" className="h-full w-full overflow-visible">
        <defs>
          <linearGradient id="spark-fill" x1="0" x2="0" y1="0" y2="1">
            <stop offset="0" stopColor="var(--color-accent)" stopOpacity="0.28" />
            <stop offset="1" stopColor="var(--color-accent)" stopOpacity="0" />
          </linearGradient>
        </defs>
        <path d={area} fill="url(#spark-fill)" />
        <path d={line} fill="none" stroke="var(--color-accent)" strokeWidth="2" strokeLinejoin="round" strokeLinecap="round" vectorEffect="non-scaling-stroke" />
        {hover !== null && <line x1={x(hi)} x2={x(hi)} y1="0" y2={height} stroke="var(--color-line-strong)" vectorEffect="non-scaling-stroke" />}
      </svg>
      {/* Marker drawn in HTML so it stays round when the SVG stretches. */}
      <span
        aria-hidden="true"
        className="pointer-events-none absolute h-2.5 w-2.5 -translate-x-1/2 -translate-y-1/2 rounded-full border-2 border-surface bg-accent"
        style={{ left: `${(x(hi) / W) * 100}%`, top: y(values[hi]!) }}
      />
      {hover !== null && (
        <div
          className="num pointer-events-none absolute -top-8 z-10 -translate-x-1/2 whitespace-nowrap rounded-md border border-line-strong bg-raised px-2 py-1 font-mono text-[11px] text-ink shadow-lg"
          style={{ left: `${Math.min(88, Math.max(12, (x(hi) / W) * 100))}%` }}
        >
          {values[hi]!.toFixed(1)} {unit} · {n - 1 - hi === 0 ? "now" : `${n - 1 - hi} samples ago`}
        </div>
      )}
    </div>
  );
}

const STAGES: { key: keyof Omit<LatencyBreakdown, "samples">; label: string; color: string; what: string }[] = [
  { key: "host_ms", label: "Host", color: "var(--color-s1)", what: "Screen grab, colour conversion and encoding on the host" },
  { key: "network_ms", label: "Network", color: "var(--color-s2)", what: "Packetizing, transport and the jitter buffer" },
  { key: "decode_ms", label: "Decode", color: "var(--color-s3)", what: "Video decoding on this device" },
  { key: "convert_ms", label: "Convert", color: "var(--color-s4)", what: "Scaling and colour conversion for the window" },
  { key: "present_ms", label: "Present", color: "var(--color-s5)", what: "Handing the frame to the display" },
];

/** Stacked bar of where capture→display latency is spent, with a legend table. */
export function LatencyBar({ breakdown }: { breakdown: LatencyBreakdown }) {
  const [hover, setHover] = useState<string | null>(null);
  const parts = STAGES.map((s) => ({ ...s, v: breakdown[s.key] }));
  const known = parts.filter((p) => p.v != null && p.v > 0);
  const total = known.reduce((a, p) => a + (p.v ?? 0), 0);
  if (known.length === 0) return <p className="text-sm text-muted">The breakdown appears after the first frames carry host timestamps.</p>;

  return (
    <div>
      <div className="flex h-3.5 w-full gap-[2px]" role="img" aria-label={`Latency by stage: ${known.map((p) => `${p.label} ${p.v!.toFixed(1)} ms`).join(", ")}`}>
        {known.map((p, i) => (
          <div
            key={p.key}
            onPointerEnter={() => setHover(p.key)}
            onPointerLeave={() => setHover(null)}
            className={`h-full transition-opacity ${i === 0 ? "rounded-l-[4px]" : ""} ${i === known.length - 1 ? "rounded-r-[4px]" : ""} ${hover && hover !== p.key ? "opacity-40" : ""}`}
            style={{ width: `${(p.v! / total) * 100}%`, minWidth: 3, background: p.color }}
            title={`${p.label}: ${p.v!.toFixed(1)} ms (${Math.round((p.v! / total) * 100)} %) — ${p.what}`}
          />
        ))}
      </div>
      <table className="mt-4 w-full text-sm">
        <caption className="sr-only">Latency by stage in milliseconds</caption>
        <tbody>
          {parts.map((p) => (
            <tr
              key={p.key}
              onPointerEnter={() => setHover(p.key)}
              onPointerLeave={() => setHover(null)}
              className={`transition-opacity ${hover && hover !== p.key ? "opacity-50" : ""}`}
              title={p.what}
            >
              <td className="py-1 pr-3">
                <span className="inline-flex items-center gap-2.5 text-muted">
                  <span className="h-2.5 w-2.5 rounded-[3px]" style={{ background: p.color }} aria-hidden="true" />
                  {p.label}
                </span>
              </td>
              <td className="hidden py-1 pr-3 text-xs text-faint xl:table-cell">{p.what}</td>
              <td className="num py-1 text-right font-mono text-ink">{p.v != null ? `${p.v.toFixed(1)} ms` : "—"}</td>
              <td className="num w-14 py-1 text-right font-mono text-xs text-faint">{p.v != null && total > 0 ? `${Math.round((p.v / total) * 100)} %` : ""}</td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}
