import { createContext, useCallback, useContext, useEffect, useState, type ReactNode } from "react";
import { api } from "./lib/api";
import type { AppInfo, Lang, ModelStatus, Settings } from "./lib/types";

export interface AppCtx {
  settings: Settings;
  models: ModelStatus[];
  languages: Lang[];
  info: AppInfo;
  save: (s: Settings) => Promise<void>;
  refreshModels: () => Promise<void>;
  refreshInfo: () => Promise<void>;
}

const Ctx = createContext<AppCtx | null>(null);
export const useApp = () => {
  const c = useContext(Ctx);
  if (!c) throw new Error("useApp outside provider");
  return c;
};

export function AppProvider({ children }: { children: (ctx: AppCtx) => ReactNode }) {
  const [state, setState] = useState<{ settings: Settings; models: ModelStatus[]; languages: Lang[]; info: AppInfo } | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    const a = api();
    Promise.all([a.getSettings(), a.modelStatus(), a.listLanguages(), a.appInfo()])
      .then(([settings, models, languages, info]) => setState({ settings, models, languages, info }))
      .catch((e) => setError(String(e)));
  }, []);

  const save = useCallback(async (settings: Settings) => {
    await api().saveSettings(settings);
    setState((s) => (s ? { ...s, settings } : s));
  }, []);
  const refreshModels = useCallback(async () => {
    const models = await api().modelStatus();
    setState((s) => (s ? { ...s, models } : s));
  }, []);
  const refreshInfo = useCallback(async () => {
    const info = await api().appInfo();
    setState((s) => (s ? { ...s, info } : s));
  }, []);

  if (error) return <div className="p-8 text-red-300">Parla could not start: {error}</div>;
  if (!state) return <div className="grid h-full place-items-center text-mute">Loading…</div>;
  const ctx: AppCtx = { ...state, save, refreshModels, refreshInfo };
  return <Ctx.Provider value={ctx}>{children(ctx)}</Ctx.Provider>;
}
