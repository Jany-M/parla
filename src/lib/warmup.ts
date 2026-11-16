const KEY = "parla.startup.ms";

/** How long the last start took on this machine (loading and warming up the speech models). */
export function readEstimate(): number | null {
  try {
    const v = Number(window.localStorage.getItem(KEY));
    return Number.isFinite(v) && v > 0 ? v : null;
  } catch {
    return null;
  }
}

export function saveEstimate(ms: number): void {
  try {
    window.localStorage.setItem(KEY, String(Math.round(ms)));
  } catch {
    /* storage is only a convenience */
  }
}

/** Text for the "starting up" notice: a countdown when the last duration is known, else a timer. */
export function startupMessage(estimateMs: number | null, elapsedMs: number): string {
  const elapsed = Math.floor(elapsedMs / 1000);
  if (estimateMs === null) return `Starting up, loading the speech models (${elapsed} s). Wait before you speak.`;
  const left = Math.ceil((estimateMs - elapsedMs) / 1000);
  return left > 0
    ? `Starting up, about ${left} s left. Wait before you speak.`
    : `Almost ready, still loading (${elapsed} s). Wait before you speak.`;
}
