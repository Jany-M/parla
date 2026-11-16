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
