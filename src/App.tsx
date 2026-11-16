import { useEffect, useState } from "react";
import { AppProvider } from "./context";
import { api } from "./lib/api";
import type { UpdateInfo } from "./lib/types";
import { Library } from "./views/Library";
import { Record } from "./views/Record";
import { Settings } from "./views/Settings";
import { Wizard } from "./views/Wizard";

type View = "record" | "library" | "settings";
const NAV: { id: View; label: string; icon: string }[] = [
  { id: "record", label: "Record", icon: "●" },
  { id: "library", label: "Library", icon: "☰" },
  { id: "settings", label: "Settings", icon: "⚙" },
];

function SidebarFooter({ version }: { version: string }) {
  const [update, setUpdate] = useState<UpdateInfo | null>(null);
  const [busy, setBusy] = useState(false);
  const [pct, setPct] = useState<number | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let off = () => {};
    void api()
      .checkForUpdate()
      .then(setUpdate)
      .catch(() => setUpdate(null));
    void api()
      .onUpdateProgress((p) => {
        if (p.total > 0) setPct(Math.min(100, Math.round((100 * p.done) / p.total)));
      })
      .then((u) => {
        off = u;
      });
    return () => off();
  }, []);

  const label = busy
    ? pct != null
      ? `Updating… ${pct}%`
      : "Updating…"
    : update
      ? `Update to v${update.version}`
      : null;

  const link = "underline decoration-dotted hover:text-slate-100";

  return (
    <div className="mt-auto flex flex-col gap-1">
      {update && (
        <div className="px-0 pb-1">
          <button
            type="button"
            disabled={busy}
            onClick={() => {
              setBusy(true);
              setError(null);
              setPct(null);
              void api()
                .installUpdate()
                .catch((e) => {
                  setBusy(false);
                  setPct(null);
                  setError(String(e));
                });
            }}
            className="w-full rounded-lg bg-accent/20 px-3 py-2 text-left text-xs font-medium text-accent hover:bg-accent/30 disabled:opacity-60"
          >
            {label}
          </button>
          {error && <p className="mt-1 px-1 text-[10px] leading-snug text-red-300">{error}</p>}
        </div>
      )}
      <div className="px-3 text-[10px] leading-snug text-mute">
        <span className="block">Developed by</span>
        <span className="flex flex-wrap items-center gap-x-1 gap-y-0.5">
          <button type="button" className={link} onClick={() => void api().openUrl("https://github.com/Jany-M")}>
            Jany Martelli
          </button>
          <span>@</span>
          <button type="button" className={link} onClick={() => void api().openUrl("https://www.shambix.com")}>
            Shambix
          </button>
          <button
            type="button"
            aria-label="Parla on GitHub"
            title="Parla on GitHub"
            className="inline-flex text-mute hover:text-slate-100"
            onClick={() => void api().openUrl("https://github.com/Jany-M/parla")}
          >
            <svg viewBox="0 0 16 16" width="12" height="12" aria-hidden="true" fill="currentColor">
              <path d="M8 0C3.58 0 0 3.58 0 8c0 3.54 2.29 6.53 5.47 7.59.4.07.55-.17.55-.38 0-.19-.01-.82-.01-1.49-2.01.37-2.53-.49-2.69-.94-.09-.23-.48-.94-.82-1.13-.28-.15-.68-.52-.01-.53.63-.01 1.08.58 1.23.82.72 1.21 1.87.87 2.33.66.07-.52.28-.87.51-1.07-1.78-.2-3.64-.89-3.64-3.95 0-.87.31-1.59.82-2.15-.08-.2-.36-1.02.08-2.12 0 0 .67-.21 2.2.82.64-.18 1.32-.27 2-.27.68 0 1.36.09 2 .27 1.53-1.04 2.2-.82 2.2-.82.44 1.1.16 1.92.08 2.12.51.56.82 1.27.82 2.15 0 3.07-1.87 3.75-3.65 3.95.29.25.54.73.54 1.48 0 1.07-.01 1.93-.01 2.2 0 .21.15.46.55.38A8.013 8.013 0 0016 8c0-4.42-3.58-8-8-8z" />
            </svg>
          </button>
        </span>
      </div>
      <button
        type="button"
        onClick={() => void api().openLogs()}
        className="px-3 text-left text-xs text-mute underline decoration-dotted hover:text-slate-100"
      >
        Open logs
      </button>
      <div className="px-3 pb-1 text-xs text-mute">v{version}</div>
    </div>
  );
}

export default function App() {
  const [view, setView] = useState<View>("record");
  return (
    <AppProvider>
      {(ctx) =>
        !ctx.settings.onboarding_done ? (
          <Wizard />
        ) : (
          <div className="flex h-full overflow-hidden">
            <nav aria-label="Main" className="flex w-44 shrink-0 flex-col gap-1 border-r border-line bg-panel p-3">
              <div className="mb-4 px-3 pt-2 text-lg font-semibold tracking-tight">Parla</div>
              {NAV.map((n) => (
                <button key={n.id} onClick={() => setView(n.id)} aria-current={view === n.id ? "page" : undefined}
                  className={`flex items-center gap-3 rounded-lg px-3 py-2 text-left text-sm transition ${view === n.id ? "bg-raised text-white" : "text-mute hover:bg-raised/60 hover:text-slate-100"}`}>
                  <span className={n.id === "record" ? "text-red-400" : ""}>{n.icon}</span>{n.label}
                </button>
              ))}
              <SidebarFooter version={ctx.info.version} />
            </nav>
            <div className="h-full min-w-0 flex-1 overflow-hidden">
              {/* Record stays mounted so a running recording's view is never lost when browsing */}
              <div className={view === "record" ? "h-full" : "hidden"}><Record goTo={(v) => setView(v)} /></div>
              {view === "library" && <Library goTo={(v) => setView(v)} />}
              {view === "settings" && <Settings />}
            </div>
          </div>
        )
      }
    </AppProvider>
  );
}
