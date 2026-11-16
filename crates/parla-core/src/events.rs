use crate::types::{Segment, StreamKind};
use serde::Serialize;
use std::sync::Mutex;

/// Everything the UI needs to hear about while a session runs.
#[derive(Clone, Debug, Serialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    Segment { segment: Segment },
    Translation { segment_id: i64, text: String, lang: String },
    Level { stream: StreamKind, level: f32 },
    /// Fraction 0..1 while a file import is being processed.
    Progress { meeting_id: i64, fraction: f32 },
    /// Non-fatal problem worth showing (device lost, translation failed...).
    Warning { message: String },
    /// A live recording ended (the meeting is saved).
    /// `saved` is false when nothing was transcribed: the empty meeting was discarded.
    Stopped { meeting_id: i64, saved: bool },
}

pub trait EventSink: Send + Sync {
    fn emit(&self, e: Event);
}

/// Collects events — used in tests.
#[derive(Default)]
pub struct VecSink(pub Mutex<Vec<Event>>);

impl VecSink {
    pub fn take(&self) -> Vec<Event> {
        std::mem::take(&mut self.0.lock().unwrap())
    }
    pub fn segments(&self) -> Vec<Segment> {
        self.0
            .lock()
            .unwrap()
            .iter()
            .filter_map(|e| match e {
                Event::Segment { segment } => Some(segment.clone()),
                _ => None,
            })
            .collect()
    }
}

impl EventSink for VecSink {
    fn emit(&self, e: Event) {
        self.0.lock().unwrap().push(e);
    }
}

pub struct NullSink;
impl EventSink for NullSink {
    fn emit(&self, _: Event) {}
}
