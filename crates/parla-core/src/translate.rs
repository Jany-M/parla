//! Translation on request: a bounded queue (drop-oldest), a worker thread, and a hub that sits
//! between the pipeline and the UI so translation is a pure add-on to the event stream.
use crate::events::{Event, EventSink};
use crate::languages;
use crate::llm::{prompts, ChatModel, ChatOptions};
use crate::store::Store;
use crate::types::{Segment, StreamKind};
use anyhow::{Context, Result};
use std::collections::VecDeque;
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;

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

/// Only what other people say, and only if it is not already in the target language.
pub fn should_translate(seg: &Segment, target: &str) -> bool {
    seg.stream != StreamKind::Mic
        && seg.text.chars().filter(|c| c.is_alphanumeric()).count() >= 2
        && seg.lang.as_deref().map(|l| !l.eq_ignore_ascii_case(target)).unwrap_or(true)
}

struct QueueState {
    items: VecDeque<Segment>,
    closed: bool,
}

/// Bounded FIFO; when full the OLDEST item is dropped so the overlay stays close to "now".
pub struct TranslationQueue {
    state: Mutex<QueueState>,
    cv: Condvar,
    cap: usize,
}

impl TranslationQueue {
    pub fn new(cap: usize) -> Self {
        Self { state: Mutex::new(QueueState { items: VecDeque::new(), closed: false }), cv: Condvar::new(), cap: cap.max(1) }
    }

    /// Returns how many older items were dropped to make room.
    pub fn push(&self, seg: Segment) -> usize {
        let mut s = self.state.lock().unwrap();
        if s.closed {
            return 0;
        }
        let mut dropped = 0;
        while s.items.len() >= self.cap {
            s.items.pop_front();
            dropped += 1;
        }
        s.items.push_back(seg);
        self.cv.notify_one();
        dropped
    }

    /// Blocks for the next item; `None` once closed (pending items are discarded on close).
    pub fn pop(&self) -> Option<Segment> {
        let mut s = self.state.lock().unwrap();
        loop {
            if s.closed {
                return None;
            }
            if let Some(x) = s.items.pop_front() {
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
        self.state.lock().unwrap().items.len()
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
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
    active: Mutex<Option<Active>>,
}

impl TranslationHub {
    pub fn new(inner: Arc<dyn EventSink>, store: Store) -> Arc<Self> {
        Arc::new(Self { inner, store, active: Mutex::new(None) })
    }

    pub fn is_active(&self) -> bool {
        self.active.lock().unwrap().is_some()
    }

    pub fn target(&self) -> Option<String> {
        self.active.lock().unwrap().as_ref().map(|a| a.target.clone())
    }

    /// (Re)starts translation into `target`. `queue_cap` bounds how far behind it may fall.
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
        *self.active.lock().unwrap() = Some(Active { queue, handle: Some(handle), target: target.to_string() });
    }

    pub fn stop(&self) {
        let taken = self.active.lock().unwrap().take();
        if let Some(mut a) = taken {
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
                        log::warn!("translation is behind: dropped {dropped} older sentence(s)");
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
        Segment { id, meeting_id: 1, stream, speaker_id: None, t0: 0.0, t1: 1.0, lang: lang.map(Into::into), text: text.into(), translation: None, translation_lang: None }
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
        assert!(should_translate(&seg(1, StreamKind::System, Some("en"), "hello there"), "it"));
        assert!(!should_translate(&seg(1, StreamKind::System, Some("it"), "ciao a tutti"), "it"), "already in target language");
        assert!(!should_translate(&seg(1, StreamKind::Mic, Some("en"), "hello there"), "it"), "never translate myself");
        assert!(should_translate(&seg(1, StreamKind::System, None, "ok"), "it"), "unknown language → translate");
        assert!(!should_translate(&seg(1, StreamKind::System, None, "?"), "it"));
        assert!(should_translate(&seg(1, StreamKind::File, Some("fr"), "bonjour à tous"), "en"));
    }

    #[test]
    fn queue_drops_the_oldest_when_full() {
        let q = TranslationQueue::new(2);
        assert_eq!(q.push(seg(1, StreamKind::System, None, "a")), 0);
        assert_eq!(q.push(seg(2, StreamKind::System, None, "b")), 0);
        assert_eq!(q.push(seg(3, StreamKind::System, None, "c")), 1);
        assert_eq!((q.pop().unwrap().id, q.pop().unwrap().id), (2, 3));
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
        let hub = TranslationHub::new(inner.clone(), store.clone());

        let s1 = store.insert_segment(&crate::types::NewSegment { meeting_id: m, stream: StreamKind::System, speaker_id: None, t0: 0.0, t1: 1.0, lang: Some("en".into()), text: "before start".into() }).unwrap();
        hub.emit(Event::Segment { segment: s1 });
        assert!(!hub.is_active());

        hub.start(Box::new(Upper), "it", 8);
        assert_eq!(hub.target().as_deref(), Some("it"));
        let mk = |text: &str, stream, lang| {
            store.insert_segment(&crate::types::NewSegment { meeting_id: m, stream, speaker_id: None, t0: 1.0, t1: 2.0, lang: Some(lang), text: text.into() }).unwrap()
        };
        let a = mk("hello there", StreamKind::System, "en".to_string());
        let mine = mk("my own words", StreamKind::Mic, "en".to_string());
        let same = mk("ciao a tutti", StreamKind::System, "it".to_string());
        for s in [a.clone(), mine, same] {
            hub.emit(Event::Segment { segment: s });
        }
        wait_until(|| inner.0.lock().unwrap().iter().any(|e| matches!(e, Event::Translation { .. })));
        hub.stop();

        let events = inner.take();
        assert_eq!(events.iter().filter(|e| matches!(e, Event::Segment { .. })).count(), 4, "all segments pass through");
        let tr: Vec<_> = events.iter().filter_map(|e| if let Event::Translation { segment_id, text, lang } = e { Some((*segment_id, text.clone(), lang.clone())) } else { None }).collect();
        assert_eq!(tr, [(a.id, "[it] HELLO THERE".to_string(), "it".to_string())]);
        assert_eq!(store.segments(m).unwrap().iter().find(|s| s.id == a.id).unwrap().translation.as_deref(), Some("[it] HELLO THERE"));
        assert!(!hub.is_active());
    }

    #[test]
    fn restarting_with_a_new_target_language_switches_cleanly() {
        let store = Store::open_in_memory().unwrap();
        let hub = TranslationHub::new(Arc::new(VecSink::default()), store);
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
                if self.0 <= 2 { anyhow::bail!("server down") } else { Ok(t.to_uppercase()) }
            }
        }
        let store = Store::open_in_memory().unwrap();
        let m = store.create_meeting("live").unwrap();
        let inner = Arc::new(VecSink::default());
        let hub = TranslationHub::new(inner.clone(), store.clone());
        hub.start(Box::new(Flaky(0)), "it", 8);
        for i in 0..3 {
            let s = store.insert_segment(&crate::types::NewSegment { meeting_id: m, stream: StreamKind::System, speaker_id: None, t0: i as f64, t1: i as f64 + 1.0, lang: Some("en".into()), text: format!("sentence {i}") }).unwrap();
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
