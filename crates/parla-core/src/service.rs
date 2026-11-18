//! Everything the UI can ask for, in one place, independent of Tauri (so it is unit-testable).
//! The shell only forwards commands here and events out.
use crate::audio::{AudioSource, FileSource};
use crate::events::{Event, EventSink};
use crate::hardware::{self, Recommendation};
use crate::languages;
use crate::llm::{ChatModel, LlamaConfig, LlamaServer, LlmClient};
use crate::models::{self, manifest::current_platform, Cancel, Item, Manifest, Slot};
use crate::paths::AppPaths;
use crate::pipeline::{DetectorFactory, Engine, Session, SessionConfig};
use crate::recorder;
use crate::settings::Settings;
use crate::slots::SlotChoice;
use crate::store::Store;
use crate::summary::{self, SummaryConfig};
use crate::translate::{LlmTranslator, TranslationHub};
use anyhow::{anyhow, bail, Context, Result};
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

/// Manifest ids the app itself depends on (the LLM ids come from `hardware::recommend`).
pub mod ids {
    pub const STT: &str = "stt-parakeet-tdt-0.6b-v3-int8";
    pub const VAD: &str = "vad-silero";
    pub const SPEAKER: &str = "spk-wespeaker-resnet34";
}

/// Paths handed to the engine loader (the only place that touches ONNX).
#[derive(Clone, Debug)]
pub struct EnginePaths {
    pub stt_dir: PathBuf,
    pub speaker_model: Option<PathBuf>,
    pub vad_model: Option<PathBuf>,
    pub threads: usize,
}

pub type EngineLoader = Arc<dyn Fn(&EnginePaths) -> Result<(Engine, DetectorFactory)> + Send + Sync>;
pub type SourceFactory = Arc<dyn Fn(&Settings) -> Result<Vec<Box<dyn AudioSource>>> + Send + Sync>;

#[derive(Clone, Debug, Serialize)]
pub struct ModelStatus {
    pub id: String,
    pub slot: Slot,
    pub label: String,
    pub description: String,
    pub license: String,
    /// This machine has enough RAM (and passes other gates) to download/use this model.
    pub eligible: bool,
    pub size_bytes: u64,
    pub installed: bool,
    /// Chosen for this machine (RAM tier) or always required.
    pub recommended: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct SystemInfo {
    pub hardware: hardware::HardwareInfo,
    pub recommendation: Recommendation,
    pub disk_usage_bytes: u64,
    pub data_dir: String,
}

struct Recording {
    meeting_id: i64,
    stop: Arc<AtomicBool>,
    paused: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
    hub: Arc<TranslationHub>,
}

pub struct Service {
    pub paths: AppPaths,
    store: Store,
    manifest: Manifest,
    settings: Mutex<Settings>,
    sink: Arc<dyn EventSink>,
    loader: EngineLoader,
    sources: SourceFactory,
    rec: Arc<Mutex<Option<Recording>>>,
    mt_server: Mutex<Option<LlamaServer>>,
    hw: hardware::HardwareInfo,
}

fn find_file(dir: &Path, pred: &dyn Fn(&str) -> bool) -> Option<PathBuf> {
    for e in std::fs::read_dir(dir).ok()?.filter_map(|e| e.ok()) {
        let p = e.path();
        if p.is_dir() {
            if let Some(f) = find_file(&p, pred) {
                return Some(f);
            }
        } else if pred(&e.file_name().to_string_lossy().to_lowercase()) {
            return Some(p);
        }
    }
    None
}

impl Service {
    pub fn new(paths: AppPaths, manifest: Manifest, sink: Arc<dyn EventSink>, loader: EngineLoader, sources: SourceFactory) -> Result<Arc<Self>> {
        Self::with_hardware(paths, manifest, sink, loader, sources, hardware::detect())
    }

    fn with_hardware(
        paths: AppPaths,
        manifest: Manifest,
        sink: Arc<dyn EventSink>,
        loader: EngineLoader,
        sources: SourceFactory,
        hw: hardware::HardwareInfo,
    ) -> Result<Arc<Self>> {
        paths.ensure_dirs()?;
        let store = Store::open(&paths.db_path())?;
        store.close_dangling_meetings()?; // a crash/kill must not leave "recording…" meetings behind
        let settings = Settings::load(&paths.settings_path());
        Ok(Arc::new(Self {
            paths,
            store,
            manifest,
            settings: Mutex::new(settings),
            sink,
            loader,
            sources,
            rec: Arc::new(Mutex::new(None)),
            mt_server: Mutex::new(None),
            hw,
        }))
    }

    pub fn store(&self) -> &Store {
        &self.store
    }

    // ---- settings & system -------------------------------------------------

    pub fn settings(&self) -> Settings {
        self.settings.lock().unwrap().clone()
    }

    pub fn update_settings(&self, s: Settings) -> Result<()> {
        for (choice, slot) in [(&s.slots.stt, Slot::Stt), (&s.slots.speaker, Slot::Speaker), (&s.slots.translate, Slot::Translate), (&s.slots.summarize, Slot::Summarize)] {
            crate::slots::validate_choice(choice, slot)?;
            if let SlotChoice::Managed { item_id: Some(id) } = choice {
                self.ensure_eligible(id)?;
            }
        }
        languages::by_code(&s.target_lang).with_context(|| format!("unsupported language '{}'", s.target_lang))?;
        s.save(&self.paths.settings_path())?;
        *self.settings.lock().unwrap() = s;
        Ok(())
    }

    pub fn recommendation(&self) -> Recommendation {
        hardware::recommend(&self.hw)
    }

    pub fn system_info(&self) -> SystemInfo {
        SystemInfo { hardware: self.hw.clone(), recommendation: self.recommendation(), disk_usage_bytes: self.paths.disk_usage(), data_dir: self.paths.root.display().to_string() }
    }

    // ---- models ------------------------------------------------------------

    pub fn model_status(&self) -> Vec<ModelStatus> {
        let rec = self.recommendation();
        let want = [ids::STT, ids::VAD, ids::SPEAKER, rec.summarize_item, rec.translate_item];
        self.manifest
            .items
            .iter()
            .filter(|i| i.matches_platform(current_platform()) && i.matches_arch(crate::models::manifest::current_arch()))
            .map(|i| ModelStatus {
                id: i.id.clone(),
                slot: i.slot,
                label: i.label.clone(),
                description: i.description.clone(),
                license: i.license.clone(),
                eligible: hardware::can_use(&self.hw, &i.id),
                size_bytes: i.total_bytes(),
                installed: models::is_installed(&self.paths.models_dir(), &i.id),
                recommended: want.contains(&i.id.as_str()) || i.slot == Slot::Runtime,
            })
            .collect()
    }

    fn item(&self, id: &str) -> Result<&Item> {
        self.manifest.get(id).ok_or_else(|| anyhow!("unknown model '{id}'"))
    }

    fn ensure_eligible(&self, id: &str) -> Result<()> {
        if hardware::can_use(&self.hw, id) {
            return Ok(());
        }
        let need = hardware::min_ram_mb_for(id).unwrap_or(0);
        bail!(
            "This model needs about {} GB of RAM; this computer has about {} GB. Use the smaller summary model instead.",
            (need + 500) / 1000,
            (self.hw.total_ram_mb + 500) / 1000
        )
    }

    /// Blocking; call from a worker thread. Runtime items are pulled in automatically with an LLM.
    pub fn install_model(&self, id: &str, cancel: &Cancel, progress: &mut dyn FnMut(u64, u64)) -> Result<()> {
        self.ensure_eligible(id)?;
        let item = self.item(id)?.clone();
        models::install_item(&self.paths.models_dir(), &item, cancel, progress)?;
        Ok(())
    }

    pub fn uninstall_model(&self, id: &str) -> Result<()> {
        self.item(id)?;
        models::uninstall_item(&self.paths.models_dir(), id)
    }

    pub fn remove_all_models(&self) -> Result<()> {
        self.stop_translation();
        self.paths.wipe_models()
    }

    fn installed(&self, id: &str) -> Option<PathBuf> {
        models::is_installed(&self.paths.models_dir(), id).then(|| models::installed_dir(&self.paths.models_dir(), id))
    }

    fn managed_dir(&self, choice: &SlotChoice, default_id: &str, what: &str) -> Result<PathBuf> {
        match choice {
            SlotChoice::LocalFile { path } => Ok(PathBuf::from(path)),
            SlotChoice::Managed { item_id } => {
                let id = item_id.as_deref().unwrap_or(default_id);
                self.installed(id).ok_or_else(|| anyhow!("The {what} is not installed yet. Open Settings → Models to download it."))
            }
            SlotChoice::Endpoint { .. } => bail!("an endpoint cannot be used for the {what}"),
        }
    }

    fn engine_paths(&self, s: &Settings) -> Result<EnginePaths> {
        let stt_dir = self.managed_dir(&s.slots.stt, ids::STT, "speech-recognition model")?;
        let speaker_model = match &s.slots.speaker {
            SlotChoice::LocalFile { path } => Some(PathBuf::from(path)),
            _ => self.installed(ids::SPEAKER).and_then(|d| find_file(&d, &|n| n.ends_with(".onnx"))),
        };
        let vad_model = self.installed(ids::VAD).and_then(|d| find_file(&d, &|n| n.ends_with(".onnx")));
        Ok(EnginePaths { stt_dir, speaker_model, vad_model, threads: self.recommendation().threads })
    }

    // ---- language models (translation / summary) --------------------------

    /// Returns the chat client and, for local models, the running server that must outlive it.
    fn chat_model(&self, choice: &SlotChoice, default_item: &str, ctx: u32, what: &str) -> Result<(Arc<dyn ChatModel>, Option<LlamaServer>)> {
        let gguf = match choice {
            SlotChoice::Endpoint { url, model, api_key } => {
                if !crate::slots::is_local_url(url) {
                    log::warn!("{what} uses a non-local endpoint: text will leave this computer");
                }
                return Ok((Arc::new(LlmClient::new(url, model, api_key.clone(), Duration::from_secs(180))?), None));
            }
            SlotChoice::LocalFile { path } => PathBuf::from(path),
            SlotChoice::Managed { item_id } => {
                let id = item_id.as_deref().unwrap_or(default_item);
                let dir = self.installed(id).ok_or_else(|| anyhow!("The {what} model is not installed yet. Open Settings → Models to download it."))?;
                find_file(&dir, &|n| n.ends_with(".gguf")).ok_or_else(|| anyhow!("the {what} model folder has no .gguf file"))?
            }
        };
        let runtime = self
            .manifest
            .for_slot(Slot::Runtime, current_platform())
            .into_iter()
            .find_map(|i| self.installed(&i.id))
            .ok_or_else(|| anyhow!("The language-model runtime is not installed yet. Open Settings → Models to download it."))?;
        let exe_name = if cfg!(windows) { "llama-server.exe" } else { "llama-server" };
        let exe = find_file(&runtime, &|n| n == exe_name).ok_or_else(|| anyhow!("llama-server was not found in the installed runtime"))?;
        let server = LlamaServer::start(&LlamaConfig {
            exe,
            model: gguf,
            ctx,
            threads: self.recommendation().threads,
            log_file: Some(self.paths.logs_dir().join("llama.log")),
            ready_timeout: Duration::from_secs(180),
        })?;
        let client = LlmClient::new(&server.base_url(), "local", None, Duration::from_secs(300))?;
        Ok((Arc::new(client), Some(server)))
    }

    pub fn summarize(&self, meeting_id: i64, progress: &dyn Fn(usize, usize)) -> Result<String> {
        let s = self.settings();
        let detail = self.store.get_meeting(meeting_id)?.context("meeting not found")?;
        let lines = crate::export::transcript_lines(&detail);
        let lang = languages::by_code(&s.summary_lang).map(|l| l.name).unwrap_or("English");
        let (model, _server) = self.chat_model(&s.slots.summarize, self.recommendation().summarize_item, 8192, "summary")?;
        let text = summary::summarize(model.as_ref(), &lines, lang, &SummaryConfig::default(), progress)?;
        self.store.set_summary(meeting_id, &text)?;
        Ok(text) // `_server` drops here: the model is unloaded again
    }

    // ---- live recording ----------------------------------------------------

    pub fn is_recording(&self) -> Option<i64> {
        self.rec.lock().unwrap().as_ref().map(|r| r.meeting_id)
    }

    fn session_for(&self, meeting_id: i64, sink: Arc<dyn EventSink>, s: &Settings, paused: Arc<AtomicBool>) -> Result<Session> {
        let (engine, detector) = (self.loader)(&self.engine_paths(s)?)?;
        let cfg = SessionConfig { echo_guard: s.echo_guard, ..Default::default() };
        Ok(Session::new(self.store.clone(), meeting_id, engine, sink, cfg, detector).with_pause(paused))
    }

    pub fn start_live(&self) -> Result<i64> {
        if self.is_recording().is_some() {
            bail!("a recording is already running");
        }
        let s = self.settings();
        let hub = TranslationHub::new(self.sink.clone(), self.store.clone());
        let meeting_id = self.store.create_meeting("live")?;
        let paused = Arc::new(AtomicBool::new(false));
        let started = (|| -> Result<(Session, Vec<Box<dyn AudioSource>>)> {
            let mut session = self.session_for(meeting_id, hub.clone(), &s, paused.clone())?;
            session.prepare(&[crate::types::StreamKind::Mic, crate::types::StreamKind::System]);
            Ok((session, (self.sources)(&s)?))
        })();
        let (session, sources) = match started {
            Ok(x) => x,
            Err(e) => {
                let _ = self.store.delete_meeting(meeting_id); // nothing was recorded
                return Err(e);
            }
        };
        let stop = Arc::new(AtomicBool::new(false));
        let (store, sink, stop2, hub2, rec_slot) = (self.store.clone(), self.sink.clone(), stop.clone(), hub.clone(), self.rec.clone());
        let handle = std::thread::spawn(move || {
            let sink_dyn: Arc<dyn EventSink> = hub2.clone();
            if let Err(e) = recorder::run(session, sources, stop2, sink_dyn) {
                log::error!("recording problem: {e:#}");
                sink.emit(Event::Warning { message: format!("Recording problem: {e}") });
            }
            hub2.stop();
            // a recording that produced no transcript is not worth keeping
            let saved = store.segments(meeting_id).map(|s| !s.is_empty()).unwrap_or(true);
            if saved {
                let _ = store.end_meeting(meeting_id);
            } else {
                log::info!("recording {meeting_id} had no speech, not saved");
                let _ = store.delete_meeting(meeting_id);
            }
            sink.emit(Event::Stopped { meeting_id, saved });
            let _ = rec_slot; // keep the slot alive for the thread's lifetime only
        });
        log::info!("recording {meeting_id} started (mic: {}, system: {})", s.mic_device.as_deref().unwrap_or("default"), s.system_device.as_deref().unwrap_or("default"));
        *self.rec.lock().unwrap() = Some(Recording { meeting_id, stop, paused, handle: Some(handle), hub });
        Ok(meeting_id)
    }

    /// Pauses or resumes the running recording. Same meeting, same transcript: audio played while
    /// paused is simply not captured. Returns whether a recording was running.
    pub fn set_paused(&self, paused: bool) -> bool {
        match self.rec.lock().unwrap().as_ref() {
            Some(r) => {
                log::info!("recording {} {}", r.meeting_id, if paused { "paused" } else { "resumed" });
                r.paused.store(paused, Ordering::SeqCst);
                true
            }
            None => false,
        }
    }

    pub fn is_paused(&self) -> bool {
        self.rec.lock().unwrap().as_ref().is_some_and(|r| r.paused.load(Ordering::SeqCst))
    }

    pub fn stop_live(&self) -> Result<Option<i64>> {
        let Some(mut r) = self.rec.lock().unwrap().take() else { return Ok(None) };
        r.stop.store(true, Ordering::SeqCst);
        if let Some(h) = r.handle.take() {
            let _ = h.join();
        }
        self.stop_translation();
        // `None` when the recording had no speech and was discarded
        if self.store.get_meeting(r.meeting_id)?.is_none() {
            return Ok(None);
        }
        // Best-effort short title via the summarize model (already installed for notes).
        self.assign_ai_title(r.meeting_id);
        Ok(Some(r.meeting_id))
    }

    /// Sets a one-line AI title when the meeting still has an empty title.
    /// Failures are logged and ignored so stopping a recording never fails on titling.
    fn assign_ai_title(&self, meeting_id: i64) {
        let result = (|| -> Result<()> {
            let detail = self.store.get_meeting(meeting_id)?.context("meeting not found")?;
            if !detail.meeting.title.trim().is_empty() {
                return Ok(());
            }
            let lines = crate::export::transcript_lines(&detail);
            if lines.iter().all(|l| l.trim().is_empty()) {
                return Ok(());
            }
            let s = self.settings();
            let lang = languages::by_code(&s.summary_lang).map(|l| l.name).unwrap_or("English");
            let (model, _server) =
                self.chat_model(&s.slots.summarize, self.recommendation().summarize_item, 2048, "title")?;
            let title = summary::generate_title(model.as_ref(), &lines, lang)?;
            self.store.set_title(meeting_id, &title)?;
            log::info!("meeting {meeting_id} titled: {title}");
            Ok(())
        })();
        if let Err(e) = result {
            log::warn!("could not auto-title meeting {meeting_id}: {e:#}");
        }
    }

    // ---- translation overlay ----------------------------------------------

    pub fn translation_target(&self) -> Option<String> {
        self.rec.lock().unwrap().as_ref().and_then(|r| r.hub.target())
    }

    /// Loads the translation model (only now) and starts translating other speakers into `target`.
    pub fn start_translation(&self, target: &str) -> Result<()> {
        languages::by_code(target).with_context(|| format!("unsupported language '{target}'"))?;
        let hub = self.rec.lock().unwrap().as_ref().map(|r| r.hub.clone()).ok_or_else(|| anyhow!("Start a recording first"))?;
        let s = self.settings();
        let (model, server) = self.chat_model(&s.slots.translate, self.recommendation().translate_item, 2048, "translation")?;
        hub.start(Box::new(LlmTranslator(model)), target, 6);
        *self.mt_server.lock().unwrap() = server;
        Ok(())
    }

    pub fn stop_translation(&self) {
        if let Some(r) = self.rec.lock().unwrap().as_ref() {
            r.hub.stop();
        }
        *self.mt_server.lock().unwrap() = None; // drop → kills llama-server, frees RAM
    }

    // ---- file import -------------------------------------------------------

    /// Blocking. Progress is reported as `Event::Progress`.
    pub fn import_file(&self, path: &Path, cancel: Arc<AtomicBool>) -> Result<i64> {
        anyhow::ensure!(path.is_file(), "{} is not a file", path.display());
        let s = self.settings();
        let meeting_id = self.store.create_meeting("file")?;
        self.store.set_title(meeting_id, &recorder::title_from_path(path))?;
        let result = (|| -> Result<()> {
            let session = self.session_for(meeting_id, self.sink.clone(), &s, Arc::new(AtomicBool::new(false)))?;
            let sink = self.sink.clone();
            let src = FileSource { path: path.to_path_buf(), progress: Box::new(move |f| sink.emit(Event::Progress { meeting_id, fraction: f })) };
            recorder::run(session, vec![Box::new(src)], cancel, self.sink.clone())
        })();
        match result {
            Ok(()) if self.store.segments(meeting_id)?.is_empty() => {
                let _ = self.store.delete_meeting(meeting_id);
                log::warn!("import of {} produced no transcript", path.display());
                bail!("No speech was detected in this file, so nothing was added to the library.")
            }
            Ok(()) => {
                self.store.end_meeting(meeting_id)?;
                Ok(meeting_id)
            }
            Err(e) => {
                log::error!("import of {} failed: {e:#}", path.display());
                let _ = self.store.delete_meeting(meeting_id);
                Err(e)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::file::tests::wav_bytes;
    use crate::audio::resample::tests::sine;
    use crate::events::VecSink;
    use crate::models::FileSpec;
    use crate::pipeline::engines::fakes::ScriptedRecognizer;
    use crate::testutil::*;
    use crate::types::{Frame, StreamKind};
    use serde_json::json;
    use std::sync::mpsc::SyncSender;

    struct Tone(StreamKind);
    impl AudioSource for Tone {
        fn run(&mut self, tx: &SyncSender<Frame>, stop: &AtomicBool) -> Result<()> {
            let mut a = vec![0.0f32; 8000];
            a.extend(sine(16_000, 1.5, 200.0, 0.3));
            a.extend(vec![0.0; 16_000]);
            for c in a.chunks(1600) {
                if stop.load(Ordering::SeqCst) || tx.send(Frame { stream: self.0, samples: c.to_vec() }).is_err() {
                    break;
                }
            }
            // like a real device: keep "listening" until told to stop
            while !stop.load(Ordering::SeqCst) {
                std::thread::sleep(Duration::from_millis(10));
            }
            Ok(())
        }
    }

    fn texts(t: &'static [&'static str]) -> EngineLoader {
        Arc::new(move |_| Ok((Engine::new(Box::new(ScriptedRecognizer::texts(t)), None), Session::energy_detector_factory())))
    }

    fn make(loader: EngineLoader, manifest: Manifest) -> (Arc<Service>, Arc<VecSink>, tempfile::TempDir) {
        let t = tempfile::tempdir().unwrap();
        let sink = Arc::new(VecSink::default());
        let sources: SourceFactory = Arc::new(|_| Ok(vec![Box::new(Tone(StreamKind::System)) as Box<dyn AudioSource>]));
        let svc = Service::new(AppPaths::new(t.path().join("Parla")), manifest, sink.clone(), loader, sources).unwrap();
        (svc, sink, t)
    }

    /// Pretends the managed speech model is installed so the loader is reached.
    fn fake_install(svc: &Service, id: &str) {
        let d = models::installed_dir(&svc.paths.models_dir(), id);
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join(".installed"), b"ok").unwrap();
    }

    fn wait_for(mut f: impl FnMut() -> bool) {
        let t = std::time::Instant::now();
        while !f() {
            assert!(t.elapsed() < Duration::from_secs(10), "timed out");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn live_recording_lifecycle_saves_segments_and_ends_the_meeting() {
        let (svc, sink, _t) = make(texts(&["Buongiorno a tutti, iniziamo la riunione di oggi"]), Manifest::default());
        fake_install(&svc, ids::STT);
        assert!(svc.stop_live().unwrap().is_none());
        let id = svc.start_live().unwrap();
        assert_eq!(svc.is_recording(), Some(id));
        assert!(svc.start_live().is_err(), "no second recording");
        wait_for(|| svc.store().segments(id).unwrap().len() == 1);
        assert_eq!(svc.stop_live().unwrap(), Some(id));
        assert!(svc.is_recording().is_none());
        assert!(svc.store().get_meeting(id).unwrap().unwrap().meeting.ended_at.is_some());
        assert!(sink.take().iter().any(|e| matches!(e, Event::Stopped { meeting_id, saved: true } if *meeting_id == id)));
    }

    #[test]
    fn starting_without_the_speech_model_gives_a_clear_error_and_leaves_no_meeting() {
        let (svc, _s, _t) = make(texts(&[]), Manifest::default());
        let e = svc.start_live().unwrap_err().to_string();
        assert!(e.contains("not installed") && e.contains("Settings"), "{e}");
        assert!(svc.store().list_meetings().unwrap().is_empty());
        assert!(svc.is_recording().is_none());
    }

    #[test]
    fn translation_needs_a_recording_then_translates_remote_speech_via_an_endpoint() {
        let llm = MockServer::start(|_| Reply::json(json!({"choices":[{"message":{"content":"Good morning everyone"}}]})));
        let (svc, sink, _t) = make(texts(&["Buongiorno a tutti, iniziamo la riunione di oggi"]), Manifest::default());
        fake_install(&svc, ids::STT);
        let mut s = svc.settings();
        s.slots.translate = SlotChoice::Endpoint { url: llm.base.clone(), model: "m".into(), api_key: None };
        svc.update_settings(s).unwrap();
        assert!(svc.start_translation("en").unwrap_err().to_string().contains("recording first"));
        let id = svc.start_live().unwrap();
        svc.start_translation("en").unwrap();
        assert_eq!(svc.translation_target().as_deref(), Some("en"));
        wait_for(|| sink.0.lock().unwrap().iter().any(|e| matches!(e, Event::Translation { text, .. } if text == "Good morning everyone")));
        svc.stop_live().unwrap();
        assert_eq!(svc.store().segments(id).unwrap()[0].translation.as_deref(), Some("Good morning everyone"));
        assert!(llm.requests().iter().all(|r| r.body.contains("Translate the following text into English")));
    }

    #[test]
    fn summary_is_generated_stored_and_uses_the_chosen_language() {
        let llm = MockServer::start(|_| Reply::json(json!({"choices":[{"message":{"content":"## Summary\nTutto bene"}}]})));
        let (svc, _s, _t) = make(texts(&["Prima frase della riunione di oggi sul budget"]), Manifest::default());
        let mut s = svc.settings();
        s.slots.summarize = SlotChoice::Endpoint { url: llm.base.clone(), model: "m".into(), api_key: None };
        s.summary_lang = "it".into();
        svc.update_settings(s).unwrap();
        let m = svc.store().create_meeting("live").unwrap();
        assert!(svc.summarize(m, &|_, _| {}).is_err(), "empty meeting");
        let sp = svc.store().create_speaker(m, false).unwrap();
        svc.store().insert_segment(&crate::types::NewSegment { meeting_id: m, stream: StreamKind::System, speaker_id: Some(sp.id), t0: 0.0, t1: 2.0, lang: None, text: "We agreed to ship on Friday".into() }).unwrap();
        let out = svc.summarize(m, &|_, _| {}).unwrap();
        assert!(out.contains("Tutto bene"));
        assert_eq!(svc.store().get_meeting(m).unwrap().unwrap().meeting.summary, out);
        let body = llm.requests()[0].body.clone();
        assert!(body.contains("in Italian") && body.contains("We agreed to ship on Friday"));
    }

    #[test]
    fn stop_live_sets_an_ai_title_from_the_summarize_endpoint() {
        let llm = MockServer::start(|_| {
            Reply::json(json!({"choices":[{"message":{"content":"\"Morning standup on the budget\""}}]}))
        });
        let (svc, _s, _t) = make(texts(&["Buongiorno a tutti, iniziamo la riunione di oggi"]), Manifest::default());
        fake_install(&svc, ids::STT);
        let mut s = svc.settings();
        s.slots.summarize = SlotChoice::Endpoint { url: llm.base.clone(), model: "m".into(), api_key: None };
        svc.update_settings(s).unwrap();
        let id = svc.start_live().unwrap();
        wait_for(|| svc.store().segments(id).unwrap().len() == 1);
        assert_eq!(svc.stop_live().unwrap(), Some(id));
        let title = svc.store().get_meeting(id).unwrap().unwrap().meeting.title;
        assert_eq!(title, "Morning standup on the budget");
        assert!(llm.requests().iter().any(|r| r.body.contains("12 words")));
    }

    #[test]
    fn file_import_creates_a_titled_finished_meeting_and_reports_progress() {
        let (svc, sink, t) = make(texts(&["Buongiorno a tutti, iniziamo la riunione di oggi"]), Manifest::default());
        fake_install(&svc, ids::STT);
        let mut a = vec![0.0f32; 16_000];
        a.extend(sine(16_000, 2.0, 200.0, 0.3));
        a.extend(vec![0.0; 16_000]);
        let p = t.path().join("Weekly sync.wav");
        std::fs::write(&p, wav_bytes(16_000, 1, &a)).unwrap();
        let id = svc.import_file(&p, Arc::new(AtomicBool::new(false))).unwrap();
        let d = svc.store().get_meeting(id).unwrap().unwrap();
        assert_eq!((d.meeting.title.as_str(), d.meeting.source.as_str(), d.segments.len()), ("Weekly sync", "file", 1));
        assert!(d.meeting.ended_at.is_some());
        assert!(sink.take().iter().any(|e| matches!(e, Event::Progress { fraction, .. } if *fraction >= 1.0)));
        let bad = t.path().join("notes.mp3");
        std::fs::write(&bad, b"not audio").unwrap();
        assert!(svc.import_file(&bad, Arc::new(AtomicBool::new(false))).is_err());
        assert_eq!(svc.store().list_meetings().unwrap().len(), 1, "failed import leaves nothing behind");
    }

    #[test]
    fn settings_are_validated_persisted_and_survive_a_restart() {
        let (svc, sink, t) = make(texts(&[]), Manifest::default());
        let mut s = svc.settings();
        s.target_lang = "xx".into();
        assert!(svc.update_settings(s).is_err());
        let mut s = svc.settings();
        s.slots.summarize = SlotChoice::LocalFile { path: "/does/not/exist.gguf".into() };
        assert!(svc.update_settings(s).is_err());
        let mut s = svc.settings();
        s.target_lang = "fr".into();
        s.onboarding_done = true;
        svc.update_settings(s).unwrap();
        let again = Service::new(AppPaths::new(t.path().join("Parla")), Manifest::default(), sink, texts(&[]), Arc::new(|_| Ok(vec![]))).unwrap();
        assert_eq!((again.settings().target_lang.as_str(), again.settings().onboarding_done), ("fr", true));
    }

    #[test]
    fn a_recording_without_speech_is_not_kept() {
        let (svc, sink, _t) = make(texts(&[]), Manifest::default());
        fake_install(&svc, ids::STT);
        let id = svc.start_live().unwrap();
        std::thread::sleep(Duration::from_millis(500));
        assert_eq!(svc.stop_live().unwrap(), None);
        assert!(svc.store().get_meeting(id).unwrap().is_none());
        assert!(svc.store().list_meetings().unwrap().is_empty());
        assert!(sink.take().iter().any(|e| matches!(e, Event::Stopped { saved: false, .. })));
    }

    #[test]
    fn importing_a_file_without_speech_adds_nothing() {
        let (svc, _s, t) = make(texts(&[]), Manifest::default());
        fake_install(&svc, ids::STT);
        let p = t.path().join("silence.wav");
        std::fs::write(&p, wav_bytes(16_000, 1, &vec![0.0f32; 32_000])).unwrap();
        let e = svc.import_file(&p, Arc::new(AtomicBool::new(false))).unwrap_err().to_string();
        assert!(e.contains("No speech was detected"));
        assert!(svc.store().list_meetings().unwrap().is_empty());
    }

    #[test]
    fn pause_flag_only_applies_to_a_running_recording() {
        let (svc, _sink, _t) = make(texts(&[]), Manifest::default());
        fake_install(&svc, ids::STT);
        assert!(!svc.set_paused(true), "nothing to pause");
        assert!(!svc.is_paused());
        let id = svc.start_live().unwrap();
        assert!(svc.set_paused(true) && svc.is_paused());
        assert!(svc.set_paused(false) && !svc.is_paused());
        assert_eq!(svc.is_recording(), Some(id));
        svc.stop_live().unwrap();
        assert!(!svc.is_paused());
    }

    #[test]
    fn a_crash_leaves_no_meeting_stuck_in_recording_state() {
        let t = tempfile::tempdir().unwrap();
        let paths = AppPaths::new(t.path().join("Parla"));
        paths.ensure_dirs().unwrap();
        let m = Store::open(&paths.db_path()).unwrap().create_meeting("live").unwrap();
        let sink: Arc<dyn EventSink> = Arc::new(VecSink::default());
        let svc = Service::new(paths, Manifest::default(), sink, texts(&[]), Arc::new(|_| Ok(vec![]))).unwrap();
        assert!(svc.store().get_meeting(m).unwrap().unwrap().meeting.ended_at.is_some());
    }

    #[test]
    fn model_catalogue_install_status_and_uninstall() {
        let data = vec![7u8; 5000];
        let d2 = data.clone();
        let srv = MockServer::start(move |l| range_reply(&d2, l.range.as_deref()));
        let item = |id: &str, slot| Item { id: id.into(), slot, label: id.into(), platform: "any".into(), arch: None, license: "MIT".into(), description: String::new(), files: vec![FileSpec { name: "m.bin".into(), url: format!("{}/m", srv.base), sha256: None, size: Some(5000), extract: false }] };
        let manifest = Manifest { version: 1, items: vec![item(ids::STT, Slot::Stt), item("mt-hy-mt2-1.8b-q4km", Slot::Translate), item("other", Slot::Summarize)] };
        let (svc, _s, _t) = make(texts(&[]), manifest);
        let st = svc.model_status();
        assert_eq!(st.len(), 3);
        assert!(st.iter().all(|m| !m.installed));
        assert!(st.iter().find(|m| m.id == ids::STT).unwrap().recommended);
        assert!(!st.iter().find(|m| m.id == "other").unwrap().recommended);
        let mut last = (0, 0);
        svc.install_model(ids::STT, &Cancel::default(), &mut |a, b| last = (a, b)).unwrap();
        assert_eq!(last, (5000, 5000));
        assert!(svc.model_status().iter().find(|m| m.id == ids::STT).unwrap().installed);
        assert!(svc.install_model("nope", &Cancel::default(), &mut |_, _| {}).is_err());
        svc.uninstall_model(ids::STT).unwrap();
        assert!(!svc.model_status().iter().find(|m| m.id == ids::STT).unwrap().installed);
        svc.install_model(ids::STT, &Cancel::default(), &mut |_, _| {}).unwrap();
        svc.remove_all_models().unwrap();
        assert!(svc.model_status().iter().all(|m| !m.installed));
    }

    #[test]
    fn install_rejects_the_4b_summary_on_low_ram() {
        let t = tempfile::tempdir().unwrap();
        let sink = Arc::new(VecSink::default());
        let sources: SourceFactory = Arc::new(|_| Ok(vec![]));
        let four = Item {
            id: hardware::SUMMARIZE_4B.into(),
            slot: Slot::Summarize,
            label: "4B".into(),
            platform: "any".into(),
            arch: None,
            license: "MIT".into(),
            description: String::new(),
            files: vec![FileSpec { name: "m.bin".into(), url: "http://127.0.0.1/x".into(), sha256: None, size: Some(1), extract: false }],
        };
        let two = Item { id: hardware::SUMMARIZE_2B.into(), ..four.clone() };
        let manifest = Manifest { version: 1, items: vec![two, four] };
        let hw = hardware::HardwareInfo { total_ram_mb: 8_000, cpu_threads: 8, has_avx2: true };
        let svc = Service::with_hardware(AppPaths::new(t.path().join("Parla")), manifest, sink, texts(&[]), sources, hw).unwrap();
        let st = svc.model_status();
        assert!(!st.iter().find(|m| m.id == hardware::SUMMARIZE_4B).unwrap().eligible);
        assert!(st.iter().find(|m| m.id == hardware::SUMMARIZE_2B).unwrap().eligible);
        assert!(st.iter().find(|m| m.id == hardware::SUMMARIZE_2B).unwrap().recommended);
        assert!(!st.iter().find(|m| m.id == hardware::SUMMARIZE_4B).unwrap().recommended);
        let err = svc.install_model(hardware::SUMMARIZE_4B, &Cancel::default(), &mut |_, _| {}).unwrap_err().to_string();
        assert!(err.contains("RAM"), "{err}");
    }
}
