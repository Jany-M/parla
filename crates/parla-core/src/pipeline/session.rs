use super::detector::{EnergyConfig, EnergyDetector, SpeechChunk, SpeechDetector};
use super::echo::{EchoGuard, HoldQueue};
use super::engines::{Recognizer, SpeakerEmbedder};
use super::speakers::OnlineClusterer;
use crate::events::{Event, EventSink};
use crate::languages;
use crate::store::Store;
use crate::types::*;
use anyhow::Result;
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, TryRecvError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub type DetectorFactory = Box<dyn Fn() -> Box<dyn SpeechDetector> + Send + Sync>;

/// The loaded models. One recogniser and one embedder are shared by every stream.
#[derive(Clone)]
pub struct Engine {
    pub recognizer: Arc<Mutex<Box<dyn Recognizer>>>,
    pub embedder: Option<Arc<Mutex<Box<dyn SpeakerEmbedder>>>>,
}

impl Engine {
    pub fn new(r: Box<dyn Recognizer>, e: Option<Box<dyn SpeakerEmbedder>>) -> Self {
        Self { recognizer: Arc::new(Mutex::new(r)), embedder: e.map(|e| Arc::new(Mutex::new(e))) }
    }
}

#[derive(Clone, Debug)]
pub struct SessionConfig {
    pub echo_guard: bool,
    /// How long Mic segments wait for a matching System segment before being committed.
    pub echo_hold: Duration,
    pub cluster_threshold: f32,
    pub max_speakers: usize,
    /// Chunks shorter than this inherit the previous speaker instead of being embedded.
    pub min_embed_secs: f64,
}

impl Default for SessionConfig {
    fn default() -> Self {
        Self { echo_guard: true, echo_hold: Duration::from_secs(2), cluster_threshold: 0.5, max_speakers: 8, min_embed_secs: 1.0 }
    }
}

struct StreamState {
    detector: Box<dyn SpeechDetector>,
    clusterer: OnlineClusterer,
    speaker_ids: Vec<i64>,
    last_speaker: Option<i64>,
    /// Recent raw audio, so a detected sentence can be given a little lead-in and tail before
    /// recognition (detectors cut right at the first/last loud sample and clip soft word edges).
    history: Vec<f32>,
    /// Absolute sample index of `history[0]`.
    hist_start: u64,
    /// Meeting-clock offset (seconds) of this stream's sample 0. Set on the first audio so mic and
    /// system share one timeline even when a device only emits while something is playing.
    origin_secs: Option<f64>,
}

const HISTORY_MAX: usize = 40 * 16_000;
const HISTORY_KEEP: usize = 30 * 16_000;
const PAD_BEFORE: u64 = 12_000; // 0.75 s: soft first syllables, Bluetooth/mic AGC ramp-up
const PAD_AFTER: u64 = 3_200; // 0.2 s

/// Raises quiet audio (laptop and headset microphones) to a level the recogniser handles well.
/// Never lowers loud audio and caps the boost so noise is not blown up.
pub(crate) fn normalize(v: &mut [f32]) {
    let peak = v.iter().fold(0f32, |m, x| m.max(x.abs()));
    if peak > 1e-4 {
        let gain = (0.6 / peak).clamp(1.0, 6.0);
        if gain > 1.01 {
            v.iter_mut().for_each(|x| *x = (*x * gain).clamp(-1.0, 1.0));
        }
    }
}

/// One recording (live or file import). Everything flows through `push(Frame)`.
pub struct Session {
    store: Store,
    meeting_id: i64,
    engine: Engine,
    sink: Arc<dyn EventSink>,
    cfg: SessionConfig,
    make_detector: DetectorFactory,
    streams: HashMap<StreamKind, StreamState>,
    me: Option<i64>,
    guard: EchoGuard,
    held: HoldQueue<NewSegment>,
    /// Speech waiting for ASR. Kept off the audio path so a slow transcription cannot stall
    /// VAD / sample clocks (which used to shove trailing Mic lines into the wrong place).
    pending_asr: VecDeque<(StreamKind, SpeechChunk)>,
    paused: Arc<AtomicBool>,
    /// Wall origin for aligning per-stream sample clocks onto one meeting timeline.
    meeting_start: Instant,
}

fn rms(s: &[f32]) -> f32 {
    if s.is_empty() {
        0.0
    } else {
        (s.iter().map(|x| x * x).sum::<f32>() / s.len() as f32).sqrt()
    }
}

/// ASR output with no letters or digits ("...", "-") is not a segment.
fn clean(text: &str) -> Option<String> {
    let t = text.trim();
    t.chars().any(|c| c.is_alphanumeric()).then(|| t.to_string())
}

impl Session {
    pub fn new(store: Store, meeting_id: i64, engine: Engine, sink: Arc<dyn EventSink>, cfg: SessionConfig, make_detector: DetectorFactory) -> Self {
        let hold = cfg.echo_hold;
        Self {
            store,
            meeting_id,
            engine,
            sink,
            cfg,
            make_detector,
            streams: HashMap::new(),
            me: None,
            guard: EchoGuard::default(),
            held: HoldQueue::new(hold),
            pending_asr: VecDeque::new(),
            paused: Arc::new(AtomicBool::new(false)),
            meeting_start: Instant::now(),
        }
    }

    /// Shares a pause flag: while set, incoming audio is discarded and the transcript simply
    /// continues (same meeting, same speakers) once cleared.
    pub fn with_pause(mut self, flag: Arc<AtomicBool>) -> Self {
        self.paused = flag;
        self
    }

    /// Detector used when the Silero model is not installed (and in tests).
    pub fn energy_detector_factory() -> DetectorFactory {
        Box::new(|| Box::new(EnergyDetector::new(EnergyConfig::default())))
    }

    pub fn push(&mut self, frame: Frame) -> Result<()> {
        self.sink.emit(Event::Level { stream: frame.stream, level: rms(&frame.samples) });
        for c in self.feed(frame.stream, &frame.samples) {
            self.pending_asr.push_back((frame.stream, c));
        }
        Ok(())
    }

    /// Runs at most one pending transcription so the caller can keep draining audio.
    fn transcribe_one(&mut self) -> Result<bool> {
        let Some((kind, chunk)) = self.pending_asr.pop_front() else {
            return Ok(false);
        };
        self.handle_chunk(kind, chunk)?;
        Ok(true)
    }

    fn drain_asr(&mut self) -> Result<()> {
        while self.transcribe_one()? {}
        Ok(())
    }

    /// Every sample the detector sees also goes into the history (same indices).
    fn feed(&mut self, kind: StreamKind, samples: &[f32]) -> Vec<SpeechChunk> {
        let origin = self.meeting_start.elapsed().as_secs_f64();
        let st = self.state(kind);
        if st.origin_secs.is_none() {
            st.origin_secs = Some(origin);
        }
        if st.history.len() > HISTORY_MAX {
            let cut = st.history.len() - HISTORY_KEEP;
            st.history.drain(..cut);
            st.hist_start += cut as u64;
        }
        st.history.extend_from_slice(samples);
        st.detector.accept(samples)
    }

    /// Map a stream-local chunk onto the shared meeting clock.
    fn meeting_times(&self, kind: StreamKind, chunk: &SpeechChunk) -> (f64, f64) {
        let origin = self.streams.get(&kind).and_then(|s| s.origin_secs).unwrap_or(0.0);
        let t0 = origin + chunk.t0();
        (t0, t0 + chunk.duration())
    }

    /// The sentence plus a short lead-in and tail, level-adjusted, for the recogniser.
    fn recognizer_input(&mut self, kind: StreamKind, chunk: &SpeechChunk) -> Vec<f32> {
        let st = self.state(kind);
        let end = chunk.start_sample + chunk.samples.len() as u64;
        let from = chunk.start_sample.saturating_sub(PAD_BEFORE).max(st.hist_start);
        let to = (end + PAD_AFTER).min(st.hist_start + st.history.len() as u64);
        let mut v = if chunk.start_sample >= st.hist_start && from < to && to >= end {
            st.history[(from - st.hist_start) as usize..(to - st.hist_start) as usize].to_vec()
        } else {
            chunk.samples.clone() // the detector's own indices do not match: use its audio as is
        };
        normalize(&mut v);
        v
    }

    /// Releases held Mic segments whose delay has passed.
    pub fn tick(&mut self, now: Instant) -> Result<()> {
        let due = self.held.release_due(now);
        self.release(due)
    }

    /// End of input: flush every detector, finish ASR, and release everything still held.
    pub fn finish(&mut self) -> Result<()> {
        self.flush_streams()?;
        self.drain_asr()?;
        let rest = self.held.drain();
        self.release(rest)
    }

    /// Blocking loop for a worker thread; returns when every sender is gone.
    pub fn run(mut self, rx: Receiver<Frame>) -> Result<()> {
        let mut was_paused = false;
        let mut disconnected = false;
        loop {
            let paused = self.paused.load(Ordering::SeqCst);
            if paused && !was_paused {
                log::info!("pause: closing open speech");
                self.quiesce()?; // finish the sentence in progress, then go quiet
                log::info!("pause: done");
            }
            if !paused && was_paused {
                log::info!("resume");
            }
            was_paused = paused;

            // Keep sample clocks moving: pull every waiting frame before any ASR work.
            let mut got = false;
            loop {
                match rx.try_recv() {
                    Ok(_) if paused => got = true,
                    Ok(f) => {
                        got = true;
                        self.push(f)?;
                    }
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        disconnected = true;
                        break;
                    }
                }
            }
            if !got && !disconnected {
                match rx.recv_timeout(Duration::from_millis(250)) {
                    Ok(_) if paused => {}
                    Ok(f) => self.push(f)?,
                    Err(RecvTimeoutError::Timeout) => {}
                    Err(RecvTimeoutError::Disconnected) => disconnected = true,
                }
            }

            self.transcribe_one()?;
            self.tick(Instant::now())?;
            if disconnected && self.pending_asr.is_empty() {
                break;
            }
        }
        self.finish()
    }

    /// Builds the per-stream detectors now (loading their models takes a moment) instead of on
    /// the first audio frame, so the start of a recording is not missed.
    pub fn prepare(&mut self, kinds: &[StreamKind]) {
        for k in kinds {
            self.state(*k);
        }
    }

    /// Ends any sentence in progress by feeding one second of silence through the normal path
    /// (the same calls as regular audio, no special flush), then zeroes the meters.
    fn quiesce(&mut self) -> Result<()> {
        let kinds: Vec<StreamKind> = self.streams.keys().copied().collect();
        let silence = vec![0.0f32; 1600];
        for k in kinds {
            for _ in 0..10 {
                for c in self.feed(k, &silence) {
                    self.pending_asr.push_back((k, c));
                }
            }
            self.sink.emit(Event::Level { stream: k, level: 0.0 });
        }
        self.drain_asr()
    }

    /// Finalises any speech still being collected on every stream and zeroes the meters.
    fn flush_streams(&mut self) -> Result<()> {
        let kinds: Vec<StreamKind> = self.streams.keys().copied().collect();
        for k in kinds {
            self.sink.emit(Event::Level { stream: k, level: 0.0 });
            let chunks = self.state(k).detector.flush();
            for c in chunks {
                self.pending_asr.push_back((k, c));
            }
        }
        Ok(())
    }

    // ---- internals ---------------------------------------------------------

    fn state(&mut self, kind: StreamKind) -> &mut StreamState {
        let (thr, max, factory) = (self.cfg.cluster_threshold, self.cfg.max_speakers, &self.make_detector);
        self.streams.entry(kind).or_insert_with(|| StreamState {
            detector: factory(),
            clusterer: OnlineClusterer::new(thr, max),
            speaker_ids: vec![],
            last_speaker: None,
            history: Vec::new(),
            hist_start: 0,
            origin_secs: None,
        })
    }

    /// Test helper: pin a stream's sample-0 to a meeting-clock offset (seconds).
    #[cfg(test)]
    pub fn set_stream_origin_for_test(&mut self, kind: StreamKind, secs: f64) {
        self.state(kind).origin_secs = Some(secs);
    }

    fn warn(&self, message: String) {
        log::warn!("{message}");
        self.sink.emit(Event::Warning { message });
    }

    fn handle_chunk(&mut self, kind: StreamKind, chunk: SpeechChunk) -> Result<()> {
        // breadcrumbs: if the process dies inside a model call, the log's last line says where
        log::info!("transcribing {:.1}s of {} speech", chunk.duration(), kind.as_str());
        let input = self.recognizer_input(kind, &chunk);
        let started = Instant::now();
        let text = {
            let mut r = self.engine.recognizer.lock().unwrap_or_else(|e| e.into_inner());
            r.transcribe(&input)
        };
        log::info!("transcribed in {} ms", started.elapsed().as_millis());
        let text = match text {
            Ok(t) => t,
            Err(e) => {
                self.warn(format!("Transcription failed: {e}"));
                return Ok(());
            }
        };
        let Some(text) = clean(&text) else { return Ok(()) };
        let (t0, t1) = self.meeting_times(kind, &chunk);
        let lang = languages::detect(&text).map(String::from);
        let speaker_id = if kind == StreamKind::Mic { self.me_speaker()? } else { self.assign_speaker(kind, &chunk)? };
        let seg = NewSegment { meeting_id: self.meeting_id, stream: kind, speaker_id: Some(speaker_id), t0, t1, lang, text };
        match kind {
            StreamKind::Mic if self.cfg.echo_guard => self.hold_or_commit_mic(seg)?,
            StreamKind::System => {
                self.guard.note_system(t0, t1, &seg.text);
                self.commit(seg)?;
                // System speech is what the hold waits for: decide held Mic lines now so they
                // keep their early timestamps instead of landing after the remote talker.
                self.release_held_against_system()?;
            }
            _ => self.commit(seg)?,
        }
        Ok(())
    }

    fn hold_or_commit_mic(&mut self, seg: NewSegment) -> Result<()> {
        if self.guard.is_echo(seg.t0, seg.t1, &seg.text) {
            log::info!("echo guard dropped mic segment: {}", seg.text);
            return Ok(());
        }
        if self.guard.has_system_near(seg.t0, seg.t1) {
            // Overlapping System speech already seen and it did not match: commit immediately.
            return self.commit(seg);
        }
        self.held.push(Instant::now(), seg);
        Ok(())
    }

    fn release_held_against_system(&mut self) -> Result<()> {
        let decided = self.held.extract_if(|seg| self.guard.has_system_near(seg.t0, seg.t1));
        self.release(decided)
    }

    fn release(&mut self, items: Vec<NewSegment>) -> Result<()> {
        for seg in items {
            if self.guard.is_echo(seg.t0, seg.t1, &seg.text) {
                log::info!("echo guard dropped mic segment: {}", seg.text);
                continue;
            }
            self.commit(seg)?;
        }
        Ok(())
    }

    fn commit(&mut self, seg: NewSegment) -> Result<()> {
        let segment = self.store.insert_segment(&seg)?;
        self.sink.emit(Event::Segment { segment });
        Ok(())
    }

    fn me_speaker(&mut self) -> Result<i64> {
        if let Some(id) = self.me {
            return Ok(id);
        }
        let id = self.store.create_speaker(self.meeting_id, true)?.id;
        self.me = Some(id);
        Ok(id)
    }

    fn assign_speaker(&mut self, kind: StreamKind, chunk: &SpeechChunk) -> Result<i64> {
        let min = self.cfg.min_embed_secs;
        let (meeting, store, embedder) = (self.meeting_id, self.store.clone(), self.engine.embedder.clone());
        let known = self.state(kind).last_speaker;
        if let Some(emb) = embedder.filter(|_| known.is_none() || chunk.duration() >= min) {
            log::info!("identifying the speaker ({} samples)", chunk.samples.len());
            let r = emb.lock().unwrap_or_else(|e| e.into_inner()).embed(&chunk.samples);
            log::info!("speaker identified");
            match r {
                Ok(v) => {
                    let st = self.state(kind);
                    if let Some(a) = st.clusterer.assign(&v) {
                        if a.is_new {
                            st.speaker_ids.push(store.create_speaker(meeting, false)?.id);
                        }
                        let id = st.speaker_ids[a.index];
                        st.last_speaker = Some(id);
                        return Ok(id);
                    }
                }
                Err(e) => self.warn(format!("Speaker recognition failed: {e}")),
            }
        }
        if let Some(id) = self.state(kind).last_speaker {
            return Ok(id);
        }
        let id = store.create_speaker(meeting, false)?.id;
        self.state(kind).last_speaker = Some(id);
        Ok(id)
    }
}

#[cfg(test)]
mod tests {
    use super::super::detector::testutil::*;
    use super::super::engines::fakes::*;
    use super::*;
    use crate::events::VecSink;
    use std::sync::mpsc;

    struct Rig {
        store: Store,
        meeting: i64,
        sink: Arc<VecSink>,
        embed_calls: Arc<Mutex<usize>>,
    }

    fn rig(texts: &[&str], embedder: bool, cfg: SessionConfig) -> (Session, Rig) {
        let store = Store::open_in_memory().unwrap();
        let meeting = store.create_meeting("live").unwrap();
        let sink = Arc::new(VecSink::default());
        let calls = Arc::new(Mutex::new(0));
        let emb: Option<Box<dyn SpeakerEmbedder>> = embedder.then(|| Box::new(LoudnessEmbedder(calls.clone())) as _);
        let engine = Engine::new(Box::new(ScriptedRecognizer::texts(texts)), emb);
        let s = Session::new(store.clone(), meeting, engine, sink.clone(), cfg, Session::energy_detector_factory());
        (s, Rig { store, meeting, sink, embed_calls: calls })
    }

    fn cfg0() -> SessionConfig {
        SessionConfig { echo_hold: Duration::ZERO, ..Default::default() }
    }

    fn burst(s: &mut Session, kind: StreamKind, pre: f32, amp: f32, secs: f32) {
        let audio = cat(&[silence(pre), tone(200.0, secs, amp), silence(1.0)]);
        for c in audio.chunks(1600) {
            s.push(Frame { stream: kind, samples: c.to_vec() }).unwrap();
        }
        s.drain_asr().unwrap();
    }

    fn names(r: &Rig) -> Vec<(String, String)> {
        let sp = r.store.speakers(r.meeting).unwrap();
        r.store
            .segments(r.meeting)
            .unwrap()
            .into_iter()
            .map(|g| (sp.iter().find(|s| Some(s.id) == g.speaker_id).unwrap().label.clone(), g.text))
            .collect()
    }

    #[test]
    fn system_speech_becomes_a_stored_segment_and_an_event() {
        let (mut s, r) = rig(&["Hello everyone, welcome to the meeting."], true, cfg0());
        burst(&mut s, StreamKind::System, 1.0, 0.3, 2.0);
        s.finish().unwrap();
        let segs = r.store.segments(r.meeting).unwrap();
        assert_eq!(segs.len(), 1);
        assert!(segs[0].t0 > 0.7 && segs[0].t0 <= 1.0 && segs[0].t1 > 3.0 && segs[0].t1 < 3.3, "{:?}", (segs[0].t0, segs[0].t1));
        assert_eq!(segs[0].lang.as_deref(), Some("en"));
        assert_eq!(r.sink.segments(), segs);
    }

    #[test]
    fn mic_and_system_share_one_meeting_clock_when_origins_differ() {
        // System-only devices often start their sample clock late (no silence frames). Without a
        // shared origin, both streams look like they begin at 00:00 and the library order is wrong.
        let (mut s, r) = rig(&["hello from me on the mic", "hello from the youtube speaker"], true, cfg0());
        s.prepare(&[StreamKind::Mic, StreamKind::System]);
        s.set_stream_origin_for_test(StreamKind::Mic, 0.0);
        s.set_stream_origin_for_test(StreamKind::System, 20.0);
        burst(&mut s, StreamKind::Mic, 0.5, 0.3, 1.5);
        burst(&mut s, StreamKind::System, 0.5, 0.3, 1.5);
        s.finish().unwrap();
        let segs = r.store.segments(r.meeting).unwrap();
        assert_eq!(segs.len(), 2);
        assert_eq!(segs[0].stream, StreamKind::Mic, "mic (earlier origin) sorts first");
        assert_eq!(segs[1].stream, StreamKind::System);
        assert!(segs[0].t0 < 5.0, "mic near the start: {}", segs[0].t0);
        assert!(segs[1].t0 > 18.0, "system shifted by origin: {}", segs[1].t0);
    }

    #[test]
    fn mic_is_always_me_and_never_clustered() {
        let (mut s, r) = rig(&["Good morning, thanks for joining."], true, cfg0());
        burst(&mut s, StreamKind::Mic, 0.5, 0.3, 1.5);
        s.finish().unwrap();
        assert_eq!(names(&r), [("Me".into(), "Good morning, thanks for joining.".into())]);
        assert_eq!(*r.embed_calls.lock().unwrap(), 0);
    }

    #[test]
    fn distinct_voices_get_distinct_speakers_and_voices_are_remembered() {
        let (mut s, r) = rig(&["first voice here", "second voice here", "first voice again"], true, cfg0());
        burst(&mut s, StreamKind::System, 0.5, 0.3, 1.5);
        burst(&mut s, StreamKind::System, 0.5, 0.5, 1.5);
        burst(&mut s, StreamKind::System, 0.5, 0.3, 1.5);
        s.finish().unwrap();
        let n = names(&r);
        assert_eq!(n[0].0, "Speaker 1");
        assert_eq!(n[1].0, "Speaker 2");
        assert_eq!(n[2].0, "Speaker 1");
    }

    #[test]
    fn short_chunks_inherit_the_previous_speaker_without_embedding() {
        let (mut s, r) = rig(&["a long first sentence", "yes sure"], true, cfg0());
        burst(&mut s, StreamKind::System, 0.5, 0.3, 2.0);
        burst(&mut s, StreamKind::System, 0.5, 0.5, 0.5); // different "voice" but too short to trust
        s.finish().unwrap();
        let n = names(&r);
        assert_eq!((n[0].0.as_str(), n[1].0.as_str()), ("Speaker 1", "Speaker 1"));
        assert_eq!(*r.embed_calls.lock().unwrap(), 1);
    }

    #[test]
    fn without_an_embedder_remote_speech_is_one_speaker() {
        let (mut s, r) = rig(&["one two three four", "five six seven eight"], false, cfg0());
        burst(&mut s, StreamKind::System, 0.5, 0.3, 1.5);
        burst(&mut s, StreamKind::System, 0.5, 0.5, 1.5);
        s.finish().unwrap();
        assert!(names(&r).iter().all(|(who, _)| who == "Speaker 1"));
        assert_eq!(r.store.speakers(r.meeting).unwrap().len(), 1);
    }

    #[test]
    fn echo_of_the_remote_sentence_on_the_mic_is_dropped() {
        let (mut s, r) = rig(&["We should move the release to next Thursday", "we should move the release to next thursday"], true, cfg0());
        burst(&mut s, StreamKind::System, 1.0, 0.3, 3.0);
        burst(&mut s, StreamKind::Mic, 1.0, 0.1, 3.0);
        s.finish().unwrap();
        let n = names(&r);
        assert_eq!(n.len(), 1);
        assert_eq!(n[0].0, "Speaker 1");
    }

    #[test]
    fn genuine_mic_speech_survives_the_guard_and_can_be_turned_off() {
        let (mut s, r) = rig(&["We should move the release to next Thursday", "I would rather do it on Friday please"], true, cfg0());
        burst(&mut s, StreamKind::System, 1.0, 0.3, 3.0);
        burst(&mut s, StreamKind::Mic, 1.0, 0.1, 3.0);
        s.finish().unwrap();
        assert_eq!(names(&r).len(), 2);

        let mut cfg = cfg0();
        cfg.echo_guard = false;
        let (mut s, r) = rig(&["same words go here now", "same words go here now"], true, cfg);
        burst(&mut s, StreamKind::System, 1.0, 0.3, 3.0);
        burst(&mut s, StreamKind::Mic, 1.0, 0.1, 3.0);
        s.finish().unwrap();
        assert_eq!(names(&r).len(), 2, "guard disabled keeps both");
    }

    #[test]
    fn mic_segments_wait_for_the_hold_time() {
        let cfg = SessionConfig { echo_hold: Duration::from_secs(60), ..Default::default() };
        let (mut s, r) = rig(&["hello from my microphone today"], true, cfg);
        burst(&mut s, StreamKind::Mic, 0.5, 0.3, 1.5);
        s.tick(Instant::now()).unwrap();
        assert!(r.store.segments(r.meeting).unwrap().is_empty(), "still held");
        s.finish().unwrap();
        assert_eq!(r.store.segments(r.meeting).unwrap().len(), 1, "finish releases everything");
    }

    #[test]
    fn system_speech_releases_held_mic_so_it_keeps_its_early_time() {
        // Trailing Mic phrases used to stay held while System ASR ran, then show up under the
        // remote speaker. Once System is known (and is not an echo match), commit Mic immediately.
        let cfg = SessionConfig { echo_hold: Duration::from_secs(60), ..Default::default() };
        let (mut s, r) = rig(&["and the microphone input with live translation", "a customer says the checkout button does nothing"], true, cfg);
        burst(&mut s, StreamKind::Mic, 0.2, 0.3, 1.5);
        assert!(r.store.segments(r.meeting).unwrap().is_empty(), "mic waiting on hold");
        burst(&mut s, StreamKind::System, 0.2, 0.3, 1.5);
        let segs = r.store.segments(r.meeting).unwrap();
        assert_eq!(segs.len(), 2, "mic released as soon as system arrived");
        assert_eq!(segs[0].stream, StreamKind::Mic);
        assert_eq!(segs[1].stream, StreamKind::System);
        assert!(segs[0].t0 < segs[1].t0, "mic stays earlier on the meeting clock: {:?} vs {:?}", segs[0].t0, segs[1].t0);
    }

    #[test]
    fn push_only_queues_asr_so_audio_can_keep_moving() {
        let (mut s, r) = rig(&["queued until drain"], true, cfg0());
        let audio = cat(&[silence(0.5), tone(200.0, 1.5, 0.3), silence(1.0)]);
        for c in audio.chunks(1600) {
            s.push(Frame { stream: StreamKind::Mic, samples: c.to_vec() }).unwrap();
        }
        assert!(r.store.segments(r.meeting).unwrap().is_empty(), "no ASR until drained");
        assert!(!s.pending_asr.is_empty());
        s.drain_asr().unwrap();
        s.finish().unwrap();
        assert_eq!(r.store.segments(r.meeting).unwrap().len(), 1);
    }

    #[test]
    fn engine_failures_become_warnings_and_do_not_stop_the_session() {
        let store = Store::open_in_memory().unwrap();
        let m = store.create_meeting("live").unwrap();
        let sink = Arc::new(VecSink::default());
        let mut rec = ScriptedRecognizer::texts(&[]);
        rec.0.push_back(Err("boom".into()));
        rec.0.push_back(Ok("...".into())); // no letters → ignored
        rec.0.push_back(Ok("this one works fine".into()));
        let mut s = Session::new(store.clone(), m, Engine::new(Box::new(rec), None), sink.clone(), cfg0(), Session::energy_detector_factory());
        for _ in 0..3 {
            burst(&mut s, StreamKind::System, 0.5, 0.3, 1.5);
        }
        s.finish().unwrap();
        let segs = store.segments(m).unwrap();
        assert_eq!(segs.len(), 1);
        assert_eq!(segs[0].text, "this one works fine");
        let warns = sink.take().into_iter().filter(|e| matches!(e, Event::Warning { .. })).count();
        assert_eq!(warns, 1);
    }

    #[test]
    fn level_events_are_emitted_per_frame() {
        let (mut s, r) = rig(&[], false, cfg0());
        s.push(Frame { stream: StreamKind::Mic, samples: tone(200.0, 0.1, 0.5) }).unwrap();
        let ev = r.sink.take();
        assert!(matches!(ev[0], Event::Level { stream: StreamKind::Mic, level } if level > 0.3));
    }

    #[test]
    fn run_processes_a_channel_and_flushes_when_senders_disconnect() {
        let (s, r) = rig(&["spoken right up to the end"], true, cfg0());
        let (tx, rx) = mpsc::channel();
        let h = std::thread::spawn(move || s.run(rx));
        for c in cat(&[silence(0.5), tone(200.0, 1.5, 0.3)]).chunks(1600) {
            tx.send(Frame { stream: StreamKind::System, samples: c.to_vec() }).unwrap();
        }
        drop(tx);
        h.join().unwrap().unwrap();
        assert_eq!(r.store.segments(r.meeting).unwrap().len(), 1);
    }

    #[test]
    fn pausing_drops_audio_and_resuming_continues_the_same_meeting() {
        let (s, r) = rig(&["said before the pause", "said after the pause"], true, cfg0());
        let paused = Arc::new(AtomicBool::new(false));
        let s = s.with_pause(paused.clone());
        let (tx, rx) = mpsc::sync_channel(0); // rendezvous: send returns once the session took the frame
        let h = std::thread::spawn(move || s.run(rx));
        let send = |audio: Vec<f32>| {
            for c in audio.chunks(1600) {
                tx.send(Frame { stream: StreamKind::System, samples: c.to_vec() }).unwrap();
            }
        };
        send(cat(&[silence(0.5), tone(200.0, 1.5, 0.3), silence(1.0)]));
        paused.store(true, Ordering::SeqCst);
        send(silence(0.3)); // lets the loop observe the pause
        send(cat(&[tone(200.0, 1.5, 0.3), silence(1.0)])); // lost: nobody hears it
        paused.store(false, Ordering::SeqCst);
        send(silence(0.3)); // lets the loop observe the resume
        send(cat(&[silence(0.5), tone(200.0, 1.5, 0.3), silence(1.0)]));
        drop(tx);
        h.join().unwrap().unwrap();
        let segs: Vec<String> = r.store.segments(r.meeting).unwrap().into_iter().map(|s| s.text).collect();
        assert_eq!(segs, ["said before the pause", "said after the pause"]);
    }

    #[test]
    fn normalize_boosts_quiet_audio_but_never_lowers_or_explodes() {
        let mut quiet = vec![0.1f32, -0.1];
        normalize(&mut quiet);
        assert!((quiet[0] - 0.6).abs() < 1e-5);
        let mut loud = vec![0.9f32, -0.9];
        normalize(&mut loud);
        assert_eq!(loud, [0.9, -0.9]);
        let mut noise = vec![0.001f32];
        normalize(&mut noise);
        assert!((noise[0] - 0.006).abs() < 1e-6, "boost is capped at 6x");
        let mut silent = vec![0.0f32; 4];
        normalize(&mut silent);
        assert_eq!(silent, [0.0; 4]);
    }

    struct Capture(Arc<Mutex<Vec<usize>>>);
    impl Recognizer for Capture {
        fn transcribe(&mut self, s: &[f32]) -> Result<String> {
            self.0.lock().unwrap().push(s.len());
            Ok("some words".into())
        }
    }

    #[test]
    fn recognition_gets_a_short_lead_in_and_tail_around_the_detected_sentence() {
        let lens = Arc::new(Mutex::new(vec![]));
        let store = Store::open_in_memory().unwrap();
        let meeting = store.create_meeting("live").unwrap();
        let engine = Engine::new(Box::new(Capture(lens.clone())), None);
        let mut s = Session::new(store, meeting, engine, Arc::new(VecSink::default()), cfg0(), Session::energy_detector_factory());
        burst(&mut s, StreamKind::System, 1.0, 0.3, 1.5);
        let seen = lens.lock().unwrap()[0];
        // ~1.5 s speech + EnergyDetector preroll/hangover keep + PAD_BEFORE/AFTER (≈45600 with 0.75 s lead-in).
        // Must be more than bare speech and far less than the full 3.5 s burst buffer (56000).
        assert!((30_000..55_000).contains(&seen), "got {seen}");
    }
}
