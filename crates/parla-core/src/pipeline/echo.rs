//! Text-level echo guard (PLAN D9 defers true AEC).
//!
//! Without a headset the microphone re-records the remote voices, so the same sentence shows
//! up twice: once on the System stream and once, slightly later and noisier, on the Mic stream.
//! Mic segments are therefore held for a short time and dropped if they repeat what the System
//! stream just said.
use std::collections::{HashSet, VecDeque};
use std::time::{Duration, Instant};

fn tokens(text: &str) -> HashSet<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(|t| t.to_lowercase())
        .collect()
}

struct Recent {
    t0: f64,
    t1: f64,
    tokens: HashSet<String>,
}

pub struct EchoGuard {
    recent: VecDeque<Recent>,
    /// Fraction of the mic words that must appear in the system text.
    pub min_overlap: f32,
    /// Mic texts with fewer words are never dropped ("yes", "ok", "I agree").
    pub min_words: usize,
    /// Allowed distance between the two segments in stream time (seconds).
    pub window: f64,
}

impl Default for EchoGuard {
    fn default() -> Self {
        Self { recent: VecDeque::new(), min_overlap: 0.6, min_words: 4, window: 4.0 }
    }
}

impl EchoGuard {
    pub fn note_system(&mut self, t0: f64, t1: f64, text: &str) {
        self.recent.push_back(Recent { t0, t1, tokens: tokens(text) });
        while self.recent.len() > 30 {
            self.recent.pop_front();
        }
    }

    pub fn is_echo(&self, t0: f64, t1: f64, text: &str) -> bool {
        let mic = tokens(text);
        if mic.len() < self.min_words {
            return false;
        }
        self.recent.iter().any(|s| {
            let near = t0 <= s.t1 + self.window && t1 >= s.t0 - self.window;
            near && mic.intersection(&s.tokens).count() as f32 / mic.len() as f32 >= self.min_overlap
        })
    }
}

/// Holds items for `hold`, then releases them in order. Deterministic: callers pass `now`.
pub struct HoldQueue<T> {
    hold: Duration,
    items: VecDeque<(Instant, T)>,
}

impl<T> HoldQueue<T> {
    pub fn new(hold: Duration) -> Self {
        Self { hold, items: VecDeque::new() }
    }
    pub fn push(&mut self, now: Instant, item: T) {
        self.items.push_back((now, item));
    }
    pub fn release_due(&mut self, now: Instant) -> Vec<T> {
        let mut out = vec![];
        while self.items.front().map(|(t, _)| now.duration_since(*t) >= self.hold).unwrap_or(false) {
            out.push(self.items.pop_front().unwrap().1);
        }
        out
    }
    pub fn drain(&mut self) -> Vec<T> {
        self.items.drain(..).map(|(_, v)| v).collect()
    }
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drops_a_noisy_repeat_of_the_system_sentence() {
        let mut g = EchoGuard::default();
        g.note_system(10.0, 14.0, "We should move the release to next Thursday");
        assert!(g.is_echo(10.2, 14.1, "we should move the release to next thursday"));
        assert!(g.is_echo(10.2, 14.1, "should move release next Thursday okay"), "ASR noise tolerated");
    }

    #[test]
    fn keeps_genuinely_different_speech() {
        let mut g = EchoGuard::default();
        g.note_system(10.0, 14.0, "We should move the release to next Thursday");
        assert!(!g.is_echo(10.5, 13.0, "I think Friday would work better for the team"));
    }

    #[test]
    fn short_replies_are_never_dropped() {
        let mut g = EchoGuard::default();
        g.note_system(10.0, 12.0, "Sounds good to me");
        assert!(!g.is_echo(12.5, 13.0, "sounds good"));
    }

    #[test]
    fn same_words_far_apart_in_time_are_not_echo() {
        let mut g = EchoGuard::default();
        g.note_system(10.0, 14.0, "We should move the release to next Thursday");
        assert!(!g.is_echo(120.0, 124.0, "We should move the release to next Thursday"));
    }

    #[test]
    fn hold_queue_releases_in_order_after_the_delay() {
        let t = Instant::now();
        let mut q = HoldQueue::new(Duration::from_secs(2));
        q.push(t, "a");
        q.push(t + Duration::from_secs(1), "b");
        assert!(q.release_due(t + Duration::from_millis(1999)).is_empty());
        assert_eq!(q.release_due(t + Duration::from_secs(2)), ["a"]);
        assert_eq!(q.release_due(t + Duration::from_secs(3)), ["b"]);
        assert!(q.is_empty());
        q.push(t, "c");
        assert_eq!(q.drain(), ["c"]);
    }

    #[test]
    fn recent_memory_is_bounded() {
        let mut g = EchoGuard::default();
        for i in 0..100 {
            g.note_system(i as f64, i as f64 + 1.0, "alpha beta gamma delta");
        }
        assert!(g.recent.len() <= 30);
    }
}
