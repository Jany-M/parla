import { useEffect, useState } from "react";
import { PermissionsCard } from "../components/PermissionsCard";
import { Banner, Button, Card, ProgressBar, Toggle } from "../components/ui";
import { useApp } from "../context";
import { useT } from "../i18n";
import { api } from "../lib/api";
import { formatBytes } from "../lib/format";
import { batchProgress, essentials, featureItems, missing, totalBytes } from "../lib/models";
import type { ModelProgress, ModelStatus } from "../lib/types";

/** First run: explain, let the user pick optional features, download, done. */
export function Wizard() {
  const { models, settings, save, refreshModels, info } = useApp();
  const t = useT();
  const [wantTranslate, setWantTranslate] = useState(true);
  const [wantSummary, setWantSummary] = useState(true);
  const [phase, setPhase] = useState<"choose" | "download">("choose");
  const [progress, setProgress] = useState<ModelProgress | null>(null);
  const [error, setError] = useState<string | null>(null);

  const chosen: ModelStatus[] = [
    ...essentials(models),
    ...(wantTranslate ? featureItems(models, "translate") : []),
    ...(wantSummary ? featureItems(models, "summarize") : []),
  ].filter((m, i, all) => all.findIndex((x) => x.id === m.id) === i);
  const todo = missing(chosen);

  useEffect(() => {
    let off: (() => void) | undefined;
    let cancelled = false;
    api().onModelProgress(setProgress).then((u) => (cancelled ? u() : (off = u)));
    return () => {
      cancelled = true;
      off?.();
    };
  }, []);

  const run = async () => {
    setError(null);
    setPhase("download");
    try {
      for (const m of todo) {
        await api().installModel(m.id);
        await refreshModels();
      }
      await save({ ...settings, onboarding_done: true });
    } catch (e) {
      setError(String(e));
      setPhase("choose");
    }
  };

  const warn = info.system.recommendation.warning;
  const ramGb = Math.round(info.system.hardware.total_ram_mb / 1024);
  const summaryPick = featureItems(models, "summarize").find((m) => m.slot === "summarize");

  return (
    <div className="grid h-full place-items-center overflow-y-auto p-6">
      <div className="w-full max-w-xl space-y-5">
        <div>
          <h1 className="text-2xl font-semibold">{t("wizard.title")}</h1>
          <p className="mt-1 text-mute">{t("wizard.subtitle")}</p>
        </div>
        <Card title={t("wizard.computer.title")}>
          <p className="text-sm">
            {t("wizard.computer.specs", { gb: ramGb, threads: info.system.hardware.cpu_threads })}
            {info.system.hardware.has_avx2 ? "" : ` · ${t("wizard.computer.noAvx2")}`}
          </p>
          {summaryPick && (
            <p className="mt-2 text-sm text-mute">
              {t("wizard.computer.summaryRec")} <span className="font-medium text-slate-100">{summaryPick.label}</span>
              {ramGb < 16 ? t("wizard.computer.summaryLowRam") : t("wizard.computer.summaryHighRam")}
            </p>
          )}
        </Card>
        <PermissionsCard />
        {warn && <Banner kind="warn">{warn}</Banner>}
        {error && <Banner onClose={() => setError(null)}>{error}</Banner>}

        <Card title={t("wizard.download.title")}>
          <ul className="space-y-3 text-sm">
            <li className="flex justify-between"><span>{t("wizard.download.required")} <span className="text-mute">{t("wizard.download.requiredTag")}</span></span><span className="text-mute">{formatBytes(totalBytes(essentials(models)))}</span></li>
            <li className="flex items-start justify-between gap-3">
              <Toggle label={t("wizard.download.translate")} checked={wantTranslate} onChange={setWantTranslate} disabled={phase === "download"} />
              <span className="shrink-0 text-mute">{formatBytes(totalBytes(featureItems(models, "translate")))}</span>
            </li>
            <li className="flex items-start justify-between gap-3">
              <Toggle label={`${t("wizard.download.summaries")}${summaryPick ? ` - ${summaryPick.label}` : ""}`} checked={wantSummary} onChange={setWantSummary} disabled={phase === "download"} />
              <span className="shrink-0 text-mute">{formatBytes(totalBytes(featureItems(models, "summarize")))}</span>
            </li>
          </ul>
          <p className="mt-3 text-xs text-mute">{t("wizard.download.sharedNote")}</p>
        </Card>

        {phase === "download" ? (
          <Card>
            <ProgressBar value={batchProgress(chosen, progress)} label={t("wizard.download.progress")} />
            <p className="mt-2 text-sm text-mute">{t("wizard.download.downloading", { label: todo.find((m) => m.id === progress?.id)?.label ?? t("wizard.download.modelsFallback") })}</p>
            <Button variant="ghost" className="mt-2" onClick={() => api().cancelInstall()}>{t("wizard.download.cancel")}</Button>
          </Card>
        ) : (
          <div className="flex items-center justify-between">
            <span className="text-sm text-mute">{todo.length === 0 ? t("wizard.download.allInstalled") : t("wizard.download.toDownload", { bytes: formatBytes(totalBytes(todo)) })}</span>
            <Button variant="primary" onClick={todo.length === 0 ? () => save({ ...settings, onboarding_done: true }) : run}>{todo.length === 0 ? t("wizard.cta.getStarted") : t("wizard.cta.downloadContinue")}</Button>
          </div>
        )}
      </div>
    </div>
  );
}
