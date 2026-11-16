import { useCallback, useEffect, useState } from "react";
import { Banner, Button, Card, ProgressBar } from "../components/ui";
import { Transcript } from "../components/Transcript";
import { useApp } from "../context";
import { api } from "../lib/api";
import { fileSafe, formatDate, formatDuration } from "../lib/format";
import { missing, featureItems } from "../lib/models";
import type { MeetingDetail, MeetingListItem } from "../lib/types";

export function Library({ goTo }: { goTo: (v: "settings") => void }) {
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
          <Button variant="primary" className="w-full" onClick={importFile} disabled={importing !== null}>Import audio or video…</Button>
          {importing !== null && (
            <div className="space-y-1">
              <ProgressBar value={importing} label="Import progress" />
              <button className="text-xs text-mute underline" onClick={() => api().cancelImport()}>Cancel</button>
            </div>
          )}
        </div>
        <ul className="min-h-0 flex-1 overflow-y-auto" aria-label="Meetings">
          {items.length === 0 && <li className="p-4 text-sm text-mute">No meetings yet. Record one or import a file.</li>}
          {items.map((m) => (
            <li key={m.id}>
              <button onClick={() => setSelected(m.id)} aria-current={selected === m.id} className={`w-full border-b border-line/60 px-4 py-3 text-left hover:bg-raised ${selected === m.id ? "bg-raised" : ""}`}>
                <div className="truncate text-sm font-medium">{m.title || "Untitled"}</div>
                <div className="text-xs text-mute">{formatDate(m.started_at)} · {formatDuration(m.started_at, m.ended_at)} · {m.source === "file" ? "file" : "live"}</div>
              </button>
            </li>
          ))}
        </ul>
      </aside>
      <main className="min-h-0 overflow-y-auto p-6">
        {error && <div className="mb-4"><Banner onClose={() => setError(null)}>{error}</Banner></div>}
        {selected === null ? (
          <p className="mt-20 text-center text-mute">Select a meeting to read, summarize or export it.</p>
        ) : (
          <MeetingView key={selected} id={selected} goTo={goTo} onChanged={reload} onDeleted={() => { setSelected(null); void reload(); }} />
        )}
      </main>
    </div>
  );
}

export function MeetingView({ id, onChanged, onDeleted, goTo }: { id: number; onChanged: () => void; onDeleted: () => void; goTo: (v: "settings") => void }) {
  const { models, settings } = useApp();
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

  if (!d) return <p className="text-mute">Loading…</p>;

  const summaryMissing = settings.slots.summarize.kind === "managed" && missing(featureItems(models, "summarize")).length > 0;
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
      await api().saveText(path, await api().exportMarkdown(id, formatDate(d.meeting.started_at)));
    });

  return (
    <div className="mx-auto max-w-3xl space-y-5">
      <div className="flex items-start justify-between gap-4">
        <input
          aria-label="Meeting title"
          defaultValue={d.meeting.title}
          onBlur={(e) => e.target.value !== d.meeting.title && guard(async () => { await api().renameMeeting(id, e.target.value); await load(); onChanged(); })}
          className="w-full rounded-lg border border-line bg-raised px-3 py-2 text-xl font-semibold text-slate-100 outline-none focus:border-accent"
        />
        <div className="flex shrink-0 gap-2">
          <Button onClick={exportMd}>Export .md</Button>
          {confirmDelete ? (
            <Button variant="danger" onClick={() => guard(async () => { await api().deleteMeeting(id); onDeleted(); })}>Really delete</Button>
          ) : (
            <Button variant="ghost" onClick={() => setConfirmDelete(true)}>Delete</Button>
          )}
        </div>
      </div>
      <p className="px-2 text-sm text-mute">{formatDate(d.meeting.started_at)} · {formatDuration(d.meeting.started_at, d.meeting.ended_at)}</p>
      {error && <Banner onClose={() => setError(null)}>{error}</Banner>}

      <Card title="Summary" action={<Button variant="primary" onClick={summarize} disabled={summarizing !== null || d.segments.length === 0 || summaryMissing}>{d.meeting.summary ? "Summarize again" : "Summarize"}</Button>}>
        {summarizing && <div className="mb-3"><ProgressBar value={summarizing.done / Math.max(1, summarizing.total)} label="Summary progress" /><p className="mt-1 text-xs text-mute">Writing the notes on your computer. Long meetings take a few minutes.</p></div>}
        {summaryMissing && <p className="text-sm text-mute">The summary model is not installed. <button className="underline" onClick={() => goTo("settings")}>Download it in Settings</button>.</p>}
        {d.meeting.summary ? <pre className="whitespace-pre-wrap font-sans text-sm leading-relaxed text-slate-200">{d.meeting.summary}</pre> : !summaryMissing && !summarizing && <p className="text-sm text-mute">Press Summarize to get the key points, decisions and action items.</p>}
      </Card>

      {d.speakers.length > 0 && (
        <Card title="Speakers">
          <ul className="space-y-2">
            {d.speakers.map((s) => (
              <li key={s.id} className="flex items-center gap-2">
                <input aria-label={`Name for ${s.label}`} defaultValue={s.name ?? ""} placeholder={s.is_me ? "Me" : s.label} disabled={s.is_me}
                  onBlur={(e) => e.target.value !== (s.name ?? "") && guard(async () => { await api().renameSpeaker(s.id, e.target.value); await load(); })}
                  className="w-56 rounded-lg border border-line bg-raised px-3 py-1.5 text-sm outline-none focus:border-accent" />
                {!s.is_me && d.speakers.filter((o) => !o.is_me && o.id !== s.id).length > 0 && (
                  <select aria-label={`Merge ${s.label} into`} value="" onChange={(e) => e.target.value && guard(async () => { await api().mergeSpeakers(s.id, Number(e.target.value)); await load(); })}
                    className="rounded-lg border border-line bg-raised px-2 py-1.5 text-xs text-mute">
                    <option value="">Same person as…</option>
                    {d.speakers.filter((o) => !o.is_me && o.id !== s.id).map((o) => <option key={o.id} value={o.id}>{o.name?.trim() || o.label}</option>)}
                  </select>
                )}
              </li>
            ))}
          </ul>
        </Card>
      )}

      <Card title="Notes">
        <textarea aria-label="Notes" defaultValue={d.meeting.notes} placeholder="Your own notes…" rows={3}
          onBlur={(e) => e.target.value !== d.meeting.notes && guard(async () => { await api().setNotes(id, e.target.value); await load(); })}
          className="w-full resize-y rounded-lg border border-line bg-raised p-3 text-sm outline-none focus:border-accent" />
      </Card>

      <Card title="Transcript"><Transcript segments={d.segments} speakers={d.speakers} empty="No speech was detected." /></Card>
    </div>
  );
}
