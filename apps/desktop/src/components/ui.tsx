// Reusable, accessible building blocks.
import { useId, type ButtonHTMLAttributes, type ReactNode } from "react";
import { Icon, type IconName } from "./Icon";

type Variant = "primary" | "secondary" | "ghost" | "danger";

const VARIANTS: Record<Variant, string> = {
  primary: "bg-accent text-accent-ink hover:bg-accent-strong disabled:bg-line disabled:text-faint",
  secondary: "bg-raised text-ink border border-line hover:border-line-strong disabled:text-faint",
  ghost: "text-muted hover:text-ink hover:bg-raised disabled:text-faint",
  danger: "bg-bad/15 text-bad border border-bad/40 hover:bg-bad/25 disabled:opacity-50",
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
  const sizes = { sm: "h-8 px-3 text-[13px]", md: "h-10 px-4 text-sm", lg: "h-12 px-5 text-[15px]" };
  return (
    <button
      type="button"
      className={`inline-flex items-center justify-center gap-2 rounded-[10px] font-medium transition-colors disabled:cursor-not-allowed ${sizes[size]} ${VARIANTS[variant]} ${className}`}
      disabled={busy || rest.disabled}
      aria-busy={busy || undefined}
      {...rest}
    >
      {busy ? <Spinner /> : icon ? <Icon name={icon} size={size === "sm" ? 15 : 17} /> : null}
      {children}
    </button>
  );
}

export function Spinner({ size = 16 }: { size?: number }) {
  return (
    <span
      role="status"
      aria-label="Loading"
      className="inline-block animate-spin rounded-full border-2 border-current border-t-transparent"
      style={{ width: size, height: size }}
    />
  );
}

export function Card({ title, actions, children, className = "" }: { title?: ReactNode; actions?: ReactNode; children: ReactNode; className?: string }) {
  return (
    <section className={`rounded-[var(--radius-card)] border border-line bg-surface p-5 ${className}`}>
      {(title || actions) && (
        <header className="mb-4 flex items-center justify-between gap-3">
          {title && <h2 className="text-[15px] font-semibold tracking-tight">{title}</h2>}
          {actions && <div className="flex items-center gap-2">{actions}</div>}
        </header>
      )}
      {children}
    </section>
  );
}

type Tone = "neutral" | "accent" | "ok" | "warn" | "bad";
const TONES: Record<Tone, string> = {
  neutral: "bg-raised text-muted border-line",
  accent: "bg-accent/12 text-accent border-accent/30",
  ok: "bg-ok/12 text-ok border-ok/30",
  warn: "bg-warn/12 text-warn border-warn/30",
  bad: "bg-bad/12 text-bad border-bad/30",
};

export function Badge({ tone = "neutral", children }: { tone?: Tone; children: ReactNode }) {
  return <span className={`inline-flex items-center gap-1.5 rounded-full border px-2.5 py-0.5 text-xs font-medium ${TONES[tone]}`}>{children}</span>;
}

export function StatusDot({ tone, label }: { tone: Tone; label: string }) {
  const colors: Record<Tone, string> = { neutral: "bg-faint", accent: "bg-accent", ok: "bg-ok", warn: "bg-warn", bad: "bg-bad" };
  return (
    <span className="inline-flex items-center gap-2 text-sm text-muted">
      <span className={`h-2 w-2 rounded-full ${colors[tone]}`} aria-hidden="true" />
      {label}
    </span>
  );
}

export function Toggle({ checked, onChange, label, description, disabled }: { checked: boolean; onChange: (v: boolean) => void; label: string; description?: string; disabled?: boolean }) {
  const id = useId();
  return (
    <div className="flex items-center justify-between gap-6 py-2">
      <div>
        <label htmlFor={id} className="text-sm text-ink">
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
        className={`relative h-6 w-11 shrink-0 rounded-full border transition-colors disabled:opacity-50 ${checked ? "border-accent bg-accent" : "border-line-strong bg-raised"}`}
      >
        <span className={`absolute left-[2px] top-[2px] h-[18px] w-[18px] rounded-full bg-ink transition-transform ${checked ? "translate-x-5" : "translate-x-0"}`} />
      </button>
    </div>
  );
}

export function Field({ label, hint, error, children }: { label: string; hint?: string; error?: string | null; children: (id: string) => ReactNode }) {
  const id = useId();
  return (
    <div className="flex flex-col gap-1.5">
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

const inputCls = "h-10 w-full rounded-[10px] border border-line bg-bg px-3 text-sm text-ink placeholder:text-faint focus:border-accent focus:outline-none";

export function TextInput(props: React.InputHTMLAttributes<HTMLInputElement>) {
  return <input {...props} className={`${inputCls} ${props.className ?? ""}`} />;
}

export function NumberInput({ value, onChange, min, max, step, id }: { value: number; onChange: (v: number) => void; min?: number; max?: number; step?: number; id?: string }) {
  return (
    <input
      id={id}
      type="number"
      className={inputCls}
      value={Number.isFinite(value) ? value : ""}
      min={min}
      max={max}
      step={step}
      onChange={(e) => onChange(Number(e.target.value))}
    />
  );
}

const CHEVRON = `url("data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 24 24' fill='none' stroke='%238a97a6' stroke-width='2' stroke-linecap='round' stroke-linejoin='round'%3E%3Cpath d='M6 9l6 6 6-6'/%3E%3C/svg%3E")`;

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

export function Slider({ id, value, onChange, min, max, step, format }: { id?: string; value: number; onChange: (v: number) => void; min: number; max: number; step: number; format?: (v: number) => string }) {
  return (
    <div className="flex items-center gap-3">
      <input id={id} type="range" min={min} max={max} step={step} value={value} onChange={(e) => onChange(Number(e.target.value))} className="w-full accent-[var(--color-accent)]" />
      <span className="w-16 text-right font-mono text-xs text-muted">{format ? format(value) : value}</span>
    </div>
  );
}

export function Stat({ label, value, hint }: { label: string; value: ReactNode; hint?: string }) {
  return (
    <div className="rounded-xl border border-line bg-bg px-4 py-3" title={hint}>
      <div className="text-xs text-faint">{label}</div>
      <div className="mt-1 font-mono text-[15px] text-ink">{value}</div>
    </div>
  );
}

export function Banner({ tone, title, children, action }: { tone: "info" | "warn" | "error"; title: string; children?: ReactNode; action?: ReactNode }) {
  const map = {
    info: "border-accent/30 bg-accent/8 text-ink",
    warn: "border-warn/40 bg-warn/8 text-ink",
    error: "border-bad/40 bg-bad/8 text-ink",
  };
  return (
    <div role={tone === "error" ? "alert" : "status"} className={`fade-in flex items-start gap-3 rounded-xl border p-4 ${map[tone]}`}>
      <Icon name={tone === "info" ? "signal" : "alert"} className={tone === "error" ? "text-bad" : tone === "warn" ? "text-warn" : "text-accent"} />
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
    <div className="flex flex-col items-center justify-center gap-2 rounded-xl border border-dashed border-line px-6 py-10 text-center">
      <Icon name={icon} size={26} className="text-faint" />
      <div className="text-sm font-medium text-ink">{title}</div>
      {children && <div className="max-w-md text-sm text-muted">{children}</div>}
    </div>
  );
}

export function PageHeader({ title, subtitle, actions }: { title: string; subtitle?: ReactNode; actions?: ReactNode }) {
  return (
    <div className="mb-6 flex flex-wrap items-end justify-between gap-4">
      <div>
        <h1 className="text-2xl font-semibold tracking-tight">{title}</h1>
        {subtitle && <p className="mt-1 text-sm text-muted">{subtitle}</p>}
      </div>
      {actions && <div className="flex items-center gap-2">{actions}</div>}
    </div>
  );
}

export function Kbd({ children }: { children: ReactNode }) {
  return <kbd className="rounded-md border border-line-strong bg-raised px-1.5 py-0.5 font-mono text-[11px] text-muted">{children}</kbd>;
}
