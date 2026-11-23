import { useCallback, useEffect, useState } from "react";
import { api } from "../lib/api";
import type { CapturePermissions, PermState } from "../lib/types";
import { useT } from "../i18n";
import { Banner, Button, Card } from "./ui";

function ok(s: PermState): boolean {
  return s === "granted" || s === "unknown";
}

function stateLabel(state: PermState): "perms.state.ok" | "perms.state.needed" | "perms.state.unknown" {
  if (state === "granted") return "perms.state.ok";
  if (state === "unknown") return "perms.state.unknown";
  return "perms.state.needed";
}

/** Shown on macOS when mic and/or Screen & System Audio Recording are missing. Hidden on Windows. */
export function PermissionsCard({ className = "" }: { className?: string }) {
  const t = useT();
  const [perms, setPerms] = useState<CapturePermissions | null>(null);
  const [busy, setBusy] = useState(false);

  const refresh = useCallback(async () => {
    try {
      setPerms(await api().capturePermissions());
    } catch {
      setPerms(null);
    }
  }, []);

  useEffect(() => {
    let cancelled = false;
    void (async () => {
      const p = await api().capturePermissions().catch(() => null);
      if (cancelled || !p) return;
      setPerms(p);
      if (p.required && (p.microphone !== "granted" || p.screen_recording !== "granted" || p.system_audio === "denied" || p.system_audio === "not_determined")) {
        const next = await api().requestCapturePermissions().catch(() => p);
        if (!cancelled) setPerms(next);
      }
    })();
    const timer = window.setInterval(() => void refresh(), 2500);
    const onFocus = () => void refresh();
    window.addEventListener("focus", onFocus);
    return () => {
      cancelled = true;
      window.clearInterval(timer);
      window.removeEventListener("focus", onFocus);
    };
  }, [refresh]);

  if (!perms?.required) return null;
  const micOk = perms.microphone === "granted";
  const screenOk = perms.screen_recording === "granted";
  const audioOk = ok(perms.system_audio);
  if (micOk && screenOk && audioOk) return null;

  return (
    <Card title={t("perms.title")} className={className}>
      <Banner kind="warn">{t("perms.intro")}</Banner>
      <ul className="mt-3 space-y-1.5 text-sm">
        <li className={micOk ? "text-mute" : "text-amber-200"}>
          {t("perms.microphone")}: {t(stateLabel(perms.microphone))}
        </li>
        <li className={screenOk ? "text-mute" : "text-amber-200"}>
          {t("perms.screen")}: {t(stateLabel(perms.screen_recording))}
        </li>
        <li className={audioOk ? "text-mute" : "text-amber-200"}>
          {t("perms.systemAudio")}: {t(stateLabel(perms.system_audio))}
        </li>
      </ul>
      <p className="mt-3 text-xs text-mute">{t("perms.howto")}</p>
      <div className="mt-3 flex flex-wrap gap-2">
        <Button
          variant="primary"
          disabled={busy}
          onClick={() => {
            setBusy(true);
            void api()
              .requestCapturePermissions()
              .then(setPerms)
              .finally(() => setBusy(false));
          }}
        >
          {t("perms.ask")}
        </Button>
        <Button
          variant="secondary"
          disabled={busy}
          onClick={() => {
            setBusy(true);
            void api()
              .openCapturePermissionSettings()
              .then(() => refresh())
              .finally(() => setBusy(false));
          }}
        >
          {t("perms.openSettings")}
        </Button>
        <Button variant="ghost" disabled={busy} onClick={() => void refresh()}>
          {t("perms.recheck")}
        </Button>
      </div>
    </Card>
  );
}
