import type { ButtonHTMLAttributes, ReactNode, SelectHTMLAttributes } from "react";

type Variant = "primary" | "secondary" | "danger" | "ghost";
const VARIANTS: Record<Variant, string> = {
  primary: "bg-accent-strong hover:bg-indigo-500 text-white",
  secondary: "bg-raised hover:bg-[#232838] text-slate-100 border border-line",
  danger: "bg-red-600/90 hover:bg-red-500 text-white",
  ghost: "hover:bg-raised text-mute hover:text-slate-100",
};

export function Button({ variant = "secondary", className = "", ...p }: ButtonHTMLAttributes<HTMLButtonElement> & { variant?: Variant }) {
  return <button {...p} className={`inline-flex items-center justify-center gap-2 rounded-lg px-3.5 py-2 text-sm font-medium transition disabled:cursor-not-allowed disabled:opacity-50 ${VARIANTS[variant]} ${className}`} />;
}

export function Card({ title, action, children, className = "" }: { title?: string; action?: ReactNode; children: ReactNode; className?: string }) {
  return (
    <section className={`rounded-xl border border-line bg-panel p-5 ${className}`}>
      {(title || action) && (
        <header className="mb-3 flex items-center justify-between">
          {title && <h2 className="text-sm font-semibold tracking-wide text-slate-200">{title}</h2>}
          {action}
        </header>
      )}
      {children}
    </section>
  );
}

export function Select({ className = "", ...p }: SelectHTMLAttributes<HTMLSelectElement>) {
  return <select {...p} className={`rounded-lg border border-line bg-raised px-3 py-2 text-sm text-slate-100 outline-none focus:border-accent ${className}`} />;
}

export function Toggle({ checked, onChange, label }: { checked: boolean; onChange: (v: boolean) => void; label: string }) {
  return (
    <label className="flex cursor-pointer items-center gap-3 text-sm">
      <input type="checkbox" role="switch" aria-label={label} checked={checked} onChange={(e) => onChange(e.target.checked)} className="peer sr-only" />
      <span className="relative h-5 w-9 rounded-full bg-line transition peer-checked:bg-accent-strong after:absolute after:left-0.5 after:top-0.5 after:h-4 after:w-4 after:rounded-full after:bg-white after:transition peer-checked:after:translate-x-4" />
      <span>{label}</span>
    </label>
  );
}

export function ProgressBar({ value, label }: { value: number; label?: string }) {
  const pct = Math.round(Math.min(1, Math.max(0, value)) * 100);
  return (
    <div className="w-full" role="progressbar" aria-valuenow={pct} aria-valuemin={0} aria-valuemax={100} aria-label={label}>
      <div className="h-2 overflow-hidden rounded-full bg-line">
        <div className="h-full rounded-full bg-accent transition-all" style={{ width: `${pct}%` }} />
      </div>
    </div>
  );
}

export function Banner({ kind = "error", children, onClose }: { kind?: "error" | "info" | "warn"; children: ReactNode; onClose?: () => void }) {
  const c = { error: "border-red-500/40 bg-red-500/10 text-red-200", warn: "border-amber-500/40 bg-amber-500/10 text-amber-200", info: "border-accent/40 bg-accent/10 text-indigo-100" }[kind];
  return (
    <div role={kind === "error" ? "alert" : "status"} className={`flex items-start justify-between gap-3 rounded-lg border px-3.5 py-2.5 text-sm ${c}`}>
      <span>{children}</span>
      {onClose && <button onClick={onClose} aria-label="Dismiss" className="opacity-70 hover:opacity-100">✕</button>}
    </div>
  );
}

export function LevelMeter({ label, level }: { label: string; level: number }) {
  const pct = Math.min(100, Math.round(Math.sqrt(Math.min(1, level * 4)) * 100));
  return (
    <div className="flex items-center gap-2 text-xs text-mute">
      <span className="w-14">{label}</span>
      <div className="h-1.5 w-28 overflow-hidden rounded-full bg-line"><div className="h-full rounded-full bg-emerald-400 transition-all duration-100" style={{ width: `${pct}%` }} /></div>
    </div>
  );
}
