import { useCallback, useEffect, useReducer, useRef, useState } from "react";
import { Banner, Button, Card, LevelMeter, Select } from "../components/ui";
import { Transcript } from "../components/Transcript";
import { useApp } from "../context";
import { useT } from "../i18n";
import { api } from "../lib/api";
import { emptyLive, reduceLive, unknownSpeakers } from "../lib/live";
import { featureItems, missing } from "../lib/models";
import { readEstimate, saveEstimate, startupMessage } from "../lib/warmup";
import type { Speaker } from "../lib/types";

export function Record({ goTo }: { goTo: (v: "library" | "settings") => void }) {
  const { settings, models, languages, save } = useApp();
  const t = useT();
  const [live, dispatch] = useReducer(reduceLive, undefined, emptyLive);
  const [meetingId, setMeetingId] = useState<number | null>(null);
  const [savedId, setSavedId] = useState<number | null>(null);
  const [speakers, setSpeakers] = useState<Speaker[]>([]);
  const [busy, setBusy] = useState(false);
  const [translating, setTranslating] = useState(false);
  const [paused, setPaused] = useState(false);
  const [noSpeech, setNoSpeech] = useState(false);
  const [starting, setStarting] = useState<{ since: number; estimate: number | null } | null>(null);
  const [, tick] = useState(0);
  const [error, setError] = useState<string | null>(null);
  const idRef = useRef<number | null>(null);
  idRef.current = meetingId;

  const loadSpeakers = useCallback(async (id: number) => {
    const d = await api().getMeeting(id);
    if (d) setSpeakers(d.speakers);
  }, []);

  // pick up a recording that is already running (e.g. after the window was reloaded)
  useEffect(() => {
    api().recordingState().then((id) => {
      if (id !== null) {
        setMeetingId(id);
        api().isPaused().then(setPaused);
        api().translationTarget().then((t) => setTranslating(t !== null));
        api().getMeeting(id).then((d) => {
          if (!d) return;
          setSpeakers(d.speakers);
          d.segments.forEach((segment) => dispatch({ type: "segment", segment }));
        });
      }
    });
  }, []);

  useEffect(() => {
    let off: (() => void) | undefined;
    let cancelled = false;
    api().onEvent((e) => {
      dispatch(e);
      if (e.type === "stopped" && e.meeting_id === idRef.current) {
        setMeetingId(null);
        if (e.saved) setSavedId(e.meeting_id);
        else setNoSpeech(true);
        setTranslating(false);
        setPaused(false);
        void api().showOverlay(false);
      }
    }).then((u) => (cancelled ? u() : (off = u)));
    return () => {
      cancelled = true;
      off?.();
    };
  }, []);

  useEffect(() => {
    if (meetingId !== null && unknownSpeakers(live.segments, speakers)) void loadSpeakers(meetingId);
  }, [live.segments, speakers, meetingId, loadSpeakers]);

  useEffect(() => {
    if (!starting) return;
    const timer = window.setInterval(() => tick((n) => n + 1), 250);
    return () => window.clearInterval(timer);
  }, [starting]);

  const start = async () => {
    setError(null);
    setBusy(true);
    const since = Date.now();
    setStarting({ since, estimate: readEstimate() });
    try {
      const id = await api().startLive();
      saveEstimate(Date.now() - since);
      setSavedId(null);
      setNoSpeech(false);
      dispatch({ type: "reset" });
      setPaused(false);
      setSpeakers([]);
      setMeetingId(id);
    } catch (e) {
      setError(String(e));
    } finally {
      setStarting(null);
      setBusy(false);
    }
  };

  const stop = async () => {
    setBusy(true);
    try {
      const id = await api().stopLive();
      setMeetingId(null);
      setTranslating(false);
      setPaused(false);
      await api().showOverlay(false);
      if (id !== null) setSavedId(id);
      else setNoSpeech(true);
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  const togglePause = async () => {
    const next = !paused;
    try {
      await api().setPaused(next);
      setPaused(next);
    } catch (e) {
      setError(String(e));
    }
  };

  const toggleTranslation = async () => {
    setError(null);
    setBusy(true);
    try {
      if (translating) {
        await api().stopTranslation();
        await api().showOverlay(false);
        setTranslating(false);
      } else {
        await api().startTranslation(settings.target_lang);
        await api().showOverlay(true);
        setTranslating(true);
      }
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  const translateMissing = missing(featureItems(models, "translate")).length > 0 && settings.slots.translate.kind === "managed";
  const recording = meetingId !== null;
  const empty = recording
    ? paused
      ? t("record.empty.paused")
      : t("record.empty.listening")
    : t("record.empty.idle");

  return (
    <div className="flex h-full flex-col gap-4 p-6">
      <header className="flex flex-wrap items-center justify-between gap-4">
        <div>
          <h1 className="text-xl font-semibold">{recording ? (paused ? t("record.status.paused") : t("record.status.recording")) : t("record.status.ready")}</h1>
          <p className="text-sm text-mute">{t("record.subtitle")}</p>
        </div>
        <div className="flex items-center gap-4">
          <div className="hidden flex-col gap-1 sm:flex">
            <LevelMeter label={t("record.meter.you")} level={live.levels.mic} />
            <LevelMeter label={t("record.meter.others")} level={live.levels.system} />
          </div>
          {recording ? (
            <>
              <Button variant="secondary" onClick={togglePause} disabled={busy} className="px-5 py-2.5">{paused ? t("record.btn.resume") : t("record.btn.pause")}</Button>
              <Button variant="danger" onClick={stop} disabled={busy} className="px-5 py-2.5">{t("record.btn.stop")}</Button>
            </>
          ) : (
            <Button variant="primary" onClick={start} disabled={busy} className="px-5 py-2.5">{t("record.btn.start")}</Button>
          )}
        </div>
      </header>

      {starting && <Banner kind="info">{startupMessage(starting.estimate, Date.now() - starting.since, t)}</Banner>}
      {error && <Banner onClose={() => setError(null)}>{error}</Banner>}
      {live.warnings.map((w) => <Banner key={w} kind="warn">{w}</Banner>)}
      {noSpeech && !recording && <Banner kind="info" onClose={() => setNoSpeech(false)}>{t("record.banner.noSpeech")}</Banner>}
      {savedId !== null && !recording && (
        <Banner kind="info">
          {t("record.banner.saved")} <button className="underline" onClick={() => goTo("library")}>{t("record.banner.openLibrary")}</button>
        </Banner>
      )}

      <Card title={t("record.card.liveTranslation")} className="shrink-0">
        <div className="flex flex-wrap items-center gap-3">
          <label className="text-sm text-mute" htmlFor="target-lang">{t("record.translate.label")}</label>
          <Select id="target-lang" value={settings.target_lang} onChange={(e) => void save({ ...settings, target_lang: e.target.value })} disabled={translating}>
            {languages.map((l) => <option key={l.code} value={l.code}>{l.name}</option>)}
          </Select>
          <Button variant={translating ? "secondary" : "primary"} onClick={toggleTranslation} disabled={!recording || busy || translateMissing}>
            {translating ? t("record.translate.stop") : t("record.translate.start")}
          </Button>
          {translating && <Button variant="ghost" onClick={() => api().showOverlay(true)}>{t("record.translate.showOverlay")}</Button>}
        </div>
        <p className="mt-2 text-xs text-mute">
          {translateMissing ? (
            <>{t("record.translate.needModel")} <button className="underline" onClick={() => goTo("settings")}>{t("record.translate.downloadLink")}</button>.</>
          ) : !recording ? t("record.translate.hintIdle") : t("record.translate.hintActive")}
        </p>
      </Card>

      <Card className="min-h-0 flex-1 overflow-y-auto">
        <Transcript segments={live.segments} speakers={speakers} autoScroll empty={empty} />
      </Card>
    </div>
  );
}
