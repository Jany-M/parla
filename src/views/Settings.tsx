import { useEffect, useState } from "react";
import { Banner, Button, Card, ProgressBar, Select, Toggle } from "../components/ui";
import { useApp } from "../context";
import { api } from "../lib/api";
import { formatBytes } from "../lib/format";
import { isLocalUrl, SLOT_SOURCES, type SourceKind } from "../lib/models";
import type { Devices, ModelProgress, Settings as S, SlotChoice } from "../lib/types";

type SlotKey = keyof S["slots"];
const SLOT_LABEL: Record<SlotKey, string> = { stt: "Speech recognition", speaker: "Speaker recognition", translate: "Translation", summarize: "Summaries" };
const LOCAL_HINT: Record<SlotKey, string> = {
  stt: "Folder with encoder, decoder, joiner (.onnx) and tokens.txt (a Parakeet/transducer model)",
  speaker: "A speaker-embedding .onnx file (WeSpeaker)",
  translate: "A .gguf model file",
  summarize: "A .gguf model file",
};
const SOURCE_LABEL: Record<SourceKind, string> = { managed: "Downloaded by Parla", local_file: "A model I already have", endpoint: "A local server (LM Studio, Ollama…)" };

export function Settings() {
  const { settings, models, languages, info, save, refreshModels, refreshInfo } = useApp();
  const [devices, setDevices] = useState<Devices>({ inputs: [], outputs: [] });
  const [busyId, setBusyId] = useState<string | null>(null);
  const [progress, setProgress] = useState<ModelProgress | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [confirmWipe, setConfirmWipe] = useState(false);

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
      <h1 className="text-xl font-semibold">Settings</h1>
      {error && <Banner onClose={() => setError(null)}>{error}</Banner>}

      <Card title="Models on this computer">
        <ul className="divide-y divide-line">
          {models.map((m) => (
            <li key={m.id} className="flex items-center justify-between gap-4 py-3">
              <div className="min-w-0">
                <div className="truncate text-sm font-medium">{m.label}</div>
                <div className="text-xs text-mute">{formatBytes(m.size_bytes)} · {m.license}{m.installed ? " · installed" : ""}</div>
                {busyId === m.id && progress?.id === m.id && <div className="mt-2 w-64"><ProgressBar value={progress.total ? progress.done / progress.total : 0} label={`Downloading ${m.label}`} /></div>}
              </div>
              {m.installed ? (
                <Button variant="ghost" onClick={() => guard(async () => { await api().uninstallModel(m.id); await refreshModels(); await refreshInfo(); })}>Remove</Button>
              ) : busyId === m.id ? (
                <Button variant="ghost" onClick={() => api().cancelInstall()}>Cancel</Button>
              ) : (
                <Button onClick={() => install(m.id)} disabled={busyId !== null}>Download</Button>
              )}
            </li>
          ))}
        </ul>
      </Card>

      <Card title="Use a model you already have">
        <p className="mb-4 text-sm text-mute">Optional. The model must be the right kind for the job. Parla checks it before saving.</p>
        <div className="space-y-5">
          {(Object.keys(SLOT_LABEL) as SlotKey[]).map((k) => {
            const c = settings.slots[k];
            return (
              <div key={k} className="space-y-2">
                <div className="flex items-center justify-between gap-3">
                  <label htmlFor={`src-${k}`} className="text-sm font-medium">{SLOT_LABEL[k]}</label>
                  <Select id={`src-${k}`} value={c.kind} onChange={(e) => void changeKind(k, e.target.value as SourceKind)}>
                    {SLOT_SOURCES[k].map((s) => <option key={s} value={s}>{SOURCE_LABEL[s]}</option>)}
                  </Select>
                </div>
                {c.kind === "local_file" && (
                  <div className="space-y-1">
                    <div className="flex gap-2">
                      <input aria-label={`${SLOT_LABEL[k]} path`} value={c.path} onChange={(e) => void setSlot(k, { kind: "local_file", path: e.target.value })} className="min-w-0 flex-1 rounded-lg border border-line bg-raised px-3 py-2 text-sm outline-none focus:border-accent" />
                      <Button onClick={async () => { const p = await api().pickFolder(); if (p) await setSlot(k, { kind: "local_file", path: p }); }}>Browse…</Button>
                    </div>
                    <p className="text-xs text-mute">{LOCAL_HINT[k]}</p>
                  </div>
                )}
                {c.kind === "endpoint" && (
                  <div className="space-y-2">
                    <div className="grid grid-cols-2 gap-2">
                      <input aria-label={`${SLOT_LABEL[k]} address`} value={c.url} onChange={(e) => void setSlot(k, { ...c, url: e.target.value })} placeholder="http://127.0.0.1:1234" className="rounded-lg border border-line bg-raised px-3 py-2 text-sm outline-none focus:border-accent" />
                      <input aria-label={`${SLOT_LABEL[k]} model name`} value={c.model} onChange={(e) => void setSlot(k, { ...c, model: e.target.value })} placeholder="model name" className="rounded-lg border border-line bg-raised px-3 py-2 text-sm outline-none focus:border-accent" />
                    </div>
                    {!isLocalUrl(c.url) && <Banner kind="warn">This address is not on your computer. Meeting text would be sent over the network to that server.</Banner>}
                  </div>
                )}
              </div>
            );
          })}
        </div>
      </Card>

      <Card title="Audio">
        <div className="grid grid-cols-[10rem_1fr] items-center gap-x-4 gap-y-3 text-sm">
          <label htmlFor="mic">Microphone</label>
          <Select id="mic" value={settings.mic_device ?? ""} onChange={(e) => void update({ mic_device: e.target.value || null })}>
            <option value="">System default</option>
            {devices.inputs.map((d) => <option key={d.id} value={d.id}>{d.name}</option>)}
          </Select>
          <label htmlFor="sys">Capture what plays on</label>
          <Select id="sys" value={settings.system_device ?? ""} onChange={(e) => void update({ system_device: e.target.value || null })}>
            <option value="">System default output</option>
            {devices.outputs.map((d) => <option key={d.id} value={d.id}>{d.name}</option>)}
          </Select>
        </div>
        <div className="mt-4"><Toggle label="Hide my microphone's echo of other speakers (useful without headphones)" checked={settings.echo_guard} onChange={(v) => void update({ echo_guard: v })} /></div>
      </Card>

      <Card title="Language and overlay">
        <div className="grid grid-cols-[10rem_1fr] items-center gap-x-4 gap-y-3 text-sm">
          <label htmlFor="sumlang">Write summaries in</label>
          <Select id="sumlang" value={settings.summary_lang} onChange={(e) => void update({ summary_lang: e.target.value })}>
            {languages.map((l) => <option key={l.code} value={l.code}>{l.name}</option>)}
          </Select>
          <label htmlFor="fs">Overlay text size</label>
          <input id="fs" type="range" min={14} max={40} value={settings.overlay_font_px} onChange={(e) => void update({ overlay_font_px: Number(e.target.value) })} />
          <label htmlFor="op">Overlay opacity</label>
          <input id="op" type="range" min={0.3} max={1} step={0.05} value={settings.overlay_opacity} onChange={(e) => void update({ overlay_opacity: Number(e.target.value) })} />
        </div>
      </Card>

      <Card title="Storage and privacy">
        <p className="text-sm text-mute">Everything Parla stores lives in one folder:</p>
        <code className="mt-1 block break-all rounded bg-raised px-2 py-1 text-xs">{info.system.data_dir}</code>
        <p className="mt-2 text-sm text-mute">Using {formatBytes(info.system.disk_usage_bytes)}. Uninstalling Parla from Windows removes this folder too.</p>
        <div className="mt-4">
          {confirmWipe ? (
            <div className="flex items-center gap-3">
              <span className="text-sm text-red-300">Delete all meetings, models and settings?</span>
              <Button variant="danger" onClick={() => guard(async () => { await api().deleteAllData(); setConfirmWipe(false); await refreshModels(); await refreshInfo(); window.location.reload(); })}>Yes, delete everything</Button>
              <Button variant="ghost" onClick={() => setConfirmWipe(false)}>Keep</Button>
            </div>
          ) : (
            <Button variant="ghost" onClick={() => setConfirmWipe(true)}>Delete all my data…</Button>
          )}
        </div>
      </Card>
    </div>
    </div>
  );
}
