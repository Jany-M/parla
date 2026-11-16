//! Smoke test against the real models. Skipped unless PARLA_TEST_MODELS points at a folder with
//! stt/ vad/ speaker/ (as laid out by the model downloader) and en.wav.
//!   PARLA_TEST_MODELS=~/models-test cargo test -p parla-core --features sherpa --test real_models -- --nocapture
#![cfg(feature = "sherpa")]
use parla_core::audio::decode_file;
use parla_core::events::VecSink;
use parla_core::pipeline::{Session, SessionConfig, SpeechDetector};
use parla_core::sherpa::{build_engine, SileroDetector};
use parla_core::store::Store;
use parla_core::types::{Frame, StreamKind};
use std::path::PathBuf;
use std::sync::Arc;

fn root() -> Option<PathBuf> {
    std::env::var_os("PARLA_TEST_MODELS").map(PathBuf::from).filter(|p| p.join("en.wav").exists())
}

fn load_wav(p: &std::path::Path) -> Vec<f32> {
    let mut all = vec![];
    decode_file(p, &mut |s| { all.extend_from_slice(s); true }, &mut |_| {}).unwrap();
    all
}

#[test]
fn silero_finds_speech_in_a_real_recording() {
    let Some(r) = root() else { return };
    let audio = load_wav(&r.join("en.wav"));
    let mut d = SileroDetector::new(&r.join("vad/silero_vad.onnx")).unwrap();
    let mut chunks = vec![];
    for c in audio.chunks(1600).chain(vec![0.0f32; 16_000].chunks(1600)) {
        chunks.extend(d.accept(c));
    }
    chunks.extend(d.flush());
    assert!(!chunks.is_empty(), "no speech found");
    let total: usize = chunks.iter().map(|c| c.samples.len()).sum();
    assert!(total as f32 > audio.len() as f32 * 0.4, "VAD kept only {total} of {} samples", audio.len());
}

#[test]
fn full_session_with_real_models_transcribes_and_labels_speakers() {
    let Some(r) = root() else { return };
    let engine = build_engine(&r.join("stt"), Some(&r.join("speaker/wespeaker_en_voxceleb_resnet34.onnx")), 4).unwrap();
    let store = Store::open_in_memory().unwrap();
    let m = store.create_meeting("file").unwrap();
    let sink = Arc::new(VecSink::default());
    let det = parla_core::sherpa::detector_factory(Some(r.join("vad/silero_vad.onnx")));
    let mut s = Session::new(store.clone(), m, engine, sink, SessionConfig::default(), det);
    let mut audio = load_wav(&r.join("en.wav"));
    audio.extend(vec![0.0; 16_000]);
    for c in audio.chunks(1600) {
        s.push(Frame { stream: StreamKind::System, samples: c.to_vec() }).unwrap();
    }
    s.finish().unwrap();
    let segs = store.segments(m).unwrap();
    println!("TRANSCRIPT: {:?}", segs.iter().map(|g| (&g.text, &g.lang)).collect::<Vec<_>>());
    assert!(!segs.is_empty());
    let text = segs.iter().map(|g| g.text.to_lowercase()).collect::<Vec<_>>().join(" ");
    assert!(text.split_whitespace().count() >= 5, "too little text: {text}");
    assert_eq!(segs[0].lang.as_deref(), Some("en"));
    assert_eq!(store.speakers(m).unwrap().len(), 1, "one voice → one speaker");
}
