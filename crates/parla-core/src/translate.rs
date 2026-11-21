//! Translation on request: a two-tier bounded queue (live + backfill), a worker thread, and a
//! hub that sits between the pipeline and the UI so translation is a pure add-on to the event stream.
use crate::events::{Event, EventSink};
use crate::languages;
use crate::llm::{prompts, ChatModel, ChatOptions};
use crate::store::Store;
use crate::types::{Segment, StreamKind};
use anyhow::{Context, Result};
use std::collections::VecDeque;
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;

/// How many fresh segments may wait in the live tier (overlay stays close to "now").
pub const LIVE_QUEUE_CAP: usize = 16;
/// Deferred demotions + start catch-up; drained when the live tier is empty.
pub const BACKFILL_QUEUE_CAP: usize = 64;
/// Minimum alphanumeric signal before a segment is worth translating.
pub const MIN_SIGNAL_CHARS: usize = 12;

pub trait Translator: Send {
    fn translate(&mut self, text: &str, target_code: &str) -> Result<String>;
}

/// Translator backed by any chat model (Hy-MT2 by default).
pub struct LlmTranslator(pub Arc<dyn ChatModel>);

impl Translator for LlmTranslator {
    fn translate(&mut self, text: &str, target_code: &str) -> Result<String> {
        let lang = languages::by_code(target_code).with_context(|| format!("unsupported target language '{target_code}'"))?;
        let out = self.0.chat(&prompts::translation_messages(lang.name, text), &ChatOptions::translation())?;
        let out = out.trim().trim_matches('"').trim().to_string();
        anyhow::ensure!(!out.is_empty(), "empty translation");
        Ok(out)
    }
}

/// Alphanumeric weight used for eligibility and for picking demotion / overflow victims.
pub fn signal_chars(text: &str) -> usize {
    text.chars().filter(|c| c.is_alphanumeric()).count()
}

/// Only what other people say, long enough to matter, and not already in the target language.
pub fn should_translate(seg: &Segment, target: &str) -> bool {
    seg.stream != StreamKind::Mic
        && signal_chars(&seg.text) >= MIN_SIGNAL_CHARS
        && seg.lang.as_deref().map(|l| !l.eq_ignore_ascii_case(target)).unwrap_or(true)
}

fn needs_translation(seg: &Segment, target: &str) -> bool {
    match seg.translation_lang.as_deref() {
        Some(lang) if lang.eq_ignore_ascii_case(target) && seg.translation.is_some() => false,
        _ => true,
    }
}

/// Index of the shortest segment (by [`signal_chars`]); ties keep the oldest (lowest index).
fn shortest_oldest_index(items: &VecDeque<Segment>) -> usize {
    let mut best = 0;
    let mut best_sig = signal_chars(&items[0].text);
    for (i, seg) in items.iter().enumerate().skip(1) {
        let sig = signal_chars(&seg.text);
        if sig < best_sig {
            best = i;
            best_sig = sig;
        }
    }
    best
}

fn trim_backfill(backfill: &mut VecDeque<Segment>) -> usize {
    let mut dropped = 0;
    while backfill.len() > BACKFILL_QUEUE_CAP {
        let i = shortest_oldest_index(backfill);
        backfill.remove(i);
        dropped += 1;
    }
    dropped
}

struct QueueState {
    live: VecDeque<Segment>,
    backfill: VecDeque<Segment>,
    closed: bool,
}

/// Two-tier FIFO: live is preferred for freshness; overflow demotes the shortest waiting live
/// item into backfill. Permanent drops happen only when backfill itself overflows.
pub struct TranslationQueue {
    state: Mutex<QueueState>,
    cv: Condvar,
    live_cap: usize,
}

impl TranslationQueue {
    pub fn new(live_cap: usize) -> Self {
        Self {
            state: Mutex::new(QueueState { live: VecDeque::new(), backfill: VecDeque::new(), closed: false }),
            cv: Condvar::new(),
            live_cap: live_cap.max(1),
        }
    }

    /// Push onto the live tier. Returns how many segments were permanently dropped from backfill.
    pub fn push(&self, seg: Segment) -> usize {
        let mut s = self.state.lock().unwrap();
        if s.closed {
            return 0;
        }
        let mut dropped = 0;
        let mut demoted = 0;
        while s.live.len() >= self.live_cap {
            let i = shortest_oldest_index(&s.live);
            let victim = s.live.remove(i).expect("live nonempty");
            s.backfill.push_back(victim);
            demoted += 1;
            dropped += trim_backfill(&mut s.backfill);
        }
        if demoted > 0 {
            log::debug!("translation demoted {demoted} sentence(s) to backfill");
        }
        s.live.push_back(seg);
        self.cv.notify_one();
        dropped
    }

    /// Push onto the backfill tier (catch-up). Returns permanent drops from backfill overflow.
    pub fn push_backfill(&self, seg: Segment) -> usize {
        let mut s = self.state.lock().unwrap();
        if s.closed {
            return 0;
        }
        s.backfill.push_back(seg);
        let dropped = trim_backfill(&mut s.backfill);
        self.cv.notify_one();
        dropped
    }

    /// Blocks for the next item (live first, then backfill); `None` once closed (pending discarded).
    pub fn pop(&self) -> Option<Segment> {
        let mut s = self.state.lock().unwrap();
        loop {
            if s.closed {
                return None;
            }
            if let Some(x) = s.live.pop_front() {
                return Some(x);
            }
            if let Some(x) = s.backfill.pop_front() {
                return Some(x);
            }
            s = self.cv.wait(s).unwrap();
        }
    }

    pub fn close(&self) {
        self.state.lock().unwrap().closed = true;
        self.cv.notify_all();
    }

    pub fn len(&self) -> usize {
        let s = self.state.lock().unwrap();
        s.live.len() + s.backfill.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    #[cfg(test)]
    fn live_len(&self) -> usize {
        self.state.lock().unwrap().live.len()
    }

    #[cfg(test)]
    fn backfill_len(&self) -> usize {
        self.state.lock().unwrap().backfill.len()
    }
}

struct Active {
    queue: Arc<TranslationQueue>,
    handle: Option<JoinHandle<()>>,
    target: String,
}

/// Wrap the session's sink with this: segments flow through unchanged, and while a translation
/// is active, eligible ones are also translated and reported as `Event::Translation`.
pub struct TranslationHub {
    inner: Arc<dyn EventSink>,
    store: Store,
    meeting_id: i64,
    active: Mutex<Option<Active>>,
}

impl TranslationHub {
    pub fn new(inner: Arc<dyn EventSink>, store: Store, meeting_id: i64) -> Arc<Self> {
        Arc::new(Self { inner, store, meeting_id, active: Mutex::new(None) })
    }

    pub fn is_active(&self) -> bool {
        self.active.lock().unwrap().is_some()
    }

    pub fn target(&self) -> Option<String> {
        self.active.lock().unwrap().as_ref().map(|a| a.target.clone())
    }

    /// (Re)starts translation into `target`. `queue_cap` is the **live** tier bound; backfill uses
    /// [`BACKFILL_QUEUE_CAP`]. Seeds backfill with recent untranslated segments from this meeting.
    pub fn start(&self, mut translator: Box<dyn Translator>, target: &str, queue_cap: usize) {
        self.stop();
        let queue = Arc::new(TranslationQueue::new(queue_cap));
        let (q, sink, store, tgt) = (queue.clone(), self.inner.clone(), self.store.clone(), target.to_string());
        let handle = std::thread::spawn(move || {
            let mut failures = 0u32;
            while let Some(seg) = q.pop() {
                match translator.translate(&seg.text, &tgt) {
                    Ok(text) => {
                        failures = 0;
                        if let Err(e) = store.set_translation(seg.id, &text, &tgt) {
                            log::warn!("saving translation failed: {e}");
                        }
                        sink.emit(Event::Translation { segment_id: seg.id, text, lang: tgt.clone() });
                    }
                    Err(e) => {
                        failures += 1;
                        log::warn!("translation failed: {e}");
                        if failures == 1 || failures % 10 == 0 {
                            sink.emit(Event::Warning { message: format!("Translation failed: {e}") });
                        }
                    }
                }
            }
        });
        *self.active.lock().unwrap() = Some(Active { queue: queue.clone(), handle: Some(handle), target: target.to_string() });
        self.seed_catchup(&queue, target);
    }

    fn seed_catchup(&self, queue: &TranslationQueue, target: &str) {
        let Ok(segments) = self.store.segments(self.meeting_id) else {
            return;
        };
        let mut eligible: Vec<Segment> = segments.into_iter().filter(|s| needs_translation(s, target) && should_translate(s, target)).collect();
        // Newest first, keep a window, then enqueue oldest-first so FIFO plays forward in time.
        eligible.sort_by(|a, b| b.t0.partial_cmp(&a.t0).unwrap_or(std::cmp::Ordering::Equal).then(b.id.cmp(&a.id)));
        if eligible.len() > BACKFILL_QUEUE_CAP {
            eligible.truncate(BACKFILL_QUEUE_CAP);
        }
        eligible.reverse();
        let mut dropped = 0;
        for seg in eligible {
            dropped += queue.push_backfill(seg);
        }
        if dropped > 0 {
            log::warn!("translation is behind: dropped {dropped} sentence(s)");
        }
    }

    pub fn stop(&self) {
        let taken = self.active.lock().unwrap().take();
        if let Some(mut a) = taken {
            let pending = a.queue.len();
            if pending > 0 {
                log::info!("translation stopping with {pending} pending sentence(s) abandoned");
            }
            a.queue.close();
            if let Some(h) = a.handle.take() {
                let _ = h.join();
            }
        }
    }
}

impl EventSink for TranslationHub {
    fn emit(&self, e: Event) {
        if let Event::Segment { segment } = &e {
            if let Some(a) = self.active.lock().unwrap().as_ref() {
                if should_translate(segment, &a.target) {
                    let dropped = a.queue.push(segment.clone());
                    if dropped > 0 {
                        log::warn!("translation is behind: dropped {dropped} sentence(s)");
                    }
                }
            }
        }
        self.inner.emit(e);
    }
}

impl Drop for TranslationHub {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::VecSink;
    use crate::llm::fakes::FakeChat;
    use std::time::{Duration, Instant};

    fn seg(id: i64, stream: StreamKind, lang: Option<&str>, text: &str) -> Segment {
        Segment {
            id,
            meeting_id: 1,
            stream,
            speaker_id: None,
            t0: id as f64,
            t1: id as f64 + 1.0,
            lang: lang.map(Into::into),
            text: text.into(),
            translation: None,
            translation_lang: None,
        }
    }

    struct Upper;
    impl Translator for Upper {
        fn translate(&mut self, t: &str, to: &str) -> Result<String> {
            Ok(format!("[{to}] {}", t.to_uppercase()))
        }
    }

    fn wait_until(mut f: impl FnMut() -> bool) {
        let t = Instant::now();
        while !f() {
            assert!(t.elapsed() < Duration::from_secs(5), "timed out");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn eligibility_rules() {
        assert_eq!(signal_chars("hello there"), 10);
        assert!(!should_translate(&seg(1, StreamKind::System, Some("en"), "hello there"), "it"), "10 alnum < MIN_SIGNAL_CHARS");
        assert!(!should_translate(&seg(1, StreamKind::System, None, "ok"), "it"), "short fluff skipped");
        assert!(should_translate(&seg(1, StreamKind::System, Some("en"), "hello there friend"), "it"));
        assert!(!should_translate(&seg(1, StreamKind::System, Some("it"), "ciao a tutti amici"), "it"), "already in target language");
        assert!(!should_translate(&seg(1, StreamKind::Mic, Some("en"), "hello there friend"), "it"), "never translate myself");
        assert!(!should_translate(&seg(1, StreamKind::System, None, "?"), "it"));
        assert!(should_translate(&seg(1, StreamKind::File, Some("fr"), "bonjour à tous"), "en"));
    }

    #[test]
    fn live_full_demotes_shortest_then_oldest_to_backfill() {
        let q = TranslationQueue::new(2);
        // id1 signal 3, id2/id3 signal 20 → demote id1
        assert_eq!(q.push(seg(1, StreamKind::System, None, "aaa")), 0);
        assert_eq!(q.push(seg(2, StreamKind::System, None, "bbbbbbbbbbbbbbbbbbbb")), 0);
        assert_eq!(q.live_len(), 2);
        assert_eq!(q.push(seg(3, StreamKind::System, None, "cccccccccccccccccccc")), 0);
        assert_eq!(q.live_len(), 2);
        assert_eq!(q.backfill_len(), 1);
        // live preferred: 2 then 3, then backfill 1
        assert_eq!(q.pop().unwrap().id, 2);
        assert_eq!(q.pop().unwrap().id, 3);
        assert_eq!(q.pop().unwrap().id, 1);
    }

    #[test]
    fn demotion_tie_breaks_to_oldest() {
        let q = TranslationQueue::new(2);
        assert_eq!(q.push(seg(1, StreamKind::System, None, "aaaa")), 0);
        assert_eq!(q.push(seg(2, StreamKind::System, None, "bbbb")), 0);
        assert_eq!(q.push(seg(3, StreamKind::System, None, "cccccccccccc")), 0);
        assert_eq!(q.backfill_len(), 1);
        assert_eq!(q.pop().unwrap().id, 2); // live keeps newer equal-ish; oldest short demoted
        // both aaaa/bbbb same signal 4 → demote oldest id1
        assert_eq!(q.pop().unwrap().id, 3);
        assert_eq!(q.pop().unwrap().id, 1);
    }

    #[test]
    fn backfill_drains_when_live_empty() {
        let q = TranslationQueue::new(4);
        assert_eq!(q.push_backfill(seg(10, StreamKind::System, None, "backfill item one")), 0);
        assert_eq!(q.push_backfill(seg(11, StreamKind::System, None, "backfill item two")), 0);
        assert_eq!(q.live_len(), 0);
        assert_eq!(q.pop().unwrap().id, 10);
        assert_eq!(q.pop().unwrap().id, 11);
    }

    #[test]
    fn backfill_overflow_drops_shortest() {
        // Fill beyond BACKFILL_QUEUE_CAP by pushing many short + one long via push_backfill.
        let q = TranslationQueue::new(1);
        let long = "x".repeat(MIN_SIGNAL_CHARS + 40);
        for i in 0..BACKFILL_QUEUE_CAP {
            assert_eq!(q.push_backfill(seg(i as i64, StreamKind::System, None, &long)), 0);
        }
        // One more short → must drop a short (the new one is shortest) or an equal-signal oldest.
        let dropped = q.push_backfill(seg(999, StreamKind::System, None, "shorty"));
        assert_eq!(dropped, 1);
        assert_eq!(q.backfill_len(), BACKFILL_QUEUE_CAP);
        // The short new item should have been dropped (or an older equal) — len stays at cap.
        let ids: Vec<i64> = (0..BACKFILL_QUEUE_CAP).map(|_| q.pop().unwrap().id).collect();
        assert!(!ids.contains(&999), "shortest overflow victim should be gone");
    }

    #[test]
    fn closing_wakes_a_blocked_worker_and_discards_pending() {
        let q = Arc::new(TranslationQueue::new(4));
        let q2 = q.clone();
        let h = std::thread::spawn(move || q2.pop());
        std::thread::sleep(Duration::from_millis(50));
        q.close();
        assert!(h.join().unwrap().is_none());
        assert_eq!(q.push(seg(1, StreamKind::System, None, "x")), 0);
        assert!(q.is_empty());
    }

    #[test]
    fn hub_passes_everything_through_and_translates_only_when_active() {
        let store = Store::open_in_memory().unwrap();
        let m = store.create_meeting("live").unwrap();
        let inner = Arc::new(VecSink::default());
        let hub = TranslationHub::new(inner.clone(), store.clone(), m);

        let s1 = store
            .insert_segment(&crate::types::NewSegment {
                meeting_id: m,
                stream: StreamKind::System,
                speaker_id: None,
                t0: 0.0,
                t1: 1.0,
                lang: Some("en".into()),
                text: "before".into(), // too short for catch-up
            })
            .unwrap();
        hub.emit(Event::Segment { segment: s1 });
        assert!(!hub.is_active());

        hub.start(Box::new(Upper), "it", 8);
        assert_eq!(hub.target().as_deref(), Some("it"));
        let mk = |text: &str, stream, lang| {
            store
                .insert_segment(&crate::types::NewSegment {
                    meeting_id: m,
                    stream,
                    speaker_id: None,
                    t0: 1.0,
                    t1: 2.0,
                    lang: Some(lang),
                    text: text.into(),
                })
                .unwrap()
        };
        let a = mk("hello there friend", StreamKind::System, "en".to_string());
        let mine = mk("my own words here", StreamKind::Mic, "en".to_string());
        let same = mk("ciao a tutti amici", StreamKind::System, "it".to_string());
        for s in [a.clone(), mine, same] {
            hub.emit(Event::Segment { segment: s });
        }
        wait_until(|| inner.0.lock().unwrap().iter().any(|e| matches!(e, Event::Translation { .. })));
        hub.stop();

        let events = inner.take();
        assert_eq!(events.iter().filter(|e| matches!(e, Event::Segment { .. })).count(), 4, "all segments pass through");
        let tr: Vec<_> = events
            .iter()
            .filter_map(|e| {
                if let Event::Translation { segment_id, text, lang } = e {
                    Some((*segment_id, text.clone(), lang.clone()))
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(tr, [(a.id, "[it] HELLO THERE FRIEND".to_string(), "it".to_string())]);
        assert_eq!(
            store.segments(m).unwrap().iter().find(|s| s.id == a.id).unwrap().translation.as_deref(),
            Some("[it] HELLO THERE FRIEND")
        );
        assert!(!hub.is_active());
    }

    #[test]
    fn start_catchup_translates_prior_eligible_segments() {
        let store = Store::open_in_memory().unwrap();
        let m = store.create_meeting("live").unwrap();
        let inner = Arc::new(VecSink::default());
        let hub = TranslationHub::new(inner.clone(), store.clone(), m);

        let prior = store
            .insert_segment(&crate::types::NewSegment {
                meeting_id: m,
                stream: StreamKind::System,
                speaker_id: None,
                t0: 0.0,
                t1: 1.0,
                lang: Some("en".into()),
                text: "earlier sentence spoken".into(),
            })
            .unwrap();
        let mic = store
            .insert_segment(&crate::types::NewSegment {
                meeting_id: m,
                stream: StreamKind::Mic,
                speaker_id: None,
                t0: 1.0,
                t1: 2.0,
                lang: Some("en".into()),
                text: "my mic before start".into(),
            })
            .unwrap();
        let same_lang = store
            .insert_segment(&crate::types::NewSegment {
                meeting_id: m,
                stream: StreamKind::System,
                speaker_id: None,
                t0: 2.0,
                t1: 3.0,
                lang: Some("it".into()),
                text: "già nella lingua destinazione".into(),
            })
            .unwrap();
        let (prior_id, mic_id, same_id) = (prior.id, mic.id, same_lang.id);
        hub.emit(Event::Segment { segment: prior });
        hub.emit(Event::Segment { segment: mic });
        hub.emit(Event::Segment { segment: same_lang });
        assert!(!hub.is_active());

        hub.start(Box::new(Upper), "it", 8);
        wait_until(|| {
            store
                .segments(m)
                .unwrap()
                .iter()
                .any(|s| s.id == prior_id && s.translation.is_some())
        });
        // Give the worker a beat; mic + same-lang must stay untranslated.
        std::thread::sleep(Duration::from_millis(80));
        hub.stop();

        let events = inner.take();
        let tr: Vec<_> = events
            .iter()
            .filter_map(|e| {
                if let Event::Translation { segment_id, .. } = e {
                    Some(*segment_id)
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(tr, [prior_id]);
        let segs = store.segments(m).unwrap();
        assert!(segs.iter().find(|s| s.id == prior_id).unwrap().translation.is_some());
        assert!(segs.iter().find(|s| s.id == mic_id).unwrap().translation.is_none());
        assert!(segs.iter().find(|s| s.id == same_id).unwrap().translation.is_none());
    }

    #[test]
    fn restarting_with_a_new_target_language_switches_cleanly() {
        let store = Store::open_in_memory().unwrap();
        let m = store.create_meeting("live").unwrap();
        let hub = TranslationHub::new(Arc::new(VecSink::default()), store, m);
        hub.start(Box::new(Upper), "it", 4);
        hub.start(Box::new(Upper), "fr", 4);
        assert_eq!(hub.target().as_deref(), Some("fr"));
        hub.stop();
        hub.stop(); // idempotent
    }

    #[test]
    fn failures_are_reported_once_and_do_not_kill_the_worker() {
        struct Flaky(u32);
        impl Translator for Flaky {
            fn translate(&mut self, t: &str, _: &str) -> Result<String> {
                self.0 += 1;
                if self.0 <= 2 {
                    anyhow::bail!("server down")
                } else {
                    Ok(t.to_uppercase())
                }
            }
        }
        let store = Store::open_in_memory().unwrap();
        let m = store.create_meeting("live").unwrap();
        let inner = Arc::new(VecSink::default());
        let hub = TranslationHub::new(inner.clone(), store.clone(), m);
        hub.start(Box::new(Flaky(0)), "it", 8);
        for i in 0..3 {
            let s = store
                .insert_segment(&crate::types::NewSegment {
                    meeting_id: m,
                    stream: StreamKind::System,
                    speaker_id: None,
                    t0: i as f64,
                    t1: i as f64 + 1.0,
                    lang: Some("en".into()),
                    text: format!("sentence number {i} here"),
                })
                .unwrap();
            hub.emit(Event::Segment { segment: s });
            std::thread::sleep(Duration::from_millis(30));
        }
        wait_until(|| inner.0.lock().unwrap().iter().any(|e| matches!(e, Event::Translation { .. })));
        hub.stop();
        let ev = inner.take();
        assert_eq!(ev.iter().filter(|e| matches!(e, Event::Warning { .. })).count(), 1);
    }

    #[test]
    fn llm_translator_uses_the_model_card_prompt_and_cleans_quotes() {
        let chat = Arc::new(FakeChat::new(|_| Ok("  \"Ciao a tutti\"  ".into())));
        let mut t = LlmTranslator(chat.clone());
        assert_eq!(t.translate("Hello everyone", "it").unwrap(), "Ciao a tutti");
        assert!(chat.calls.lock().unwrap()[0][0].content.starts_with("Translate the following text into Italian."));
        assert!(t.translate("x", "xx").is_err(), "unknown language code");
        let empty = Arc::new(FakeChat::new(|_| Ok("   ".into())));
        assert!(LlmTranslator(empty).translate("x", "it").is_err());
    }
}
