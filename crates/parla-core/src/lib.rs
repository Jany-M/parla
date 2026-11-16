//! parla-core — all product logic, no UI and no OS-specific code outside `audio::device`.
//!
//! Design rules (see PLAN.md §2):
//! * one `Segment` type flows from the pipeline to the store, UI, translation, summary and export;
//! * one `AudioSource` seam hides the OS (device capture, file import);
//! * one OpenAI-compatible `ChatModel` client serves translation and summary;
//! * one manifest drives every download, the wizard, settings and custom-model validation.

pub mod audio;
pub mod events;
pub mod export;
pub mod hardware;
pub mod languages;
pub mod llm;
pub mod logging;
pub mod models;
pub mod paths;
pub mod pipeline;
pub mod recorder;
pub mod service;
pub mod settings;
pub mod slots;
pub mod store;
pub mod summary;
pub mod translate;
pub mod types;
pub mod update;

#[cfg(test)]
pub(crate) mod testutil;

#[cfg(feature = "sherpa")]
pub mod sherpa;

pub use types::*;
