import { useCallback, useEffect, useState } from "react";
import { PermissionsCard } from "./components/PermissionsCard";
import { AppProvider } from "./context";
import { useT } from "./i18n";
import { api } from "./lib/api";
import type { UpdateInfo } from "./lib/types";
import { Library } from "./views/Library";
import { Record } from "./views/Record";
import { Settings } from "./views/Settings";
import { Wizard } from "./views/Wizard";

type View = "record" | "library" | "settings";
const NAV: { id: View; labelKey: "nav.record" | "nav.library" | "nav.settings"; icon: string }[] = [
  { id: "record", labelKey: "nav.record", icon: "●" },
  { id: "library", labelKey: "nav.library", icon: "☰" },
  { id: "settings", labelKey: "nav.settings", icon: "⚙" },
];

/** How often to re-check GitHub Releases while the main window is open. */
export const UPDATE_CHECK_MS = 60 * 60 * 1000;

function SidebarFooter({ version }: { version: string }) {
  const t = useT();
  const [update, setUpdate] = useState<UpdateInfo | null>(null);
  const [busy, setBusy] = useState(false);
  const [pct, setPct] = useState<number | null>(null);
  const [error, setError] = useState<string | null>(null);

  const check = useCallback(() => {
    void api()
      .checkForUpdate()
      .then(setUpdate)
      .catch((e) => {
        setUpdate(null);
        void api().logUiError(`update check failed: ${String(e)}`);
      });
  }, []);

  useEffect(() => {
    let off = () => {};
    check(); // startup
    const timer = window.setInterval(check, UPDATE_CHECK_MS);
    void api()
      .onUpdateProgress((p) => {
        if (p.total > 0) setPct(Math.min(100, Math.round((100 * p.done) / p.total)));
      })
      .then((u) => {
        off = u;
      });
    return () => {
      window.clearInterval(timer);
      off();
    };
  }, [check]);

  const label = busy
    ? pct != null
      ? t("update.updatingPct", { pct })
      : t("update.updating")
    : update
      ? update.auto_install
        ? t("update.toVersion", { version: update.version })
        : t("update.downloadVersion", { version: update.version })
      : null;

  const link = "cursor-pointer underline decoration-dotted hover:text-slate-100";
  const issues = "https://github.com/Jany-M/parla/issues";

  return (
    <div className="mt-auto flex flex-col">
      <div className="px-3 text-[10px] leading-snug text-mute">
        <span className="block">{t("sidebar.developedBy")}</span>
        <span className="flex flex-wrap items-center gap-x-1 gap-y-0.5">
          <button type="button" className={link} onClick={() => void api().openUrl("https://github.com/Jany-M")}>
            Jany Martelli
          </button>
          <span>@</span>
          <button type="button" className={link} onClick={() => void api().openUrl("https://www.shambix.com/?utm_source=parla&utm_medium=referral&utm_campaign=projects&utm_content=app-sidebar")}>
            Shambix
          </button>
        </span>
        <span className="mt-2.5 flex items-center gap-1.5">
          <button type="button" className={link} onClick={() => void api().openUrl(issues)}>
            {t("sidebar.feedback")}
          </button>
          <button
            type="button"
            aria-label={t("sidebar.githubAria")}
            title={t("sidebar.githubAria")}
            className="inline-flex cursor-pointer text-mute hover:text-slate-100"
            onClick={() => void api().openUrl(issues)}
          >
            <svg viewBox="0 0 16 16" width="12" height="12" aria-hidden="true" fill="currentColor">
              <path d="M8 0C3.58 0 0 3.58 0 8c0 3.54 2.29 6.53 5.47 7.59.4.07.55-.17.55-.38 0-.19-.01-.82-.01-1.49-2.01.37-2.53-.49-2.69-.94-.09-.23-.48-.94-.82-1.13-.28-.15-.68-.52-.01-.53.63-.01 1.08.58 1.23.82.72 1.21 1.87.87 2.33.66.07-.52.28-.87.51-1.07-1.78-.2-3.64-.89-3.64-3.95 0-.87.31-1.59.82-2.15-.08-.2-.36-1.02.08-2.12 0 0 .67-.21 2.2.82.64-.18 1.32-.27 2-.27.68 0 1.36.09 2 .27 1.53-1.04 2.2-.82 2.2-.82.44 1.1.16 1.92.08 2.12.51.56.82 1.27.82 2.15 0 3.07-1.87 3.75-3.65 3.95.29.25.54.73.54 1.48 0 1.07-.01 1.93-.01 2.2 0 .21.15.46.55.38A8.013 8.013 0 0016 8c0-4.42-3.58-8-8-8z" />
            </svg>
          </button>
        </span>
      </div>
      <div className="mt-3 flex items-center justify-between gap-2 px-3 text-[10px] leading-snug text-mute">
        <button type="button" onClick={() => void api().openLogs()} className={link}>
          {t("sidebar.openLogs")}
        </button>
        <button type="button" className={link} title={t("sidebar.checkUpdatesTitle")} aria-label={t("sidebar.versionAria", { version })} onClick={check}>
          v{version}
        </button>
      </div>
      {update && (
        <div className="mt-2 px-0 pb-1">
          <button
            type="button"
            disabled={busy}
            onClick={() => {
              if (!update) return;
              setBusy(true);
              setError(null);
              setPct(null);
              if (!update.auto_install) {
                // macOS (and any build without a silent installer): open Releases; DMG is manual.
                void api()
                  .openUrl("https://github.com/Jany-M/parla/releases/latest")
                  .then(() => setBusy(false))
                  .catch((e) => {
                    setBusy(false);
                    setError(String(e));
                    void api().logUiError(`open releases failed: ${String(e)}`);
                  });
                return;
              }
              void api()
                .installUpdate()
                .catch((e) => {
                  setBusy(false);
                  setPct(null);
                  setError(String(e));
                  void api().logUiError(`install update failed: ${String(e)}`);
                });
            }}
            className="w-full rounded-lg bg-accent/20 px-3 py-2 text-left text-xs font-medium text-accent hover:bg-accent/30 disabled:opacity-60"
          >
            {label}
          </button>
          {error && <p className="mt-1 px-1 text-[10px] leading-snug text-red-300">{error}</p>}
        </div>
      )}
    </div>
  );
}

function Shell({ version }: { version: string }) {
  const t = useT();
  const [view, setView] = useState<View>("record");
  return (
    <div className="flex h-full overflow-hidden">
      <nav aria-label={t("nav.main")} className="flex w-44 shrink-0 flex-col gap-1 border-r border-line bg-panel p-3">
        <div className="mb-4 px-3 pt-2 text-lg font-semibold tracking-tight">{t("brand.name")}</div>
        {NAV.map((n) => (
          <button key={n.id} onClick={() => setView(n.id)} aria-current={view === n.id ? "page" : undefined}
            className={`flex items-center gap-3 rounded-lg px-3 py-2 text-left text-sm transition ${view === n.id ? "bg-raised text-white" : "text-mute hover:bg-raised/60 hover:text-slate-100"}`}>
            <span className={n.id === "record" ? "text-red-400" : ""}>{n.icon}</span>{t(n.labelKey)}
          </button>
        ))}
        <SidebarFooter version={version} />
      </nav>
      <div className="flex h-full min-w-0 flex-1 flex-col overflow-hidden">
        <PermissionsCard className="mx-6 mt-4 shrink-0" />
        {/* Record stays mounted so a running recording's view is never lost when browsing */}
        <div className={view === "record" ? "min-h-0 flex-1" : "hidden"}><Record goTo={(v) => setView(v)} /></div>
        {view === "library" && <div className="min-h-0 flex-1 overflow-hidden"><Library goTo={(v) => setView(v)} /></div>}
        {view === "settings" && <div className="min-h-0 flex-1 overflow-hidden"><Settings /></div>}
      </div>
    </div>
  );
}

export default function App() {
  return (
    <AppProvider>
      {(ctx) => (!ctx.settings.onboarding_done ? <Wizard /> : <Shell version={ctx.info.version} />)}
    </AppProvider>
  );
}
