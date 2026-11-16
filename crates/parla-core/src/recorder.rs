//! Joins audio sources to a `Session`. Live recording and file import are the same code:
//! only the list of sources differs.
use crate::audio::AudioSource;
use crate::events::{Event, EventSink};
use crate::pipeline::Session;
use crate::store::Store;
use anyhow::Result;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::sync_channel;
use std::sync::Arc;

/// Runs every source on its own thread feeding one session; returns when all sources have
/// ended (or `stop` was set) and the session has flushed. A failing source is reported as a
/// warning and does not stop the others.
pub fn run(session: Session, sources: Vec<Box<dyn AudioSource>>, stop: Arc<AtomicBool>, sink: Arc<dyn EventSink>) -> Result<()> {
    let (tx, rx) = sync_channel(256);
    let mut handles = Vec::new();
    let total = sources.len();
    let failures: Arc<std::sync::Mutex<Vec<String>>> = Default::default();
    for mut src in sources {
        let (tx, stop, sink, failures) = (tx.clone(), stop.clone(), sink.clone(), failures.clone());
        handles.push(std::thread::spawn(move || {
            if let Err(e) = src.run(&tx, &stop) {
                log::warn!("audio source failed: {e:#}");
                sink.emit(Event::Warning { message: format!("Audio problem: {e}") });
                failures.lock().unwrap().push(format!("{e:#}"));
            }
        }));
    }
    drop(tx);
    let result = session.run(rx);
    for h in handles {
        let _ = h.join();
    }
    result?;
    // One broken source is a warning; if *every* source failed there is nothing to keep.
    let failures = failures.lock().unwrap();
    if total > 0 && failures.len() == total {
        anyhow::bail!("{}", failures[0]);
    }
    Ok(())
}

/// File title from its name ("Q3 planning.mp4" → "Q3 planning").
pub fn title_from_path(p: &std::path::Path) -> String {
    p.file_stem().map(|s| s.to_string_lossy().into_owned()).filter(|s| !s.is_empty()).unwrap_or_else(|| "Imported file".into())
}

/// Convenience used by both entry points once the session has finished.
pub fn finish_meeting(store: &Store, meeting_id: i64) -> Result<()> {
    store.end_meeting(meeting_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::file::tests::wav_bytes;
    use crate::audio::resample::tests::sine;
    use crate::audio::FileSource;
    use crate::events::VecSink;
    use crate::pipeline::engines::fakes::ScriptedRecognizer;
    use crate::pipeline::{Engine, Session, SessionConfig};
    use crate::types::{Frame, StreamKind};
    use std::sync::mpsc::SyncSender;
    use std::time::Duration;

    struct Failing;
    impl AudioSource for Failing {
        fn run(&mut self, _: &SyncSender<Frame>, _: &AtomicBool) -> Result<()> {
            anyhow::bail!("device unplugged")
        }
    }

    /// Sends tone bursts on a stream, like a loopback device would.
    struct Tone(StreamKind);
    impl AudioSource for Tone {
        fn run(&mut self, tx: &SyncSender<Frame>, stop: &AtomicBool) -> Result<()> {
            let mut audio = vec![0.0f32; 8000];
            audio.extend(sine(16_000, 1.5, 200.0, 0.3));
            audio.extend(vec![0.0; 16_000]);
            for c in audio.chunks(1600) {
                if stop.load(std::sync::atomic::Ordering::SeqCst) || tx.send(Frame { stream: self.0, samples: c.to_vec() }).is_err() {
                    break;
                }
            }
            Ok(())
        }
    }

    fn session(texts: &[&str], store: &Store, sink: Arc<VecSink>) -> (Session, i64) {
        let m = store.create_meeting("live").unwrap();
        let cfg = SessionConfig { echo_hold: Duration::ZERO, ..Default::default() };
        (Session::new(store.clone(), m, Engine::new(Box::new(ScriptedRecognizer::texts(texts)), None), sink, cfg, Session::energy_detector_factory()), m)
    }

    #[test]
    fn two_sources_feed_one_session_and_both_streams_are_stored() {
        let store = Store::open_in_memory().unwrap();
        let sink = Arc::new(VecSink::default());
        let (s, m) = session(&["what the remote person said", "something entirely different from me"], &store, sink.clone());
        run(s, vec![Box::new(Tone(StreamKind::System)), Box::new(Tone(StreamKind::Mic))], Arc::new(AtomicBool::new(false)), sink).unwrap();
        let segs = store.segments(m).unwrap();
        assert_eq!(segs.len(), 2);
        let mut kinds: Vec<_> = segs.iter().map(|s| s.stream).collect();
        kinds.sort_by_key(|k| k.as_str());
        assert_eq!(kinds, [StreamKind::Mic, StreamKind::System]);
    }

    #[test]
    fn a_failing_source_warns_but_the_other_still_records() {
        let store = Store::open_in_memory().unwrap();
        let sink = Arc::new(VecSink::default());
        let (s, m) = session(&["still recorded despite the failure"], &store, sink.clone());
        run(s, vec![Box::new(Failing), Box::new(Tone(StreamKind::System))], Arc::new(AtomicBool::new(false)), sink.clone()).unwrap();
        assert_eq!(store.segments(m).unwrap().len(), 1);
        assert!(sink.take().iter().any(|e| matches!(e, Event::Warning { message } if message.contains("device unplugged"))));
    }

    #[test]
    fn when_every_source_fails_the_run_fails() {
        let store = Store::open_in_memory().unwrap();
        let sink = Arc::new(VecSink::default());
        let (s, _) = session(&[], &store, sink.clone());
        let e = run(s, vec![Box::new(Failing)], Arc::new(AtomicBool::new(false)), sink).unwrap_err().to_string();
        assert!(e.contains("device unplugged"));
    }

    #[test]
    fn importing_a_file_end_to_end() {
        let t = tempfile::tempdir().unwrap();
        let mut audio = vec![0.0f32; 16_000];
        audio.extend(sine(16_000, 2.0, 200.0, 0.3));
        audio.extend(vec![0.0; 16_000]);
        let p = t.path().join("Q3 planning.wav");
        std::fs::write(&p, wav_bytes(16_000, 1, &audio)).unwrap();
        let store = Store::open_in_memory().unwrap();
        let sink = Arc::new(VecSink::default());
        let (s, m) = session(&["Buongiorno a tutti, oggi parleremo del budget per il prossimo trimestre e delle nuove assunzioni"], &store, sink.clone());
        let sink2 = sink.clone();
        let src = FileSource { path: p.clone(), progress: Box::new(move |f| sink2.emit(Event::Progress { meeting_id: m, fraction: f })) };
        run(s, vec![Box::new(src)], Arc::new(AtomicBool::new(false)), sink.clone()).unwrap();
        store.set_title(m, &title_from_path(&p)).unwrap();
        finish_meeting(&store, m).unwrap();
        let d = store.get_meeting(m).unwrap().unwrap();
        assert_eq!(d.meeting.title, "Q3 planning");
        assert!(d.meeting.ended_at.is_some());
        assert_eq!(d.segments.len(), 1);
        assert_eq!(d.segments[0].stream, StreamKind::File);
        assert_eq!(d.segments[0].lang.as_deref(), Some("it"));
        assert!(sink.take().iter().any(|e| matches!(e, Event::Progress { fraction, .. } if *fraction >= 1.0)));
    }

    #[test]
    fn title_fallbacks() {
        assert_eq!(title_from_path(std::path::Path::new("/a/b/Call 2.mp4")), "Call 2");
        assert_eq!(title_from_path(std::path::Path::new("/")), "Imported file");
    }
}
