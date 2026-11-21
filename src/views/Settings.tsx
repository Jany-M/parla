import { useEffect, useState } from "react";
import { Banner, Button, Card, ProgressBar, Select, Toggle } from "../components/ui";
import { useApp } from "../context";
import { UI_LANG_OPTIONS, useT, type MessageKey } from "../i18n";
import { api } from "../lib/api";
import { formatBytes } from "../lib/format";
import { eligibleForSlot, isLocalUrl, SLOT_SOURCES, type SourceKind } from "../lib/models";
import type { Devices, ModelProgress, Settings as S, SlotChoice } from "../lib/types";

type SlotKey = keyof S["slots"];
const SLOT_LABEL_KEY: Record<SlotKey, MessageKey> = {
  stt: "settings.slot.stt",
  speaker: "settings.slot.speaker",
  translate: "settings.slot.translate",
  summarize: "settings.slot.summarize",
};
const LOCAL_HINT_KEY: Record<SlotKey, MessageKey> = {
  stt: "settings.hint.stt",
  speaker: "settings.hint.speaker",
  translate: "settings.hint.translate",
  summarize: "settings.hint.summarize",
};
const SOURCE_LABEL_KEY: Record<SourceKind, MessageKey> = {
  managed: "settings.source.managed",
  local_file: "settings.source.localFile",
  endpoint: "settings.source.endpoint",
};

export function Settings() {
  const { settings, models, languages, info, save, refreshModels, refreshInfo } = useApp();
  const t = useT();
  const [devices, setDevices] = useState<Devices>({ inputs: [], outputs: [] });
  const [busyId, setBusyId] = useState<string | null>(null);
  const [progress, setProgress] = useState<ModelProgress | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [confirmWipe, setConfirmWipe] = useState(false);
  const isMac = /Mac/i.test(navigator.userAgent);

  useEffect(() => void api().listDevices().then(setDevices), []);
  useEffect(() => {
    let off: (() => void) | undefined;
    let cancelled = false;
    api().onModelProgress(setProgress).then((u) => (cancelled ? u() : (off = u)));
    return () => {
      cancelled = true;
      off?.();
    };
  }, []);

  const guard = async (f: () => Promise<unknown>) => {
    setError(null);
    try {
      await f();
    } catch (e) {
      setError(String(e));
    }
  };
  const update = (patch: Partial<S>) => guard(() => save({ ...settings, ...patch }));
  const setSlot = (k: SlotKey, c: SlotChoice) => guard(() => save({ ...settings, slots: { ...settings.slots, [k]: c } }));

  const install = (id: string) =>
    guard(async () => {
      setBusyId(id);
      try {
        await api().installModel(id);
      } finally {
        setBusyId(null);
        setProgress(null);
        await refreshModels();
        await refreshInfo();
      }
    });

  const changeKind = (k: SlotKey, kind: SourceKind) =>
    setSlot(k, kind === "managed" ? { kind: "managed", item_id: null } : kind === "local_file" ? { kind: "local_file", path: "" } : { kind: "endpoint", url: "http://127.0.0.1:1234", model: "", api_key: null });

  return (
    <div className="h-full overflow-y-auto">
    <div className="mx-auto max-w-3xl space-y-5 p-6">
      <h1 className="text-xl font-semibold">{t("settings.title")}</h1>
      {error && <Banner onClose={() => setError(null)}>{error}</Banner>}

      <Card title={t("settings.models.title")}>
        <p className="mb-3 text-sm text-mute">
          {t("settings.models.ramDetected", { gb: Math.round(info.system.hardware.total_ram_mb / 1024) })}
          {info.system.recommendation.tier === "low" ? t("settings.models.ramLow") : t("settings.models.ramOk")}
        </p>
        <ul className="divide-y divide-line">
          {models.filter((m) => m.eligible || m.installed).map((m) => (
            <li key={m.id} className="flex items-center justify-between gap-4 py-3">
              <div className="min-w-0">
                <div className="truncate text-sm font-medium">
                  {m.label}
                  {m.recommended ? <span className="ml-2 text-xs font-normal text-mute">{t("settings.models.recommended")}</span> : null}
                </div>
                <div className="text-xs text-mute">{formatBytes(m.size_bytes)} · {m.license}{m.installed ? ` · ${t("settings.models.installed")}` : ""}</div>
                {busyId === m.id && progress?.id === m.id && <div className="mt-2 w-64"><ProgressBar value={progress.total ? progress.done / progress.total : 0} label={t("settings.models.downloading", { label: m.label })} /></div>}
              </div>
              {m.installed ? (
                <Button variant="ghost" onClick={() => guard(async () => { await api().uninstallModel(m.id); await refreshModels(); await refreshInfo(); })}>{t("settings.models.remove")}</Button>
              ) : busyId === m.id ? (
                <Button variant="ghost" onClick={() => api().cancelInstall()}>{t("settings.models.cancel")}</Button>
              ) : m.eligible ? (
                <Button onClick={() => install(m.id)} disabled={busyId !== null}>{t("settings.models.download")}</Button>
              ) : (
                <span className="shrink-0 text-xs text-mute">{t("settings.models.needsRam")}</span>
              )}
            </li>
          ))}
        </ul>
      </Card>

      <Card title={t("settings.slots.title")}>
        <p className="mb-4 text-sm text-mute">{t("settings.slots.intro")}</p>
        <div className="space-y-5">
          {(Object.keys(SLOT_LABEL_KEY) as SlotKey[]).map((k) => {
            const c = settings.slots[k];
            const slotLabel = t(SLOT_LABEL_KEY[k]);
            return (
              <div key={k} className="space-y-2">
                <div className="flex items-center justify-between gap-3">
                  <label htmlFor={`src-${k}`} className="text-sm font-medium">{slotLabel}</label>
                  <Select id={`src-${k}`} value={c.kind} onChange={(e) => void changeKind(k, e.target.value as SourceKind)}>
                    {SLOT_SOURCES[k].map((s) => <option key={s} value={s}>{t(SOURCE_LABEL_KEY[s])}</option>)}
                  </Select>
                </div>
                {c.kind === "managed" && k === "summarize" && (
                  <div className="space-y-1">
                    <label htmlFor="sum-model" className="sr-only">{t("settings.slot.summarizeModelAria")}</label>
                    <Select
                      id="sum-model"
                      aria-label={t("settings.slot.summarizeModelAria")}
                      value={c.item_id ?? ""}
                      onChange={(e) => void setSlot(k, { kind: "managed", item_id: e.target.value || null })}
                    >
                      <option value="">{t("settings.slot.recommendedOption", { id: info.system.recommendation.summarize_item })}</option>
                      {eligibleForSlot(models, "summarize").map((m) => (
                        <option key={m.id} value={m.id}>{m.label}{m.recommended ? ` · ${t("settings.models.recommended")}` : ""}{m.installed ? "" : ` · ${t("settings.slot.notDownloaded")}`}</option>
                      ))}
                    </Select>
                    <p className="text-xs text-mute">{t("settings.slot.summarizeHint")}</p>
                  </div>
                )}
                {c.kind === "local_file" && (
                  <div className="space-y-1">
                    <div className="flex gap-2">
                      <input aria-label={`${slotLabel} path`} value={c.path} onChange={(e) => void setSlot(k, { kind: "local_file", path: e.target.value })} className="min-w-0 flex-1 rounded-lg border border-line bg-raised px-3 py-2 text-sm text-slate-100 outline-none focus:border-accent" />
                      <Button onClick={async () => { const p = await api().pickFolder(); if (p) await setSlot(k, { kind: "local_file", path: p }); }}>{t("settings.slot.browse")}</Button>
                    </div>
                    <p className="text-xs text-mute">{t(LOCAL_HINT_KEY[k])}</p>
                  </div>
                )}
                {c.kind === "endpoint" && (
                  <div className="space-y-2">
                    <div className="grid grid-cols-2 gap-2">
                      <input aria-label={`${slotLabel} address`} value={c.url} onChange={(e) => void setSlot(k, { ...c, url: e.target.value })} placeholder="http://127.0.0.1:1234" className="rounded-lg border border-line bg-raised px-3 py-2 text-sm text-slate-100 outline-none focus:border-accent" />
                      <input aria-label={`${slotLabel} model name`} value={c.model} onChange={(e) => void setSlot(k, { ...c, model: e.target.value })} placeholder={t("settings.endpoint.modelPlaceholder")} className="rounded-lg border border-line bg-raised px-3 py-2 text-sm text-slate-100 outline-none focus:border-accent" />
                    </div>
                    {!isLocalUrl(c.url) && <Banner kind="warn">{t("settings.endpoint.remoteWarn")}</Banner>}
                  </div>
                )}
              </div>
            );
          })}
        </div>
      </Card>

      <Card title={t("settings.audio.title")}>
        <div className="grid grid-cols-[10rem_1fr] items-center gap-x-4 gap-y-3 text-sm">
          <label htmlFor="mic">{t("settings.audio.mic")}</label>
          <Select id="mic" value={settings.mic_device ?? ""} onChange={(e) => void update({ mic_device: e.target.value || null })}>
            <option value="">{t("settings.audio.micDefault")}</option>
            {devices.inputs.map((d) => <option key={d.id} value={d.id}>{d.name}</option>)}
          </Select>
          <label htmlFor="sys">{t("settings.audio.system")}</label>
          <Select id="sys" value={settings.system_device ?? ""} onChange={(e) => void update({ system_device: e.target.value || null })}>
            <option value="">{t("settings.audio.systemDefault")}</option>
            {devices.outputs.map((d) => <option key={d.id} value={d.id}>{d.name}</option>)}
          </Select>
        </div>
        <div className="mt-4"><Toggle label={t("settings.audio.echoGuard")} checked={settings.echo_guard} onChange={(v) => void update({ echo_guard: v })} /></div>
      </Card>

      <Card title={t("settings.langOverlay.title")}>
        <div className="grid grid-cols-[10rem_1fr] items-center gap-x-4 gap-y-3 text-sm">
          <label htmlFor="uilang">{t("settings.langOverlay.uiLang")}</label>
          <Select id="uilang" value={settings.ui_lang} onChange={(e) => void update({ ui_lang: e.target.value })}>
            {UI_LANG_OPTIONS.map((l) => <option key={l.code} value={l.code}>{l.name}</option>)}
          </Select>
          <label htmlFor="sumlang">{t("settings.langOverlay.summaryLang")}</label>
          <Select id="sumlang" value={settings.summary_lang} onChange={(e) => void update({ summary_lang: e.target.value })}>
            {languages.map((l) => <option key={l.code} value={l.code}>{l.name}</option>)}
          </Select>
          <label htmlFor="fs">{t("settings.langOverlay.fontSize")}</label>
          <input id="fs" type="range" min={14} max={40} value={settings.overlay_font_px} onChange={(e) => void update({ overlay_font_px: Number(e.target.value) })} />
          <label htmlFor="op">{t("settings.langOverlay.opacity")}</label>
          <input id="op" type="range" min={0.3} max={1} step={0.05} value={settings.overlay_opacity} onChange={(e) => void update({ overlay_opacity: Number(e.target.value) })} />
        </div>
      </Card>

      <Card title={t("settings.storage.title")}>
        <p className="text-sm text-mute">{t("settings.storage.livesIn")}</p>
        <code className="mt-1 block break-all rounded bg-raised px-2 py-1 text-xs">{info.system.data_dir}</code>
        <p className="mt-2 text-sm text-mute">{t(isMac ? "settings.storage.usageMac" : "settings.storage.usage", { bytes: formatBytes(info.system.disk_usage_bytes) })}</p>
        <div className="mt-4">
          {confirmWipe ? (
            <div className="flex items-center gap-3">
              <span className="text-sm text-red-300">{t("settings.storage.wipeAsk")}</span>
              <Button variant="danger" onClick={() => guard(async () => { await api().deleteAllData(); setConfirmWipe(false); await refreshModels(); await refreshInfo(); window.location.reload(); })}>{t("settings.storage.wipeYes")}</Button>
              <Button variant="ghost" onClick={() => setConfirmWipe(false)}>{t("settings.storage.wipeKeep")}</Button>
            </div>
          ) : (
            <Button variant="ghost" onClick={() => setConfirmWipe(true)}>{t("settings.storage.wipeStart")}</Button>
          )}
        </div>
      </Card>
    </div>
    </div>
  );
}
