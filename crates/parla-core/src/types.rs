use serde::{Deserialize, Serialize};

/// Every engine consumes 16 kHz mono f32.
pub const SAMPLE_RATE: u32 = 16_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StreamKind {
    /// The user's microphone — always "Me".
    Mic,
    /// Everything the computer plays (the remote participants).
    System,
    /// An imported audio/video file.
    File,
}

impl StreamKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            StreamKind::Mic => "mic",
            StreamKind::System => "system",
            StreamKind::File => "file",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "mic" => Some(StreamKind::Mic),
            "system" => Some(StreamKind::System),
            "file" => Some(StreamKind::File),
            _ => None,
        }
    }
}

/// A block of 16 kHz mono audio from one stream.
#[derive(Clone, Debug)]
pub struct Frame {
    pub stream: StreamKind,
    pub samples: Vec<f32>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Speaker {
    pub id: i64,
    pub meeting_id: i64,
    pub label: String,
    pub name: Option<String>,
    pub is_me: bool,
}

impl Speaker {
    pub fn display(&self) -> &str {
        self.name
            .as_deref()
            .filter(|n| !n.trim().is_empty())
            .unwrap_or(&self.label)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Segment {
    pub id: i64,
    pub meeting_id: i64,
    pub stream: StreamKind,
    pub speaker_id: Option<i64>,
    pub t0: f64,
    pub t1: f64,
    pub lang: Option<String>,
    pub text: String,
    pub translation: Option<String>,
    pub translation_lang: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct NewSegment {
    pub meeting_id: i64,
    pub stream: StreamKind,
    pub speaker_id: Option<i64>,
    pub t0: f64,
    pub t1: f64,
    pub lang: Option<String>,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MeetingInfo {
    pub id: i64,
    pub title: String,
    pub started_at: i64,
    pub ended_at: Option<i64>,
    pub source: String,
    pub notes: String,
    pub summary: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MeetingListItem {
    pub id: i64,
    pub title: String,
    pub started_at: i64,
    pub ended_at: Option<i64>,
    pub source: String,
    pub segment_count: i64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MeetingDetail {
    pub meeting: MeetingInfo,
    pub speakers: Vec<Speaker>,
    pub segments: Vec<Segment>,
}

/// Seconds → "mm:ss" (or "h:mm:ss" past an hour).
pub fn format_clock(secs: f64) -> String {
    let s = secs.max(0.0) as u64;
    let (h, m, s) = (s / 3600, (s % 3600) / 60, s % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m:02}:{s:02}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clock_formats() {
        assert_eq!(format_clock(0.0), "00:00");
        assert_eq!(format_clock(65.9), "01:05");
        assert_eq!(format_clock(3725.0), "1:02:05");
        assert_eq!(format_clock(-3.0), "00:00");
    }

    #[test]
    fn stream_roundtrip() {
        for k in [StreamKind::Mic, StreamKind::System, StreamKind::File] {
            assert_eq!(StreamKind::parse(k.as_str()), Some(k));
        }
        assert_eq!(StreamKind::parse("nope"), None);
    }

    #[test]
    fn speaker_display_prefers_name() {
        let mut s = Speaker { id: 1, meeting_id: 1, label: "Speaker 1".into(), name: None, is_me: false };
        assert_eq!(s.display(), "Speaker 1");
        s.name = Some("  ".into());
        assert_eq!(s.display(), "Speaker 1");
        s.name = Some("Anna".into());
        assert_eq!(s.display(), "Anna");
    }
}
