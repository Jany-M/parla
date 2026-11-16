import type { Segment } from "./types";

export interface OverlayLine { id: number; text: string; fresh: boolean }

/** The last `n` translated sentences; the newest is flagged so the UI can emphasise it. */
export function overlayLines(segments: Segment[], n = 2): OverlayLine[] {
  const translated = segments.filter((s) => s.translation && s.stream !== "mic").slice(-n);
  return translated.map((s, i) => ({ id: s.id, text: s.translation as string, fresh: i === translated.length - 1 }));
}
