import type { ModelProgress, ModelStatus } from "./types";

export type Feature = "translate" | "summarize";

/** What the app cannot work without: speech, voice activity, speaker recognition. */
export const essentials = (m: ModelStatus[]) => m.filter((x) => ["stt", "vad", "speaker"].includes(x.slot) && x.recommended);

/** Models for an optional feature: its recommended model for this machine, plus the shared runtime. */
export function featureItems(m: ModelStatus[], f: Feature): ModelStatus[] {
  return m.filter((x) => (x.slot === f && x.recommended && x.eligible) || x.slot === "runtime");
}

/** Eligible managed options for a slot (e.g. both summary sizes on 16 GB+). */
export function eligibleForSlot(m: ModelStatus[], slot: ModelStatus["slot"]): ModelStatus[] {
  return m.filter((x) => x.slot === slot && x.eligible);
}

export const missing = (items: ModelStatus[]) => items.filter((x) => !x.installed);
export const totalBytes = (items: ModelStatus[]) => items.reduce((a, b) => a + b.size_bytes, 0);
export const isReady = (m: ModelStatus[]) => essentials(m).every((x) => x.installed);

/** Overall 0..1 for a batch download (finished items count fully). */
export function batchProgress(items: ModelStatus[], current: ModelProgress | null): number {
  const total = totalBytes(items);
  if (total === 0) return 1;
  const done = items.filter((x) => x.installed).reduce((a, b) => a + b.size_bytes, 0) + (current && !items.find((x) => x.id === current.id)?.installed ? current.done : 0);
  return Math.min(1, done / total);
}

/** Loopback hosts keep your text on this computer; anything else leaves it (mirrors the Rust check). */
export function isLocalUrl(url: string): boolean {
  const rest = url.includes("://") ? url.split("://")[1] : url;
  const authority = (rest.split(/[/?#]/)[0] ?? "").split("@").pop() ?? "";
  const host = authority.startsWith("[") ? authority.slice(1).split("]")[0] : authority.split(":")[0];
  return host.toLowerCase() === "localhost" || host === "::1" || host.startsWith("127.");
}

export type SourceKind = "managed" | "local_file" | "endpoint";
export const SLOT_SOURCES: Record<"stt" | "speaker" | "translate" | "summarize", SourceKind[]> = {
  stt: ["managed", "local_file"],
  speaker: ["managed", "local_file"],
  translate: ["managed", "local_file", "endpoint"],
  summarize: ["managed", "local_file", "endpoint"],
};
