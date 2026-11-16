//! sherpa-onnx engines: Silero VAD, Parakeet (offline transducer) ASR, WeSpeaker embeddings.
//! Compiled only with the `sherpa` feature; everything above it is engine-agnostic.
use crate::pipeline::detector::{EnergyConfig, EnergyDetector, SpeechChunk, SpeechDetector};
use crate::pipeline::{DetectorFactory, Engine, Recognizer, SpeakerEmbedder};
use crate::slots::find_transducer_files;
use crate::types::SAMPLE_RATE;
use anyhow::{anyhow, Context, Result};
use sherpa_onnx::{
    OfflineRecognizer, OfflineRecognizerConfig, OfflineTransducerModelConfig, SileroVadModelConfig, SpeakerEmbeddingExtractor, SpeakerEmbeddingExtractorConfig,
    VadModelConfig, VoiceActivityDetector,
};
use std::path::Path;

pub struct SileroDetector {
    vad: VoiceActivityDetector,
}

fn vad_config(model: &Path) -> VadModelConfig {
    VadModelConfig {
        silero_vad: SileroVadModelConfig {
            model: Some(model.to_string_lossy().into_owned()),
            threshold: 0.4,
            min_silence_duration: 0.5,
            min_speech_duration: 0.2,
            window_size: 512,
            max_speech_duration: 12.0,
        },
        sample_rate: SAMPLE_RATE as i32,
        num_threads: 1,
        provider: Some("cpu".into()),
        ..Default::default()
    }
}

impl SileroDetector {
    pub fn new(model: &Path) -> Result<Self> {
        let vad = VoiceActivityDetector::create(&vad_config(model), 60.0).ok_or_else(|| anyhow!("could not load the voice-activity model {}", model.display()))?;
        Ok(Self { vad })
    }

    fn drain(&self) -> Vec<SpeechChunk> {
        let mut out = Vec::new();
        while let Some(seg) = self.vad.front() {
            out.push(SpeechChunk { start_sample: seg.start().max(0) as u64, samples: seg.samples().to_vec() });
            self.vad.pop();
        }
        out
    }
}

impl SpeechDetector for SileroDetector {
    fn accept(&mut self, samples: &[f32]) -> Vec<SpeechChunk> {
        self.vad.accept_waveform(samples);
        self.drain()
    }
    fn flush(&mut self) -> Vec<SpeechChunk> {
        self.vad.flush();
        self.drain()
    }
}

/// One detector per stream; falls back to the energy detector if the model cannot load.
pub fn detector_factory(model: Option<std::path::PathBuf>) -> DetectorFactory {
    Box::new(move || {
        if let Some(m) = &model {
            match SileroDetector::new(m) {
                Ok(d) => return Box::new(d) as Box<dyn SpeechDetector>,
                Err(e) => log::warn!("{e:#}; falling back to the energy detector"),
            }
        }
        Box::new(EnergyDetector::new(EnergyConfig::default()))
    })
}

pub struct ParakeetRecognizer {
    rec: OfflineRecognizer,
}

impl ParakeetRecognizer {
    /// `dir` holds encoder/decoder/joiner `.onnx` and `tokens.txt`.
    pub fn new(dir: &Path, threads: usize) -> Result<Self> {
        let (encoder, decoder, joiner, tokens) = find_transducer_files(dir)?;
        let mut cfg = OfflineRecognizerConfig::default();
        cfg.model_config.transducer = OfflineTransducerModelConfig { encoder: Some(encoder), decoder: Some(decoder), joiner: Some(joiner) };
        cfg.model_config.tokens = Some(tokens);
        cfg.model_config.model_type = Some("nemo_transducer".into());
        cfg.model_config.num_threads = threads.max(1) as i32;
        cfg.model_config.provider = Some("cpu".into());
        cfg.decoding_method = Some("greedy_search".into());
        let rec = OfflineRecognizer::create(&cfg).ok_or_else(|| anyhow!("could not load the speech-recognition model in {}", dir.display()))?;
        let mut this = Self { rec };
        // The first inference is much slower than later ones (runtime initialisation); pay for it
        // now, while the app shows "starting", not on the user's first sentence.
        let started = std::time::Instant::now();
        let _ = this.transcribe(&vec![0.0f32; 16_000]);
        log::info!("speech model warmed up in {} ms", started.elapsed().as_millis());
        Ok(this)
    }
}

impl Recognizer for ParakeetRecognizer {
    fn transcribe(&mut self, samples: &[f32]) -> Result<String> {
        let stream = self.rec.create_stream();
        stream.accept_waveform(SAMPLE_RATE as i32, samples);
        self.rec.decode(&stream);
        Ok(stream.get_result().context("no recognition result")?.text.trim().to_string())
    }
}

pub struct WeSpeakerEmbedder {
    ex: SpeakerEmbeddingExtractor,
}

impl WeSpeakerEmbedder {
    pub fn new(model: &Path, threads: usize) -> Result<Self> {
        let cfg = SpeakerEmbeddingExtractorConfig { model: Some(model.to_string_lossy().into_owned()), num_threads: threads.max(1) as i32, ..Default::default() };
        let ex = SpeakerEmbeddingExtractor::create(&cfg).ok_or_else(|| anyhow!("could not load the speaker model {}", model.display()))?;
        Ok(Self { ex })
    }
}

impl SpeakerEmbedder for WeSpeakerEmbedder {
    fn embed(&mut self, samples: &[f32]) -> Result<Vec<f32>> {
        let stream = self.ex.create_stream().context("speaker stream")?;
        stream.accept_waveform(SAMPLE_RATE as i32, samples);
        stream.input_finished();
        anyhow::ensure!(self.ex.is_ready(&stream), "audio too short for a voice fingerprint");
        self.ex.compute(&stream).context("no voice fingerprint")
    }
}

/// Loads the recogniser (required) and the speaker model (optional: without it every remote
/// voice is "Speaker 1" rather than failing the whole session).
pub fn build_engine(stt_dir: &Path, speaker_model: Option<&Path>, threads: usize) -> Result<Engine> {
    let rec = ParakeetRecognizer::new(stt_dir, threads)?;
    let emb: Option<Box<dyn SpeakerEmbedder>> = match speaker_model {
        Some(p) => match WeSpeakerEmbedder::new(p, 1) {
            Ok(e) => Some(Box::new(e)),
            Err(e) => {
                log::warn!("{e:#}; speaker recognition disabled");
                None
            }
        },
        None => None,
    };
    Ok(Engine::new(Box::new(rec), emb))
}
