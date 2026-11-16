use anyhow::Result;

/// Speech → text for one VAD chunk (16 kHz mono f32). Implemented by sherpa-onnx in the app,
/// by scripted fakes in tests.
pub trait Recognizer: Send {
    fn transcribe(&mut self, samples: &[f32]) -> Result<String>;
}

/// Voice → fixed-size embedding vector.
pub trait SpeakerEmbedder: Send {
    fn embed(&mut self, samples: &[f32]) -> Result<Vec<f32>>;
}

#[cfg(test)]
pub(crate) mod fakes {
    use super::*;
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};

    /// Returns scripted texts in call order; `Err("boom")` entries simulate engine failures.
    pub struct ScriptedRecognizer(pub VecDeque<Result<String, String>>);
    impl ScriptedRecognizer {
        pub fn texts(t: &[&str]) -> Self {
            Self(t.iter().map(|s| Ok(s.to_string())).collect())
        }
    }
    impl Recognizer for ScriptedRecognizer {
        fn transcribe(&mut self, _: &[f32]) -> Result<String> {
            match self.0.pop_front() {
                Some(Ok(s)) => Ok(s),
                Some(Err(e)) => anyhow::bail!(e),
                None => Ok(String::new()),
            }
        }
    }

    /// "Voice" = loudness of the chunk: peak 0.3 → voice 3, 0.5 → voice 5 (one-hot embedding).
    pub struct LoudnessEmbedder(pub Arc<Mutex<usize>>);
    impl SpeakerEmbedder for LoudnessEmbedder {
        fn embed(&mut self, s: &[f32]) -> Result<Vec<f32>> {
            *self.0.lock().unwrap() += 1;
            let peak = s.iter().fold(0f32, |m, x| m.max(x.abs()));
            let mut v = vec![0.0; 10];
            v[((peak * 10.0).round() as usize).min(9)] = 1.0;
            Ok(v)
        }
    }
}
