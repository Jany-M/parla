import type { Api } from "./api";
import type { MeetingDetail, ModelProgress, ModelStatus, ParlaEvent, Segment, Settings, SummaryProgress } from "./types";

export const defaultSettings = (): Settings => ({
  slots: {
    stt: { kind: "managed", item_id: null }, speaker: { kind: "managed", item_id: null },
    translate: { kind: "managed", item_id: null }, summarize: { kind: "managed", item_id: null },
  },
  target_lang: "en", mic_device: null, system_device: null, echo_guard: true, onboarding_done: false,
  overlay_font_px: 22, overlay_opacity: 0.85, summary_lang: "en",
});

const model = (id: string, slot: ModelStatus["slot"], label: string, mb: number, installed = false): ModelStatus => ({
  id, slot, label, description: `${label} (demo)`, license: "Apache-2.0", size_bytes: mb * 1_000_000, installed, recommended: true,
});

/** In-memory backend: powers `npm run dev` in a browser and every UI test. */
export function createMockApi(opts: { installed?: boolean } = {}) {
  let settings = defaultSettings();
  let models: ModelStatus[] = [
    model("stt-parakeet", "stt", "Speech recognition", 660, opts.installed),
    model("vad-silero", "vad", "Voice activity", 1, opts.installed),
    model("spk-wespeaker", "speaker", "Speaker recognition", 27, opts.installed),
    model("mt-hy-mt2", "translate", "Translation", 1130, opts.installed),
    model("sum-qwen-2b", "summarize", "Summaries", 1280, opts.installed),
    model("rt-llama", "runtime", "Language-model runtime", 25, opts.installed),
  ];
  let meetings: MeetingDetail[] = [];
  let recording: number | null = null;
  let target: string | null = null;
  let paused = false;
  const listeners = new Set<(e: ParlaEvent) => void>();
  const progress = new Set<(p: ModelProgress) => void>();
  const sprogress = new Set<(p: SummaryProgress) => void>();
  let nextId = 1;
  const calls: string[] = [];
  const track = <T,>(name: string, v: T): Promise<T> => (calls.push(name), Promise.resolve(v));
  const sub = <T,>(set: Set<(x: T) => void>) => (cb: (x: T) => void) => (set.add(cb), Promise.resolve(() => void set.delete(cb)));

  const api: Api & { emit(e: ParlaEvent): void; calls: string[]; meetings: MeetingDetail[]; models: ModelStatus[]; addSegment(s: Partial<Segment>): Segment } = {
    calls, meetings, models,
    emit: (e) => listeners.forEach((l) => l(e)),
    addSegment(partial) {
      const seg: Segment = { id: nextId++, meeting_id: recording ?? 1, stream: "system", speaker_id: 1, t0: 0, t1: 2, lang: "en", text: "hello", translation: null, translation_lang: null, ...partial };
      api.emit({ type: "segment", segment: seg });
      return seg;
    },
    appInfo: () => track("appInfo", { version: "0.1.0", system: { hardware: { total_ram_mb: 8000, cpu_threads: 8, has_avx2: true }, recommendation: { tier: "8gb", summarize_item: "sum-qwen-2b", translate_item: "mt-hy-mt2", threads: 4, warning: null }, disk_usage_bytes: 0, data_dir: "C:\\Users\\you\\AppData\\Local\\Parla" } }),
    getSettings: () => track("getSettings", structuredClone(settings)),
    saveSettings: (s) => ((settings = structuredClone(s)), track("saveSettings", undefined)),
    listDevices: () => track("listDevices", { inputs: [{ id: "mic1", name: "Built-in microphone", is_default: true }], outputs: [{ id: "spk1", name: "Speakers", is_default: true }] }),
    listLanguages: () => track("listLanguages", [{ code: "en", name: "English" }, { code: "it", name: "Italian" }, { code: "fr", name: "French" }]),
    modelStatus: () => track("modelStatus", structuredClone(models)),
    installModel: async (id) => {
      calls.push(`install:${id}`);
      const m = models.find((x) => x.id === id)!;
      progress.forEach((p) => p({ id, done: m.size_bytes / 2, total: m.size_bytes }));
      progress.forEach((p) => p({ id, done: m.size_bytes, total: m.size_bytes }));
      m.installed = true;
    },
    cancelInstall: () => track("cancelInstall", undefined),
    uninstallModel: async (id) => {
      calls.push(`uninstall:${id}`);
      models.find((x) => x.id === id)!.installed = false;
    },
    deleteAllData: async () => {
      calls.push("deleteAllData");
      meetings.length = 0;
      models.forEach((m) => (m.installed = false));
      settings = defaultSettings();
    },
    startLive: async () => {
      calls.push("startLive");
      recording = nextId++;
      return recording;
    },
    stopLive: async () => {
      calls.push("stopLive");
      const id = recording;
      recording = null;
      target = null;
      paused = false;
      return id;
    },
    recordingState: () => track("recordingState", recording),
    openLogs: () => track("openLogs", undefined),
    logUiError: (m) => track(`logUiError:${m}`, undefined),
    setPaused: (p) => { paused = p; return track(`setPaused:${p}`, recording !== null); },
    isPaused: () => track("isPaused", paused),
    startTranslation: async (t) => (calls.push(`startTranslation:${t}`), void (target = t)),
    stopTranslation: async () => (calls.push("stopTranslation"), void (target = null)),
    translationTarget: () => track("translationTarget", target),
    importFile: async (p) => {
      calls.push(`import:${p}`);
      const id = nextId++;
      meetings.push({ meeting: { id, title: "Imported", started_at: 1, ended_at: 2, source: "file", notes: "", summary: "" }, speakers: [], segments: [] });
      return id;
    },
    cancelImport: () => track("cancelImport", undefined),
    listMeetings: () => track("listMeetings", meetings.map((d) => ({ id: d.meeting.id, title: d.meeting.title, started_at: d.meeting.started_at, ended_at: d.meeting.ended_at, source: d.meeting.source, segment_count: d.segments.length }))),
    getMeeting: (id) => track("getMeeting", structuredClone(meetings.find((d) => d.meeting.id === id) ?? null)),
    renameMeeting: async (id, title) => void (meetings.find((d) => d.meeting.id === id)!.meeting.title = title),
    setNotes: async (id, notes) => void (meetings.find((d) => d.meeting.id === id)!.meeting.notes = notes),
    deleteMeeting: async (id) => void (meetings = meetings.filter((d) => d.meeting.id !== id)),
    renameSpeaker: async (id, name) => void meetings.forEach((d) => d.speakers.forEach((s) => s.id === id && (s.name = name))),
    mergeSpeakers: async (from, into) => (calls.push(`merge:${from}->${into}`), undefined),
    summarize: async (id) => {
      calls.push(`summarize:${id}`);
      sprogress.forEach((p) => p({ meeting_id: id, done: 1, total: 1 }));
      const text = "## Summary\nDemo summary";
      meetings.find((d) => d.meeting.id === id)!.meeting.summary = text;
      return text;
    },
    exportMarkdown: async (id) => `# Meeting ${id}`,
    saveText: (p, t) => track(`saveText:${p}:${t.length}`, undefined),
    showOverlay: (s) => track(`overlay:${s}`, undefined),
    onEvent: sub(listeners),
    onModelProgress: sub(progress),
    onSummaryProgress: sub(sprogress),
    pickAudioFile: () => track("pickAudioFile", "C:\\audio\\call.wav"),
    pickSavePath: (n) => track("pickSavePath", `C:\\out\\${n}`),
    pickFolder: () => track("pickFolder", "C:\\models\\mine"),
  };
  return api;
}
export type MockApi = ReturnType<typeof createMockApi>;
