use crate::types::*;
use std::collections::HashMap;

/// Markdown export. `date_label` is formatted by the caller in the user's local time zone.
pub fn to_markdown(d: &MeetingDetail, date_label: &str) -> String {
    let names: HashMap<i64, &str> = d.speakers.iter().map(|s| (s.id, s.display())).collect();
    let title = if d.meeting.title.trim().is_empty() { "Meeting" } else { d.meeting.title.trim() };
    let mut out = format!("# {title}\n\n*{date_label}*\n");
    if !d.meeting.summary.trim().is_empty() {
        // The model already returns "## Summary" / "## Decisions" / "## Action items".
        out.push('\n');
        out.push_str(d.meeting.summary.trim());
        out.push('\n');
    }
    if !d.meeting.notes.trim().is_empty() {
        out.push_str(&format!("\n## Notes\n\n{}\n", d.meeting.notes.trim()));
    }
    out.push_str("\n## Transcript\n\n");
    for s in &d.segments {
        let who = s.speaker_id.and_then(|id| names.get(&id).copied()).unwrap_or("Unknown");
        out.push_str(&format!("**{who}** ({}): {}\n", format_clock(s.t0), s.text.trim()));
        if let Some(t) = s.translation.as_deref().filter(|t| !t.trim().is_empty()) {
            out.push_str(&format!("> {}\n", t.trim().replace('\n', " ")));
        }
        out.push('\n');
    }
    out
}

/// Plain transcript lines "Name: text" — the single input format for summarisation.
pub fn transcript_lines(d: &MeetingDetail) -> Vec<String> {
    let names: HashMap<i64, &str> = d.speakers.iter().map(|s| (s.id, s.display())).collect();
    d.segments
        .iter()
        .map(|s| {
            let who = s.speaker_id.and_then(|id| names.get(&id).copied()).unwrap_or("Unknown");
            format!("{who}: {}", s.text.trim())
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn detail() -> MeetingDetail {
        let sp = |id, label: &str, name: Option<&str>, me| Speaker { id, meeting_id: 1, label: label.into(), name: name.map(Into::into), is_me: me };
        let seg = |id, speaker_id, t0, text: &str, tr: Option<&str>| Segment {
            id, meeting_id: 1, stream: StreamKind::System, speaker_id, t0, t1: t0 + 1.0, lang: None, text: text.into(),
            translation: tr.map(Into::into), translation_lang: tr.map(|_| "it".into()),
        };
        MeetingDetail {
            meeting: MeetingInfo { id: 1, title: "Sync".into(), started_at: 0, ended_at: None, source: "live".into(), notes: "remember x".into(), summary: "".into() },
            speakers: vec![sp(1, "Me", None, true), sp(2, "Speaker 1", Some("Anna"), false)],
            segments: vec![seg(1, Some(1), 0.0, " Hi ", None), seg(2, Some(2), 65.0, "Hello there", Some("Ciao\na tutti")), seg(3, None, 70.0, "??", None)],
        }
    }

    #[test]
    fn markdown_has_names_clock_translation_and_skips_empty_sections() {
        let md = to_markdown(&detail(), "16 Nov 2026");
        assert!(md.starts_with("# Sync\n\n*16 Nov 2026*"));
        assert!(md.contains("**Me** (00:00): Hi"));
        assert!(md.contains("**Anna** (01:05): Hello there\n> Ciao a tutti"));
        assert!(md.contains("**Unknown** (01:10): ??"));
        assert!(md.contains("## Notes\n\nremember x"));
        assert!(!md.contains("## Summary"));
    }

    #[test]
    fn markdown_does_not_duplicate_summary_heading() {
        let mut d = detail();
        d.meeting.summary = "## Summary\nWe talked.\n## Decisions\nNone\n## Action items\nNone".into();
        let md = to_markdown(&d, "16 Nov 2026");
        assert_eq!(md.matches("## Summary").count(), 1, "{md}");
        assert!(md.contains("## Decisions\nNone"));
    }

    #[test]
    fn transcript_lines_use_display_names() {
        assert_eq!(transcript_lines(&detail()), ["Me: Hi", "Anna: Hello there", "Unknown: ??"]);
    }
}
