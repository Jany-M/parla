import type { ParlaEvent, Segment, Speaker, StreamKind } from "./types";

export interface LiveState {
  segments: Segment[];
  levels: Record<StreamKind, number>;
  warnings: string[];
}

export const emptyLive = (): LiveState => ({ segments: [], levels: { mic: 0, system: 0, file: 0 }, warnings: [] });

/** Pure reducer for the event stream — the one place live transcript state changes. */
export function reduceLive(state: LiveState, e: ParlaEvent | { type: "reset" }): LiveState {
  switch (e.type) {
    case "reset":
      return emptyLive();
    case "segment": {
      if (state.segments.some((s) => s.id === e.segment.id)) return state;
      // Sort by meeting clock so a delayed mic commit (echo hold) still lands in time order.
      const segments = [...state.segments, e.segment].sort((a, b) => a.t0 - b.t0 || a.id - b.id);
      return { ...state, segments };
    }
    case "translation":
      return { ...state, segments: state.segments.map((s) => (s.id === e.segment_id ? { ...s, translation: e.text, translation_lang: e.lang } : s)) };
    case "level": {
      // fast attack, slow release: meters that jump up and fall smoothly
      const prev = state.levels[e.stream];
      const level = e.level > prev ? e.level : prev * 0.8 + e.level * 0.2;
      return { ...state, levels: { ...state.levels, [e.stream]: level } };
    }
    case "warning":
      return state.warnings.includes(e.message) ? state : { ...state, warnings: [...state.warnings.slice(-4), e.message] };
    default:
      return state;
  }
}

export interface Turn {
  key: string;
  speakerId: number | null;
  stream: StreamKind;
  t0: number;
  segments: Segment[];
}

/** Merges consecutive segments of the same speaker (within `maxGap` seconds) into one "turn". */
export function groupTurns(segments: Segment[], maxGap = 8): Turn[] {
  const turns: Turn[] = [];
  for (const s of segments) {
    const last = turns[turns.length - 1];
    const lastEnd = last ? last.segments[last.segments.length - 1].t1 : 0;
    if (last && last.speakerId === s.speaker_id && last.stream === s.stream && s.t0 - lastEnd <= maxGap) {
      last.segments.push(s);
    } else {
      turns.push({ key: `${s.id}`, speakerId: s.speaker_id, stream: s.stream, t0: s.t0, segments: [s] });
    }
  }
  return turns;
}

export function speakerName(speakers: Speaker[], id: number | null): string {
  const s = speakers.find((x) => x.id === id);
  if (!s) return "Speaker";
  return s.name?.trim() ? s.name : s.label;
}

const PALETTE = ["#60a5fa", "#f472b6", "#34d399", "#fbbf24", "#a78bfa", "#fb923c", "#22d3ee", "#f87171"];
/** Stable colour per speaker; "Me" is always the accent colour. */
export function speakerColor(speakers: Speaker[], id: number | null): string {
  const s = speakers.find((x) => x.id === id);
  if (s?.is_me) return "#818cf8";
  const idx = speakers.filter((x) => !x.is_me).findIndex((x) => x.id === id);
  return PALETTE[(idx < 0 ? 0 : idx) % PALETTE.length];
}

/** Segment ids whose speaker is not in `known` — the live view then reloads the speaker list. */
export function unknownSpeakers(segments: Segment[], known: Speaker[]): boolean {
  return segments.some((s) => s.speaker_id !== null && !known.some((k) => k.id === s.speaker_id));
}
