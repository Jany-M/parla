import type {
  AppInfo, Devices, Lang, MeetingDetail, MeetingListItem, ModelProgress, ModelStatus, ParlaEvent, Settings, SummaryProgress,
  UpdateInfo, UpdateProgress,
} from "./types";

/** Everything the UI can ask the backend for. One implementation talks to Tauri, one is in-memory. */
export interface Api {
  appInfo(): Promise<AppInfo>;
  getSettings(): Promise<Settings>;
  saveSettings(s: Settings): Promise<void>;
  listDevices(): Promise<Devices>;
  listLanguages(): Promise<Lang[]>;
  modelStatus(): Promise<ModelStatus[]>;
  installModel(id: string): Promise<void>;
  cancelInstall(): Promise<void>;
  uninstallModel(id: string): Promise<void>;
  deleteAllData(): Promise<void>;
  startLive(): Promise<number>;
  stopLive(): Promise<number | null>;
  recordingState(): Promise<number | null>;
  openLogs(): Promise<void>;
  openUrl(url: string): Promise<void>;
  logUiError(message: string): Promise<void>;
  checkForUpdate(): Promise<UpdateInfo | null>;
  installUpdate(): Promise<void>;
  setPaused(paused: boolean): Promise<boolean>;
  isPaused(): Promise<boolean>;
  startTranslation(target: string): Promise<void>;
  stopTranslation(): Promise<void>;
  translationTarget(): Promise<string | null>;
  importFile(path: string): Promise<number>;
  cancelImport(): Promise<void>;
  listMeetings(): Promise<MeetingListItem[]>;
  getMeeting(id: number): Promise<MeetingDetail | null>;
  renameMeeting(id: number, title: string): Promise<void>;
  setNotes(id: number, notes: string): Promise<void>;
  deleteMeeting(id: number): Promise<void>;
  renameSpeaker(id: number, name: string): Promise<void>;
  mergeSpeakers(from: number, into: number): Promise<void>;
  summarize(id: number): Promise<string>;
  exportMarkdown(id: number, dateLabel: string): Promise<string>;
  saveText(path: string, text: string): Promise<void>;
  showOverlay(show: boolean): Promise<void>;
  /** Begin moving the current window (used by the translation overlay). */
  startDragging(): Promise<void>;
  onEvent(cb: (e: ParlaEvent) => void): Promise<() => void>;
  onModelProgress(cb: (p: ModelProgress) => void): Promise<() => void>;
  onSummaryProgress(cb: (p: SummaryProgress) => void): Promise<() => void>;
  onUpdateProgress(cb: (p: UpdateProgress) => void): Promise<() => void>;
  pickAudioFile(): Promise<string | null>;
  pickSavePath(defaultName: string): Promise<string | null>;
  pickFolder(): Promise<string | null>;
}

export const isTauri = () => typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

async function realApi(): Promise<Api> {
  const { invoke } = await import("@tauri-apps/api/core");
  const { listen } = await import("@tauri-apps/api/event");
  const dialog = await import("@tauri-apps/plugin-dialog");
  const on = <T,>(name: string) => (cb: (p: T) => void) => listen<T>(name, (e) => cb(e.payload));
  return {
    appInfo: () => invoke("app_info"),
    getSettings: () => invoke("get_settings"),
    saveSettings: (settings) => invoke("save_settings", { settings }),
    listDevices: () => invoke("list_devices"),
    listLanguages: () => invoke("list_languages"),
    modelStatus: () => invoke("model_status"),
    installModel: (id) => invoke("install_model", { id }),
    cancelInstall: () => invoke("cancel_install"),
    uninstallModel: (id) => invoke("uninstall_model", { id }),
    deleteAllData: () => invoke("delete_all_data"),
    startLive: () => invoke("start_live"),
    stopLive: () => invoke("stop_live"),
    recordingState: () => invoke("recording_state"),
    openLogs: () => invoke("open_logs"),
    openUrl: (url) => invoke("open_url", { url }),
    logUiError: (message) => invoke("log_ui_error", { message }),
    checkForUpdate: () => invoke("check_for_update"),
    installUpdate: () => invoke("install_update"),
    setPaused: (paused) => invoke("set_paused", { paused }),
    isPaused: () => invoke("is_paused"),
    startTranslation: (target) => invoke("start_translation", { target }),
    stopTranslation: () => invoke("stop_translation"),
    translationTarget: () => invoke("translation_target"),
    importFile: (path) => invoke("import_file", { path }),
    cancelImport: () => invoke("cancel_import"),
    listMeetings: () => invoke("list_meetings"),
    getMeeting: (id) => invoke("get_meeting", { id }),
    renameMeeting: (id, title) => invoke("rename_meeting", { id, title }),
    setNotes: (id, notes) => invoke("set_notes", { id, notes }),
    deleteMeeting: (id) => invoke("delete_meeting", { id }),
    renameSpeaker: (id, name) => invoke("rename_speaker", { id, name }),
    mergeSpeakers: (from, into) => invoke("merge_speakers", { from, into }),
    summarize: (id) => invoke("summarize", { id }),
    exportMarkdown: (id, dateLabel) => invoke("export_markdown", { id, dateLabel }),
    saveText: (path, text) => invoke("save_text", { path, text }),
    showOverlay: (show) => invoke("show_overlay", { show }),
    startDragging: async () => {
      const { getCurrentWindow } = await import("@tauri-apps/api/window");
      await getCurrentWindow().startDragging();
    },
    onEvent: on<ParlaEvent>("parla-event"),
    onModelProgress: on<ModelProgress>("model-progress"),
    onSummaryProgress: on<SummaryProgress>("summary-progress"),
    onUpdateProgress: on<UpdateProgress>("update-progress"),
    pickAudioFile: async () => {
      const r = await dialog.open({
        multiple: false,
        filters: [{ name: "Audio / video", extensions: ["wav", "mp3", "m4a", "aac", "flac", "ogg", "mp4", "mkv", "mov", "webm", "wma"] }],
      });
      return typeof r === "string" ? r : null;
    },
    pickSavePath: async (name) => (await dialog.save({ defaultPath: name, filters: [{ name: "Markdown", extensions: ["md"] }] })) ?? null,
    pickFolder: async () => {
      const r = await dialog.open({ directory: true, multiple: false });
      return typeof r === "string" ? r : null;
    },
  };
}

let current: Api | null = null;
let loading: Promise<Api> | null = null;

/** Tests and the browser preview install the in-memory backend with `setApi`. */
export function setApi(a: Api | null) {
  current = a;
  loading = null;
}

export async function getApi(): Promise<Api> {
  if (current) return current;
  loading ??= (isTauri() ? realApi() : import("./mockApi").then((m) => m.createMockApi())).then((a) => (current = a));
  return loading;
}

/** Synchronous accessor for components (the app awaits `getApi()` before rendering). */
export function api(): Api {
  if (!current) throw new Error("backend not ready");
  return current;
}
