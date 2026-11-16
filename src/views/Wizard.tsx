import { useEffect, useState } from "react";
import { Banner, Button, Card, ProgressBar } from "../components/ui";
import { useApp } from "../context";
import { api } from "../lib/api";
import { formatBytes } from "../lib/format";
import { batchProgress, essentials, featureItems, missing, totalBytes } from "../lib/models";
import type { ModelProgress, ModelStatus } from "../lib/types";

/** First run: explain, let the user pick optional features, download, done. */
export function Wizard() {
  const { models, settings, save, refreshModels, info } = useApp();
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
          <h1 className="text-2xl font-semibold">Welcome to Parla</h1>
          <p className="mt-1 text-mute">Meeting notes, speaker labels and live translation, all on this computer. Nothing is sent anywhere, and there are no accounts.</p>
        </div>
        <Card title="This computer">
          <p className="text-sm">About <strong>{ramGb} GB</strong> RAM · {info.system.hardware.cpu_threads} CPU threads{info.system.hardware.has_avx2 ? "" : " · no AVX2"}</p>
          {summaryPick && (
            <p className="mt-2 text-sm text-mute">
              Recommended summaries model: <span className="text-ink">{summaryPick.label}</span>
              {ramGb < 16 ? " (the larger 4B model needs about 16 GB RAM)." : " (you can switch to the smaller 2B model later in Settings)."}
            </p>
          )}
        </Card>
        {warn && <Banner kind="warn">{warn}</Banner>}
        {error && <Banner onClose={() => setError(null)}>{error}</Banner>}

        <Card title="What gets downloaded (once)">
          <ul className="space-y-3 text-sm">
            <li className="flex justify-between"><span>Speech recognition and speaker labels <span className="text-mute">(required)</span></span><span className="text-mute">{formatBytes(totalBytes(essentials(models)))}</span></li>
            <li className="flex items-start justify-between gap-3">
              <label className="flex items-center gap-2"><input type="checkbox" checked={wantTranslate} onChange={(e) => setWantTranslate(e.target.checked)} disabled={phase === "download"} /> Live translation overlay</label>
              <span className="text-mute">{formatBytes(totalBytes(featureItems(models, "translate")))}</span>
            </li>
            <li className="flex items-start justify-between gap-3">
              <label className="flex items-center gap-2"><input type="checkbox" checked={wantSummary} onChange={(e) => setWantSummary(e.target.checked)} disabled={phase === "download"} /> Meeting summaries{summaryPick ? ` — ${summaryPick.label}` : ""}</label>
              <span className="text-mute">{formatBytes(totalBytes(featureItems(models, "summarize")))}</span>
            </li>
          </ul>
          <p className="mt-3 text-xs text-mute">Shared parts are counted once. You can add or remove models later in Settings, and uninstalling Parla removes them all.</p>
        </Card>

        {phase === "download" ? (
          <Card>
            <ProgressBar value={batchProgress(chosen, progress)} label="Download progress" />
            <p className="mt-2 text-sm text-mute">Downloading {todo.find((m) => m.id === progress?.id)?.label ?? "models"}… this can take a few minutes.</p>
            <Button variant="ghost" className="mt-2" onClick={() => api().cancelInstall()}>Cancel</Button>
          </Card>
        ) : (
          <div className="flex items-center justify-between">
            <span className="text-sm text-mute">{todo.length === 0 ? "Everything is already installed." : `${formatBytes(totalBytes(todo))} to download`}</span>
            <Button variant="primary" onClick={todo.length === 0 ? () => save({ ...settings, onboarding_done: true }) : run}>{todo.length === 0 ? "Get started" : "Download and continue"}</Button>
          </div>
        )}
      </div>
    </div>
  );
}
