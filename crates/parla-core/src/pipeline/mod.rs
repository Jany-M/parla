//! VAD segmenter → ASR → speaker assignment → store/event sink.
pub mod detector;
pub mod echo;
pub mod engines;
pub mod session;
pub mod speakers;

pub use detector::{EnergyConfig, EnergyDetector, SpeechChunk, SpeechDetector};
pub use engines::{Recognizer, SpeakerEmbedder};
pub use session::{DetectorFactory, Engine, Session, SessionConfig};
