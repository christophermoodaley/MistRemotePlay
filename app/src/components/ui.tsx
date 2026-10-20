// Reusable, accessible building blocks.
import { useId, useState, type ButtonHTMLAttributes, type ReactNode } from "react";
import { Icon, type IconName } from "./Icon";

type Variant = "primary" | "secondary" | "ghost" | "danger";

const VARIANTS: Record<Variant, string> = {
  primary: "bg-accent text-accent-ink hover:bg-accent-strong glow-accent disabled:bg-line disabled:text-faint disabled:shadow-none",
  secondary: "bg-raised text-ink border border-line hover:border-line-strong hover:bg-hover disabled:text-faint disabled:hover:bg-raised",
  ghost: "text-muted hover:text-ink hover:bg-raised disabled:text-faint disabled:hover:bg-transparent",
  danger: "bg-bad/12 text-bad border border-bad/35 hover:bg-bad/22 disabled:opacity-50",
};

export function Button({
  variant = "secondary",
  size = "md",
  icon,
  busy,
  children,
  className = "",
  ...rest
}: ButtonHTMLAttributes<HTMLButtonElement> & {
  variant?: Variant;
  size?: "sm" | "md" | "lg";
  icon?: IconName;
  busy?: boolean;
}) {
  const sizes = { sm: "h-8 px-3 text-[13px] rounded-[9px]", md: "h-10 px-4 text-sm rounded-[11px]", lg: "h-12 px-6 text-[15px] rounded-[13px]" };
  return (
    <button
      type="button"
      className={`inline-flex shrink-0 items-center justify-center gap-2 font-medium transition-all duration-150 active:scale-[0.98] disabled:cursor-not-allowed disabled:active:scale-100 ${sizes[size]} ${VARIANTS[variant]} ${className}`}
      disabled={busy || rest.disabled}
      aria-busy={busy || undefined}
      {...rest}
    >
      {busy ? <Spinner size={size === "sm" ? 13 : 15} /> : icon ? <Icon name={icon} size={size === "sm" ? 15 : size === "lg" ? 19 : 17} /> : null}
      {children}
    </button>
  );
}

export function Spinner({ size = 16 }: { size?: number }) {
  return (
    <span
      role="status"
      aria-label="Loading"
      className="inline-block shrink-0 animate-spin rounded-full border-2 border-current border-t-transparent"
      style={{ width: size, height: size }}
    />
  );
}

export function Card({
  title,
  subtitle,
  icon,
  actions,
  children,
  className = "",
}: {
  title?: ReactNode;
  subtitle?: ReactNode;
  icon?: IconName;
  actions?: ReactNode;
  children: ReactNode;
  className?: string;
}) {
  return (
    <section className={`card rounded-[var(--radius-card)] border border-line p-5 ${className}`}>
      {(title || actions) && (
        <header className="mb-4 flex items-start justify-between gap-3">
          <div className="flex min-w-0 items-center gap-3">
            {icon && (
              <span className="flex h-8 w-8 shrink-0 items-center justify-center rounded-[10px] border border-line bg-raised text-muted">
                <Icon name={icon} size={16} />
              </span>
            )}
            <div className="min-w-0">
              {title && <h2 className="text-[15px] font-semibold tracking-tight">{title}</h2>}
              {subtitle && <p className="mt-0.5 text-xs text-muted">{subtitle}</p>}
            </div>
          </div>
          {actions && <div className="flex shrink-0 items-center gap-2">{actions}</div>}
        </header>
      )}
      {children}
    </section>
  );
}

export type Tone = "neutral" | "accent" | "ok" | "warn" | "bad";
const TONES: Record<Tone, string> = {
  neutral: "bg-raised text-muted border-line",
  accent: "bg-accent/12 text-accent border-accent/30",
  ok: "bg-ok/12 text-ok border-ok/30",
  warn: "bg-warn/12 text-warn border-warn/30",
  bad: "bg-bad/12 text-bad border-bad/30",
};
const DOT: Record<Tone, string> = { neutral: "bg-faint", accent: "bg-accent", ok: "bg-ok", warn: "bg-warn", bad: "bg-bad" };
const DOT_TEXT: Record<Tone, string> = { neutral: "text-faint", accent: "text-accent", ok: "text-ok", warn: "text-warn", bad: "text-bad" };

export function Badge({ tone = "neutral", children }: { tone?: Tone; children: ReactNode }) {
  return <span className={`inline-flex shrink-0 items-center gap-1.5 whitespace-nowrap rounded-full border px-2.5 py-0.5 text-xs font-medium ${TONES[tone]}`}>{children}</span>;
}

/** A status dot; `live` adds a soft pulse for things that are happening now. */
export function LiveDot({ tone, live = false, size = 8 }: { tone: Tone; live?: boolean; size?: number }) {
  return (
    <span
      aria-hidden="true"
      className={`inline-block shrink-0 rounded-full ${DOT[tone]} ${DOT_TEXT[tone]} ${live ? "animate-pulse-ring" : ""}`}
      style={{ width: size, height: size }}
    />
  );
}

export function StatusDot({ tone, label, live }: { tone: Tone; label: string; live?: boolean }) {
  return (
    <span className="inline-flex shrink-0 items-center gap-2 text-sm text-muted">
      <LiveDot tone={tone} live={live} />
      {label}
    </span>
  );
}

export function Toggle({ checked, onChange, label, description, disabled }: { checked: boolean; onChange: (v: boolean) => void; label: string; description?: string; disabled?: boolean }) {
  const id = useId();
  return (
    <div className="flex items-center justify-between gap-6 py-2.5">
      <div className="min-w-0">
        <label htmlFor={id} className={`text-sm ${disabled ? "text-faint" : "text-ink"}`}>
          {label}
        </label>
        {description && <p className="mt-0.5 text-xs text-muted">{description}</p>}
      </div>
      <button
        id={id}
        type="button"
        role="switch"
        aria-checked={checked}
        disabled={disabled}
        onClick={() => onChange(!checked)}
        className={`relative h-6 w-11 shrink-0 rounded-full border transition-colors duration-200 disabled:opacity-45 ${checked ? "border-accent bg-accent" : "border-line-strong bg-sunken"}`}
      >
        <span className={`absolute left-[2px] top-[2px] h-[18px] w-[18px] rounded-full shadow transition-transform duration-200 ${checked ? "translate-x-5 bg-accent-ink" : "translate-x-0 bg-muted"}`} />
      </button>
    </div>
  );
}

export function Field({ label, hint, error, children }: { label: string; hint?: string; error?: string | null; children: (id: string) => ReactNode }) {
  const id = useId();
  return (
    <div className="flex min-w-0 flex-col gap-1.5">
      <label htmlFor={id} className="text-[13px] font-medium text-muted">
        {label}
      </label>
      {children(id)}
      {error ? (
        <p role="alert" className="text-xs text-bad">
          {error}
        </p>
      ) : hint ? (
        <p className="text-xs text-faint">{hint}</p>
      ) : null}
    </div>
  );
}

const inputCls =
  "h-10 w-full rounded-[11px] border border-line bg-sunken px-3 text-sm text-ink placeholder:text-faint transition-colors hover:border-line-strong focus:border-accent focus:bg-bg focus:outline-none";

export function TextInput(props: React.InputHTMLAttributes<HTMLInputElement>) {
  return <input {...props} className={`${inputCls} ${props.className ?? ""}`} />;
}

export function NumberInput({ value, onChange, min, max, step, id }: { value: number; onChange: (v: number) => void; min?: number; max?: number; step?: number; id?: string }) {
  return (
    <input
      id={id}
      type="number"
      className={`${inputCls} num`}
      value={Number.isFinite(value) ? value : ""}
      min={min}
      max={max}
      step={step}
      onChange={(e) => onChange(Number(e.target.value))}
    />
  );
}

const CHEVRON = `url("data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 24 24' fill='none' stroke='%2394a1af' stroke-width='2' stroke-linecap='round' stroke-linejoin='round'%3E%3Cpath d='M6 9l6 6 6-6'/%3E%3C/svg%3E")`;

export function Select<T extends string>({ id, value, onChange, options }: { id?: string; value: T; onChange: (v: T) => void; options: { value: T; label: string; disabled?: boolean }[] }) {
  return (
    <select id={id} className={`${inputCls} appearance-none bg-[length:16px] bg-[right_10px_center] bg-no-repeat pr-9`} style={{ backgroundImage: CHEVRON }} value={value} onChange={(e) => onChange(e.target.value as T)}>
      {options.map((o) => (
        <option key={o.value} value={o.value} disabled={o.disabled}>
          {o.label}
        </option>
      ))}
    </select>
  );
}

export function Slider({ id, value, onChange, min, max, step, format, label }: { id?: string; value: number; onChange: (v: number) => void; min: number; max: number; step: number; format?: (v: number) => string; label?: string }) {
  return (
    <div className="flex items-center gap-3">
      <input id={id} aria-label={label} type="range" min={min} max={max} step={step} value={value} onChange={(e) => onChange(Number(e.target.value))} className="w-full" />
      <span className="num w-14 shrink-0 text-right font-mono text-xs text-muted">{format ? format(value) : value}</span>
    </div>
  );
}

/** A labelled value. `unit` is set smaller next to the number. */
export function Stat({ label, value, unit, hint, tone }: { label: string; value: ReactNode; unit?: string; hint?: string; tone?: Tone }) {
  return (
    <div className="min-w-0 rounded-[13px] border border-line bg-sunken/70 px-3.5 py-3" title={hint}>
      <div className="truncate text-[11px] font-medium uppercase tracking-[0.06em] text-faint">{label}</div>
      <div className={`num mt-1.5 truncate text-[15px] font-semibold ${tone === "warn" ? "text-warn" : tone === "bad" ? "text-bad" : tone === "ok" ? "text-ok" : "text-ink"}`}>
        {value}
        {unit && <span className="ml-1 text-xs font-medium text-muted">{unit}</span>}
      </div>
    </div>
  );
}

/** Small uppercase label used to group stats. */
export function SectionLabel({ children, icon }: { children: ReactNode; icon?: IconName }) {
  return (
    <div className="mb-2 flex items-center gap-2 text-[11px] font-semibold uppercase tracking-[0.08em] text-faint">
      {icon && <Icon name={icon} size={13} />}
      {children}
    </div>
  );
}

export function Banner({ tone, title, children, action }: { tone: "info" | "warn" | "error"; title: string; children?: ReactNode; action?: ReactNode }) {
  const map = {
    info: "border-accent/30 bg-accent/[0.07]",
    warn: "border-warn/40 bg-warn/[0.07]",
    error: "border-bad/40 bg-bad/[0.07]",
  };
  return (
    <div role={tone === "error" ? "alert" : "status"} className={`fade-in flex flex-wrap items-start gap-3 rounded-[14px] border p-4 text-ink ${map[tone]}`}>
      <Icon name={tone === "info" ? "signal" : "alert"} className={`mt-0.5 shrink-0 ${tone === "error" ? "text-bad" : tone === "warn" ? "text-warn" : "text-accent"}`} />
      <div className="min-w-0 flex-1">
        <div className="text-sm font-medium">{title}</div>
        {children && <div className="mt-1 text-sm text-muted">{children}</div>}
      </div>
      {action}
    </div>
  );
}

export function EmptyState({ icon, title, children }: { icon: IconName; title: string; children?: ReactNode }) {
  return (
    <div className="flex flex-col items-center justify-center gap-2 rounded-[14px] border border-dashed border-line-strong/70 bg-sunken/40 px-6 py-10 text-center">
      <span className="mb-1 flex h-11 w-11 items-center justify-center rounded-full border border-line bg-raised text-muted">
        <Icon name={icon} size={20} />
      </span>
      <div className="text-sm font-medium text-ink">{title}</div>
      {children && <div className="max-w-md text-sm text-muted">{children}</div>}
    </div>
  );
}

export function PageHeader({ title, eyebrow, subtitle, actions }: { title: ReactNode; eyebrow?: ReactNode; subtitle?: ReactNode; actions?: ReactNode }) {
  return (
    <div className="mb-7 flex flex-wrap items-end justify-between gap-4">
      <div className="min-w-0">
        {eyebrow && <div className="mb-1.5 text-xs font-semibold uppercase tracking-[0.1em] text-accent">{eyebrow}</div>}
        <h1 className="text-[28px] font-semibold leading-tight tracking-[-0.02em]">{title}</h1>
        {subtitle && <p className="mt-1.5 max-w-2xl text-sm text-muted">{subtitle}</p>}
      </div>
      {actions && <div className="flex flex-wrap items-center gap-2">{actions}</div>}
    </div>
  );
}

export function Kbd({ children }: { children: ReactNode }) {
  return <kbd className="inline-flex min-w-[22px] justify-center rounded-md border border-line-strong border-b-2 bg-raised px-1.5 py-0.5 font-mono text-[11px] text-muted">{children}</kbd>;
}

/** Pill-shaped tab bar (WAI-ARIA tabs). */
export function Segmented<T extends string>({ items, value, onChange, label }: { items: { id: T; label: string; icon?: IconName }[]; value: T; onChange: (v: T) => void; label: string }) {
  return (
    <div role="tablist" aria-label={label} className="inline-flex max-w-full flex-wrap gap-1 rounded-[14px] border border-line bg-sunken/80 p-1">
      {items.map((t) => {
        const active = t.id === value;
        return (
          <button
            key={t.id}
            role="tab"
            type="button"
            aria-selected={active}
            onClick={() => onChange(t.id)}
            className={`inline-flex items-center gap-2 rounded-[10px] px-3.5 py-2 text-[13px] font-medium transition-colors ${active ? "bg-raised text-ink shadow-[inset_0_1px_0_rgb(255_255_255/0.05),0_1px_2px_rgb(0_0_0/0.4)]" : "text-muted hover:text-ink"}`}
          >
            {t.icon && <Icon name={t.icon} size={15} className={active ? "text-accent" : ""} />}
            {t.label}
          </button>
        );
      })}
    </div>
  );
}

// ---- host artwork

const ART: [string, string][] = [
  ["#0f5c55", "#163a6b"],
  ["#173f73", "#3b2a6e"],
  ["#4a2463", "#14485a"],
  ["#0e4f43", "#2b5a1e"],
  ["#5a2a2a", "#3a2a63"],
  ["#1d4a6e", "#0f5c55"],
];

function hash(s: string): number {
  let h = 2166136261;
  for (let i = 0; i < s.length; i++) h = Math.imul(h ^ s.charCodeAt(i), 16777619);
  return h >>> 0;
}

/** A decorative, deterministic "screen" for a host, derived from its id. */
export function HostArt({ id, className = "", children }: { id: string; className?: string; children?: ReactNode }) {
  const h = hash(id);
  const [a, b] = ART[h % ART.length];
  const x = 20 + (h >> 8) % 60;
  return (
    <div className={`relative overflow-hidden ${className}`} style={{ background: `radial-gradient(120% 90% at ${x}% 0%, ${a}, transparent 70%), linear-gradient(160deg, ${b}, #090c11 85%)` }}>
      <svg aria-hidden="true" className="absolute inset-x-0 bottom-0 h-1/2 w-full opacity-40" viewBox="0 0 200 60" preserveAspectRatio="none">
        <path d={`M0 40 Q 50 ${20 + (h % 15)} 100 38 T 200 30 V60 H0z`} fill="rgb(255 255 255 / 0.06)" />
        <path d={`M0 50 Q 60 ${35 + (h % 10)} 120 48 T 200 44 V60 H0z`} fill="rgb(255 255 255 / 0.05)" />
      </svg>
      {children}
    </div>
  );
}

/** Round avatar with initials for a device. */
export function Avatar({ name, id, size = 36 }: { name: string; id: string; size?: number }) {
  const initials = name
    .split(/[\s\-_']+/)
    .filter(Boolean)
    .slice(0, 2)
    .map((w) => w[0]!.toUpperCase())
    .join("");
  return (
    <HostArt id={id} className="shrink-0 rounded-[11px] border border-white/10">
      <span className="relative flex items-center justify-center text-[13px] font-semibold text-white/90" style={{ width: size, height: size }} aria-hidden="true">
        {initials || "?"}
      </span>
    </HostArt>
  );
}

/** Circular countdown; `fraction` is the share of time left (0..1). */
export function CountdownRing({ fraction, label, size = 56 }: { fraction: number; label: string; size?: number }) {
  const r = (size - 6) / 2;
  const c = 2 * Math.PI * r;
  const f = Math.max(0, Math.min(1, fraction));
  return (
    <div className="relative shrink-0" style={{ width: size, height: size }}>
      <svg width={size} height={size} viewBox={`0 0 ${size} ${size}`} className="-rotate-90" aria-hidden="true">
        <circle cx={size / 2} cy={size / 2} r={r} fill="none" stroke="var(--color-line)" strokeWidth="3" />
        <circle
          cx={size / 2}
          cy={size / 2}
          r={r}
          fill="none"
          stroke={f < 0.2 ? "var(--color-warn)" : "var(--color-accent)"}
          strokeWidth="3"
          strokeLinecap="round"
          strokeDasharray={c}
          strokeDashoffset={c * (1 - f)}
          style={{ transition: "stroke-dashoffset 1s linear" }}
        />
      </svg>
      <span className="num absolute inset-0 flex items-center justify-center font-mono text-[11px] text-muted">{label}</span>
    </div>
  );
}

/** Numbered progress for a short guided flow. `current` is 0-based. */
export function Steps({ steps, current }: { steps: string[]; current: number }) {
  return (
    <ol className="flex items-center gap-3" aria-label="Progress">
      {steps.map((s, i) => {
        const done = i < current;
        const active = i === current;
        return (
          <li key={s} className="flex flex-1 items-center gap-3 last:flex-none" aria-current={active ? "step" : undefined}>
            <span className="flex items-center gap-2.5">
              <span
                className={`flex h-7 w-7 shrink-0 items-center justify-center rounded-full border text-xs font-semibold transition-colors ${
                  done ? "border-accent/60 bg-accent/15 text-accent" : active ? "border-accent bg-accent text-accent-ink" : "border-line-strong text-faint"
                }`}
              >
                {done ? <Icon name="check" size={14} /> : i + 1}
              </span>
              <span className={`hidden whitespace-nowrap text-sm sm:inline ${active ? "font-medium text-ink" : done ? "text-muted" : "text-faint"}`}>{s}</span>
            </span>
            {i < steps.length - 1 && <span className={`h-px flex-1 ${done ? "bg-accent/50" : "bg-line"}`} aria-hidden="true" />}
          </li>
        );
      })}
    </ol>
  );
}

/** A section that stays closed until asked for: keeps expert options out of the way. */
export function Disclosure({ title, subtitle, children, defaultOpen = false }: { title: string; subtitle?: string; children: ReactNode; defaultOpen?: boolean }) {
  return (
    <details open={defaultOpen} className="group card rounded-[var(--radius-card)] border border-line">
      <summary className="flex cursor-pointer list-none items-center gap-3 px-5 py-4 [&::-webkit-details-marker]:hidden">
        <Icon name="chevron" size={16} className="shrink-0 text-muted transition-transform group-open:rotate-90" />
        <span className="min-w-0 flex-1">
          <span className="block text-[15px] font-semibold tracking-tight">{title}</span>
          {subtitle && <span className="mt-0.5 block text-xs text-muted">{subtitle}</span>}
        </span>
      </summary>
      <div className="border-t border-line px-5 pb-5 pt-4">{children}</div>
    </details>
  );
}

/** A value with a copy button, e.g. an address to share. */
export function CopyChip({ text }: { text: string }) {
  const [copied, setCopied] = useState(false);
  return (
    <button
      type="button"
      onClick={() => navigator.clipboard?.writeText(text).then(() => { setCopied(true); setTimeout(() => setCopied(false), 1500); })}
      className="inline-flex items-center gap-2 rounded-[10px] border border-line-strong bg-sunken px-3 py-1.5 font-mono text-sm text-ink transition-colors hover:border-accent/60"
      title="Copy"
    >
      {text}
      <Icon name={copied ? "check" : "copy"} size={14} className={copied ? "text-ok" : "text-faint"} />
    </button>
  );
}

/** A certificate fingerprint laid out in groups for people to compare. */
export function FingerprintGrid({ hex }: { hex: string }) {
  return (
    <div className="grid grid-cols-4 gap-1.5 font-mono text-xs text-ink sm:grid-cols-8">
      {(hex.toUpperCase().match(/.{1,4}/g) ?? []).map((g, i) => (
        <span key={i} className="rounded-md border border-line-strong bg-sunken px-1.5 py-1 text-center">
          {g}
        </span>
      ))}
    </div>
  );
}
