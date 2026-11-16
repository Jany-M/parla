// Mirrors the Rust types in parla-core (serde JSON). Keep in sync with crates/parla-core/src/types.rs.
export type StreamKind = "mic" | "system" | "file";

export interface Speaker { id: number; meeting_id: number; label: string; name: string | null; is_me: boolean }

export interface Segment {
  id: number; meeting_id: number; stream: StreamKind; speaker_id: number | null;
  t0: number; t1: number; lang: string | null; text: string;
  translation: string | null; translation_lang: string | null;
}

export interface MeetingInfo { id: number; title: string; started_at: number; ended_at: number | null; source: string; notes: string; summary: string }
export interface MeetingListItem { id: number; title: string; started_at: number; ended_at: number | null; source: string; segment_count: number }
export interface MeetingDetail { meeting: MeetingInfo; speakers: Speaker[]; segments: Segment[] }

export type ParlaEvent =
  | { type: "segment"; segment: Segment }
  | { type: "translation"; segment_id: number; text: string; lang: string }
  | { type: "level"; stream: StreamKind; level: number }
  | { type: "progress"; meeting_id: number; fraction: number }
  | { type: "warning"; message: string }
  | { type: "stopped"; meeting_id: number; saved: boolean };

export type SlotChoice =
  | { kind: "managed"; item_id: string | null }
  | { kind: "local_file"; path: string }
  | { kind: "endpoint"; url: string; model: string; api_key: string | null };

export interface Settings {
  slots: { stt: SlotChoice; speaker: SlotChoice; translate: SlotChoice; summarize: SlotChoice };
  target_lang: string; mic_device: string | null; system_device: string | null;
  echo_guard: boolean; onboarding_done: boolean; overlay_font_px: number; overlay_opacity: number; summary_lang: string;
}

export type ModelSlot = "stt" | "vad" | "speaker" | "translate" | "summarize" | "runtime";
export interface ModelStatus {
  id: string; slot: ModelSlot; label: string; description: string; license: string;
  size_bytes: number; installed: boolean; recommended: boolean;
  /** False when this machine lacks the RAM (or other gates) to download/use the model. */
  eligible: boolean;
}
export interface ModelProgress { id: string; done: number; total: number }
export interface SummaryProgress { meeting_id: number; done: number; total: number }

export interface DeviceInfo { id: string; name: string; is_default: boolean }
export interface Devices { inputs: DeviceInfo[]; outputs: DeviceInfo[] }
export interface Lang { code: string; name: string }

export interface SystemInfo {
  hardware: { total_ram_mb: number; cpu_threads: number; has_avx2: boolean };
  recommendation: { tier: string; summarize_item: string; translate_item: string; threads: number; warning: string | null };
  disk_usage_bytes: number; data_dir: string;
}
export interface AppInfo { version: string; system: SystemInfo }

/** Newer GitHub Release available for this install (Windows installer). */
export interface UpdateInfo {
  version: string;
  tag: string;
  asset_name: string;
  size: number;
}
export interface UpdateProgress { done: number; total: number }
