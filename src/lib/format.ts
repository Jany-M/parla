export function formatBytes(n: number): string {
  if (n < 1_000_000) return `${Math.max(1, Math.round(n / 1000))} KB`;
  if (n < 1_000_000_000) return `${Math.round(n / 1_000_000)} MB`;
  return `${(n / 1_000_000_000).toFixed(1)} GB`;
}

/** Seconds → "mm:ss" or "h:mm:ss" (same as the Rust side so exports match the screen). */
export function formatClock(secs: number): string {
  const s = Math.max(0, Math.floor(secs));
  const [h, m, r] = [Math.floor(s / 3600), Math.floor((s % 3600) / 60), s % 60];
  const mm = String(m).padStart(2, "0"), ss = String(r).padStart(2, "0");
  return h > 0 ? `${h}:${mm}:${ss}` : `${mm}:${ss}`;
}

export function formatDuration(startedAt: number, endedAt: number | null): string {
  return endedAt ? formatClock(endedAt - startedAt) : "recording…";
}

/** Unix seconds → local date and time. */
export function formatDate(unixSecs: number, locale?: string): string {
  return new Date(unixSecs * 1000).toLocaleString(locale, { dateStyle: "medium", timeStyle: "short" });
}

export function fileSafe(name: string): string {
  return name.replace(/[\\/:*?"<>|]+/g, "_").trim() || "meeting";
}
