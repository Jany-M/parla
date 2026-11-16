use crate::types::SAMPLE_RATE;

/// One utterance cut out of a continuous stream.
#[derive(Clone, Debug, PartialEq)]
pub struct SpeechChunk {
    /// Absolute index (in this stream) of the first sample.
    pub start_sample: u64,
    pub samples: Vec<f32>,
}

impl SpeechChunk {
    pub fn t0(&self) -> f64 {
        self.start_sample as f64 / SAMPLE_RATE as f64
    }
    pub fn duration(&self) -> f64 {
        self.samples.len() as f64 / SAMPLE_RATE as f64
    }
}

/// Splits a continuous stream into utterances. Feed it any chunk size; results are identical.
pub trait SpeechDetector: Send {
    fn accept(&mut self, samples: &[f32]) -> Vec<SpeechChunk>;
    /// End of stream: emit whatever is still being spoken.
    fn flush(&mut self) -> Vec<SpeechChunk>;
}

#[derive(Clone, Debug)]
pub struct EnergyConfig {
    /// RMS level that counts as speech.
    pub threshold: f32,
    pub window: usize,
    /// Silence (samples) that ends an utterance.
    pub hangover: usize,
    /// Utterances with less voiced audio than this are dropped (clicks, coughs).
    pub min_speech: usize,
    /// Utterances are force-split at this length so ASR/translation lag stays bounded.
    pub max_speech: usize,
    /// Audio kept from before the first voiced window (so word onsets are not clipped).
    pub preroll: usize,
}

impl Default for EnergyConfig {
    fn default() -> Self {
        let sr = SAMPLE_RATE as usize;
        Self { threshold: 0.012, window: sr / 50, hangover: sr * 6 / 10, min_speech: sr / 4, max_speech: sr * 12, preroll: sr / 5 }
    }
}

enum State {
    Idle,
    Speaking { start: u64, buf: Vec<f32>, voiced: usize, silence: usize },
}

/// Pure-Rust energy detector: the always-available fallback and the deterministic engine for
/// tests. The app prefers the Silero model (see `sherpa` feature) when it is installed.
pub struct EnergyDetector {
    cfg: EnergyConfig,
    pending: Vec<f32>,
    consumed: u64,
    preroll: Vec<f32>,
    state: State,
}

fn rms(w: &[f32]) -> f32 {
    if w.is_empty() {
        return 0.0;
    }
    (w.iter().map(|x| x * x).sum::<f32>() / w.len() as f32).sqrt()
}

impl EnergyDetector {
    pub fn new(cfg: EnergyConfig) -> Self {
        Self { cfg, pending: Vec::new(), consumed: 0, preroll: Vec::new(), state: State::Idle }
    }

    fn emit(&self, start: u64, mut buf: Vec<f32>, voiced: usize, silence: usize, out: &mut Vec<SpeechChunk>) {
        if voiced < self.cfg.min_speech {
            return;
        }
        // Keep at most `preroll` of the trailing silence so t1 reflects the end of speech.
        let drop = silence.saturating_sub(self.cfg.preroll).min(buf.len());
        buf.truncate(buf.len() - drop);
        out.push(SpeechChunk { start_sample: start, samples: buf });
    }

    fn window(&mut self, w: &[f32], out: &mut Vec<SpeechChunk>) {
        let voiced_now = rms(w) >= self.cfg.threshold;
        let wstart = self.consumed;
        self.consumed += w.len() as u64;
        match std::mem::replace(&mut self.state, State::Idle) {
            State::Idle => {
                if voiced_now {
                    let mut buf = std::mem::take(&mut self.preroll);
                    let start = wstart - buf.len() as u64;
                    buf.extend_from_slice(w);
                    self.state = State::Speaking { start, buf, voiced: w.len(), silence: 0 };
                } else {
                    self.preroll.extend_from_slice(w);
                    let over = self.preroll.len().saturating_sub(self.cfg.preroll);
                    self.preroll.drain(..over);
                }
            }
            State::Speaking { start, mut buf, mut voiced, mut silence } => {
                buf.extend_from_slice(w);
                if voiced_now {
                    voiced += w.len();
                    silence = 0;
                } else {
                    silence += w.len();
                }
                if silence >= self.cfg.hangover {
                    self.emit(start, buf, voiced, silence, out);
                } else if buf.len() >= self.cfg.max_speech {
                    self.emit(start, buf, voiced, 0, out);
                    self.state = State::Speaking { start: self.consumed, buf: Vec::new(), voiced: 0, silence: 0 };
                } else {
                    self.state = State::Speaking { start, buf, voiced, silence };
                }
            }
        }
    }
}

impl SpeechDetector for EnergyDetector {
    fn accept(&mut self, samples: &[f32]) -> Vec<SpeechChunk> {
        let mut out = Vec::new();
        self.pending.extend_from_slice(samples);
        let w = self.cfg.window;
        let mut i = 0;
        while self.pending.len() - i >= w {
            let win: Vec<f32> = self.pending[i..i + w].to_vec();
            self.window(&win, &mut out);
            i += w;
        }
        self.pending.drain(..i);
        out
    }

    fn flush(&mut self) -> Vec<SpeechChunk> {
        let mut out = Vec::new();
        if !self.pending.is_empty() {
            let rest = std::mem::take(&mut self.pending);
            self.window(&rest, &mut out);
        }
        if let State::Speaking { start, buf, voiced, silence } = std::mem::replace(&mut self.state, State::Idle) {
            self.emit(start, buf, voiced, silence, &mut out);
        }
        self.preroll.clear();
        out
    }
}

#[cfg(test)]
pub(crate) mod testutil {
    use crate::types::SAMPLE_RATE;
    pub fn tone(freq: f32, secs: f32, amp: f32) -> Vec<f32> {
        (0..(secs * SAMPLE_RATE as f32) as usize)
            .map(|i| amp * (2.0 * std::f32::consts::PI * freq * i as f32 / SAMPLE_RATE as f32).sin())
            .collect()
    }
    pub fn silence(secs: f32) -> Vec<f32> {
        vec![0.0; (secs * SAMPLE_RATE as f32) as usize]
    }
    pub fn cat(parts: &[Vec<f32>]) -> Vec<f32> {
        parts.concat()
    }
}

#[cfg(test)]
mod tests {
    use super::testutil::*;
    use super::*;

    fn run(samples: &[f32], chunk: usize) -> Vec<SpeechChunk> {
        let mut d = EnergyDetector::new(EnergyConfig::default());
        let mut out = vec![];
        for c in samples.chunks(chunk) {
            out.extend(d.accept(c));
        }
        out.extend(d.flush());
        out
    }

    #[test]
    fn silence_yields_nothing() {
        assert!(run(&silence(5.0), 1600).is_empty());
    }

    #[test]
    fn one_burst_is_one_chunk_with_correct_timing() {
        let a = cat(&[silence(1.0), tone(220.0, 1.0, 0.3), silence(2.0)]);
        let c = run(&a, 1600);
        assert_eq!(c.len(), 1);
        // speech starts at 1.0 s; preroll may pull the start back by <= 0.2 s
        assert!(c[0].t0() >= 0.79 && c[0].t0() <= 1.0, "t0={}", c[0].t0());
        let end = c[0].t0() + c[0].duration();
        assert!(end >= 2.0 && end <= 2.25, "end={end}");
    }

    #[test]
    fn gap_longer_than_hangover_splits_shorter_merges() {
        let split = cat(&[tone(220.0, 1.0, 0.3), silence(1.0), tone(220.0, 1.0, 0.3), silence(1.0)]);
        assert_eq!(run(&split, 1600).len(), 2);
        let merged = cat(&[tone(220.0, 1.0, 0.3), silence(0.3), tone(220.0, 1.0, 0.3), silence(1.0)]);
        assert_eq!(run(&merged, 1600).len(), 1);
    }

    #[test]
    fn long_speech_is_force_split_at_max_length() {
        let c = run(&cat(&[tone(220.0, 30.0, 0.3), silence(1.0)]), 1600);
        assert_eq!(c.len(), 3, "30 s of speech with a 12 s cap → 12 + 12 + 6");
        assert!(c.iter().all(|x| x.duration() <= 12.3));
        for w in c.windows(2) {
            assert_eq!(w[0].start_sample + w[0].samples.len() as u64, w[1].start_sample, "no gaps between splits");
        }
    }

    #[test]
    fn clicks_shorter_than_min_speech_are_dropped() {
        let a = cat(&[silence(1.0), tone(220.0, 0.1, 0.5), silence(2.0)]);
        assert!(run(&a, 1600).is_empty());
    }

    #[test]
    fn chunk_size_does_not_change_the_result() {
        let a = cat(&[silence(0.5), tone(180.0, 2.0, 0.3), silence(1.0), tone(300.0, 1.5, 0.4), silence(1.0)]);
        let whole = run(&a, a.len());
        assert_eq!(whole.len(), 2);
        for size in [1, 7, 320, 1000, 4097] {
            assert_eq!(run(&a, size), whole, "chunk size {size}");
        }
    }

    #[test]
    fn flush_emits_speech_in_progress() {
        let a = tone(220.0, 1.0, 0.3);
        let c = run(&a, 1600);
        assert_eq!(c.len(), 1);
        assert!(c[0].duration() >= 0.99);
    }

    #[test]
    fn quiet_noise_below_threshold_is_ignored() {
        assert!(run(&tone(220.0, 3.0, 0.005), 1600).is_empty());
    }
}
