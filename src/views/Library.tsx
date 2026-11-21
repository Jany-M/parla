import { useCallback, useEffect, useState } from "react";
import { Banner, Button, Card, ProgressBar } from "../components/ui";
import { Transcript } from "../components/Transcript";
import { useApp } from "../context";
import { useI18n, useT } from "../i18n";
import { api } from "../lib/api";
import { fileSafe, formatDate, formatDuration } from "../lib/format";
import { missing, featureItems } from "../lib/models";
import type { MeetingDetail, MeetingListItem } from "../lib/types";

export function Library({ goTo }: { goTo: (v: "settings") => void }) {
  const t = useT();
  const { lang } = useI18n();
  const [items, setItems] = useState<MeetingListItem[]>([]);
  const [selected, setSelected] = useState<number | null>(null);
  const [importing, setImporting] = useState<number | null>(null);
  const [error, setError] = useState<string | null>(null);

  const reload = useCallback(async () => setItems(await api().listMeetings()), []);
  useEffect(() => void reload(), [reload]);

  useEffect(() => {
    if (importing === null) return;
    let off: (() => void) | undefined;
    let cancelled = false;
    api().onEvent((e) => e.type === "progress" && setImporting(e.fraction)).then((u) => (cancelled ? u() : (off = u)));
    return () => {
      cancelled = true;
      off?.();
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [importing === null]);

  const importFile = async () => {
    setError(null);
    const path = await api().pickAudioFile();
    if (!path) return;
    setImporting(0);
    try {
      const id = await api().importFile(path);
      await reload();
      setSelected(id);
    } catch (e) {
      setError(String(e));
    } finally {
      setImporting(null);
    }
  };

  return (
    <div className="grid h-full grid-cols-[18rem_1fr]">
      <aside className="flex min-h-0 flex-col border-r border-line bg-panel">
        <div className="space-y-2 border-b border-line p-4">
          <Button variant="primary" className="w-full" onClick={importFile} disabled={importing !== null}>{t("library.import")}</Button>
          {importing !== null && (
            <div className="space-y-1">
              <ProgressBar value={importing} label={t("library.importProgress")} />
              <button className="text-xs text-mute underline" onClick={() => api().cancelImport()}>{t("library.cancel")}</button>
            </div>
          )}
        </div>
        <ul className="min-h-0 flex-1 overflow-y-auto" aria-label={t("library.meetingsAria")}>
          {items.length === 0 && <li className="p-4 text-sm text-mute">{t("library.emptyList")}</li>}
          {items.map((m) => (
            <li key={m.id}>
              <button onClick={() => setSelected(m.id)} aria-current={selected === m.id} className={`w-full border-b border-line/60 px-4 py-3 text-left hover:bg-raised ${selected === m.id ? "bg-raised" : ""}`}>
                <div className="truncate text-sm font-medium">{m.title || t("library.untitled")}</div>
                <div className="text-xs text-mute">{formatDate(m.started_at, lang)} · {formatDuration(m.started_at, m.ended_at, t("format.recording"))} · {m.source === "file" ? t("library.source.file") : t("library.source.live")}</div>
              </button>
            </li>
          ))}
        </ul>
      </aside>
      <main className="min-h-0 overflow-y-auto p-6">
        {error && <div className="mb-4"><Banner onClose={() => setError(null)}>{error}</Banner></div>}
        {selected === null ? (
          <p className="mt-20 text-center text-mute">{t("library.selectPrompt")}</p>
        ) : (
          <MeetingView key={selected} id={selected} goTo={goTo} onChanged={reload} onDeleted={() => { setSelected(null); void reload(); }} />
        )}
      </main>
    </div>
  );
}

export function MeetingView({ id, onChanged, onDeleted, goTo }: { id: number; onChanged: () => void; onDeleted: () => void; goTo: (v: "settings") => void }) {
  const { models, settings } = useApp();
  const t = useT();
  const { lang } = useI18n();
  const [d, setD] = useState<MeetingDetail | null>(null);
  const [summarizing, setSummarizing] = useState<{ done: number; total: number } | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [confirmDelete, setConfirmDelete] = useState(false);

  const load = useCallback(async () => setD(await api().getMeeting(id)), [id]);
  useEffect(() => void load(), [load]);

  useEffect(() => {
    let off: (() => void) | undefined;
    let cancelled = false;
    api().onSummaryProgress((p) => p.meeting_id === id && setSummarizing({ done: p.done, total: p.total })).then((u) => (cancelled ? u() : (off = u)));
    return () => {
      cancelled = true;
      off?.();
    };
  }, [id]);

  if (!d) return <p className="text-mute">{t("library.loading")}</p>;

  const summaryMissing = settings.slots.summarize.kind === "managed" && missing(featureItems(models, "summarize")).length > 0;
  const untitled = t("library.untitled");
  const guard = async (f: () => Promise<unknown>) => {
    setError(null);
    try {
      await f();
    } catch (e) {
      setError(String(e));
    }
  };

  const summarize = () =>
    guard(async () => {
      setSummarizing({ done: 0, total: 1 });
      try {
        await api().summarize(id);
        await load();
      } finally {
        setSummarizing(null);
      }
    });

  const exportMd = () =>
    guard(async () => {
      const path = await api().pickSavePath(`${fileSafe(d.meeting.title)}.md`);
      if (!path) return;
      await api().saveText(path, await api().exportMarkdown(id, formatDate(d.meeting.started_at, lang)));
    });

  return (
    <div className="mx-auto max-w-3xl space-y-5">
      <div className="flex items-start justify-between gap-4">
        <input
          key={`${id}-${d.meeting.title || "untitled"}`}
          aria-label={t("library.titleAria")}
          defaultValue={d.meeting.title.trim() ? d.meeting.title : untitled}
          onBlur={(e) => {
            const next = e.target.value.trim();
            const shown = d.meeting.title.trim() || untitled;
            if (next === shown) return;
            void guard(async () => {
              await api().renameMeeting(id, next);
              await load();
              onChanged();
            });
          }}
          className="w-full rounded-lg border border-line bg-raised px-3 py-2 text-xl font-semibold text-slate-100 outline-none focus:border-accent placeholder:text-mute"
          placeholder={untitled}
        />
        <div className="flex shrink-0 gap-2">
          <Button onClick={exportMd}>{t("library.exportMd")}</Button>
          {confirmDelete ? (
            <Button variant="danger" onClick={() => guard(async () => { await api().deleteMeeting(id); onDeleted(); })}>{t("library.deleteConfirm")}</Button>
          ) : (
            <Button variant="ghost" onClick={() => setConfirmDelete(true)}>{t("library.delete")}</Button>
          )}
        </div>
      </div>
      <p className="px-2 text-sm text-mute">{formatDate(d.meeting.started_at, lang)} · {formatDuration(d.meeting.started_at, d.meeting.ended_at, t("format.recording"))}</p>
      {error && <Banner onClose={() => setError(null)}>{error}</Banner>}

      <Card title={t("library.card.summary")} action={<Button variant="primary" onClick={summarize} disabled={summarizing !== null || d.segments.length === 0 || summaryMissing}>{d.meeting.summary ? t("library.summarizeAgain") : t("library.summarize")}</Button>}>
        {summarizing && <div className="mb-3"><ProgressBar value={summarizing.done / Math.max(1, summarizing.total)} label={t("library.summaryProgress")} /><p className="mt-1 text-xs text-mute">{t("library.summaryWorking")}</p></div>}
        {summaryMissing && <p className="text-sm text-mute">{t("library.summaryNeedModel")} <button className="underline" onClick={() => goTo("settings")}>{t("library.summaryDownloadLink")}</button>.</p>}
        {d.meeting.summary ? <pre className="whitespace-pre-wrap font-sans text-sm leading-relaxed text-slate-200">{d.meeting.summary}</pre> : !summaryMissing && !summarizing && <p className="text-sm text-mute">{t("library.summaryEmpty")}</p>}
      </Card>

      {d.speakers.length > 0 && (
        <Card title={t("library.card.speakers")}>
          <ul className="space-y-2">
            {d.speakers.map((s) => (
              <li key={s.id} className="flex items-center gap-2">
                <input aria-label={t("library.speaker.nameAria", { label: s.label })} defaultValue={s.name ?? ""} placeholder={s.is_me ? t("library.speaker.mePlaceholder") : s.label} disabled={s.is_me}
                  onBlur={(e) => e.target.value !== (s.name ?? "") && guard(async () => { await api().renameSpeaker(s.id, e.target.value); await load(); })}
                  className="w-56 rounded-lg border border-line bg-raised px-3 py-1.5 text-sm text-slate-100 outline-none focus:border-accent" />
                {!s.is_me && d.speakers.filter((o) => !o.is_me && o.id !== s.id).length > 0 && (
                  <select aria-label={t("library.speaker.mergeAria", { label: s.label })} value="" onChange={(e) => e.target.value && guard(async () => { await api().mergeSpeakers(s.id, Number(e.target.value)); await load(); })}
                    className="rounded-lg border border-line bg-raised px-2 py-1.5 text-xs text-mute">
                    <option value="">{t("library.speaker.samePerson")}</option>
                    {d.speakers.filter((o) => !o.is_me && o.id !== s.id).map((o) => <option key={o.id} value={o.id}>{o.name?.trim() || o.label}</option>)}
                  </select>
                )}
              </li>
            ))}
          </ul>
        </Card>
      )}

      <Card title={t("library.card.notes")}>
        <textarea aria-label={t("library.notesAria")} defaultValue={d.meeting.notes} placeholder={t("library.notesPlaceholder")} rows={3}
          onBlur={(e) => e.target.value !== d.meeting.notes && guard(async () => { await api().setNotes(id, e.target.value); await load(); })}
          className="w-full resize-y rounded-lg border border-line bg-raised p-3 text-sm text-slate-100 outline-none focus:border-accent" />
      </Card>

      <Card title={t("library.card.transcript")}><Transcript segments={d.segments} speakers={d.speakers} empty={t("library.transcriptEmpty")} /></Card>
    </div>
  );
}
