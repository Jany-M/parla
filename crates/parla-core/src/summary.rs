use crate::llm::{prompts, ChatModel, ChatOptions};
use anyhow::{anyhow, bail, Result};

#[derive(Clone, Debug)]
pub struct SummaryConfig {
    /// Largest transcript slice sent in one request (≈ 3 chars/token, leaves room for the answer).
    pub max_chars_per_chunk: usize,
    /// Safety bound on merge depth for absurdly long meetings.
    pub max_levels: usize,
}

impl Default for SummaryConfig {
    fn default() -> Self {
        Self { max_chars_per_chunk: 9_000, max_levels: 4 }
    }
}

const TITLE_EXCERPT_CHARS: usize = 2_000;
const TITLE_MAX_CHARS: usize = 80;

/// Pack transcript lines into a short excerpt for the title prompt.
pub fn excerpt_lines(lines: &[String], max_chars: usize) -> String {
    let max = max_chars.max(200);
    let mut out = String::new();
    for line in lines {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if !out.is_empty() {
            if out.len() + line.len() + 1 > max {
                break;
            }
            out.push('\n');
        } else if line.len() > max {
            let mut end = max;
            while end > 0 && !line.is_char_boundary(end) {
                end -= 1;
            }
            return line[..end].trim().to_string();
        }
        out.push_str(line);
    }
    out
}

/// Clean a model title into a single short library label.
pub fn sanitize_title(raw: &str) -> Option<String> {
    let mut t = raw.lines().map(str::trim).find(|l| !l.is_empty())?.to_string();
    while let Some(rest) = t.strip_prefix('#') {
        t = rest.trim().to_string();
    }
    for wrap in [('"', '"'), ('\'', '\''), ('“', '”'), ('«', '»')] {
        if t.starts_with(wrap.0) && t.ends_with(wrap.1) && t.len() >= 2 {
            t = t[wrap.0.len_utf8()..t.len() - wrap.1.len_utf8()].trim().to_string();
        }
    }
    let collapsed: String = t.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.chars().count() < 2 {
        return None;
    }
    let mut out = collapsed;
    if out.chars().count() > TITLE_MAX_CHARS {
        let mut cut = TITLE_MAX_CHARS;
        while cut > 0 && !out.is_char_boundary(cut) {
            cut -= 1;
        }
        let head = &out[..cut];
        out = match head.rfind(' ') {
            Some(i) if i > 20 => head[..i].trim().to_string(),
            _ => head.trim().to_string(),
        };
        if !out.ends_with('…') {
            out.push('…');
        }
    }
    if out.chars().count() < 2 {
        None
    } else {
        Some(out)
    }
}

/// One short title from the transcript, using the same chat model as summaries.
pub fn generate_title(model: &dyn ChatModel, lines: &[String], language_name: &str) -> Result<String> {
    if lines.iter().all(|l| l.trim().is_empty()) {
        bail!("there is nothing to title yet");
    }
    let excerpt = excerpt_lines(lines, TITLE_EXCERPT_CHARS);
    let raw = model.chat(&prompts::title_messages(language_name, &excerpt), &ChatOptions::title())?;
    sanitize_title(&raw).ok_or_else(|| anyhow!("the model returned an empty title"))
}

/// Map-reduce summary: one call if it fits, otherwise partial notes per slice, merged
/// (recursively if the notes themselves are still too long), then the final structured notes.
/// `progress(done, total)` is called after each request.
pub fn summarize(model: &dyn ChatModel, lines: &[String], language_name: &str, cfg: &SummaryConfig, progress: &dyn Fn(usize, usize)) -> Result<String> {
    if lines.iter().all(|l| l.trim().is_empty()) {
        bail!("there is nothing to summarize yet");
    }
    let opts = ChatOptions::summary();
    let mut material: Vec<String> = lines.to_vec();
    let mut done = 0;
    for level in 0..cfg.max_levels {
        let total_len: usize = material.iter().map(|l| l.len() + 1).sum();
        if total_len <= cfg.max_chars_per_chunk {
            break;
        }
        let chunks = prompts::chunk_lines(&material, cfg.max_chars_per_chunk);
        log::info!("summary level {level}: {} chunks", chunks.len());
        let mut partials = Vec::with_capacity(chunks.len());
        for (i, c) in chunks.iter().enumerate() {
            partials.push(model.chat(&prompts::partial_summary_messages(language_name, i + 1, chunks.len(), c), &opts)?);
            done += 1;
            progress(done, done + 1);
        }
        material = partials;
    }
    let text = model.chat(&prompts::final_summary_messages(language_name, &material.join("\n\n")), &opts)?;
    progress(done + 1, done + 1);
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::fakes::FakeChat;
    use std::sync::Mutex;

    fn lines(n: usize) -> Vec<String> {
        (0..n).map(|i| format!("Speaker {}: this is sentence {i} of the discussion", i % 2)).collect()
    }

    #[test]
    fn short_transcript_is_one_request() {
        let m = FakeChat::new(|_| Ok("NOTES".into()));
        let out = summarize(&m, &lines(5), "English", &SummaryConfig::default(), &|_, _| {}).unwrap();
        assert_eq!(out, "NOTES");
        let calls = m.calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert!(calls[0][1].content.contains("sentence 4"));
    }

    #[test]
    fn long_transcript_is_mapped_then_merged_and_reports_progress() {
        let m = FakeChat::new(|p| Ok(if p.contains("part ") { "- partial".into() } else { "FINAL".into() }));
        let seen = Mutex::new(vec![]);
        let cfg = SummaryConfig { max_chars_per_chunk: 600, max_levels: 4 };
        let out = summarize(&m, &lines(60), "Italian", &cfg, &|d, t| seen.lock().unwrap().push((d, t))).unwrap();
        assert_eq!(out, "FINAL");
        let calls = m.calls.lock().unwrap();
        let partials = calls.iter().filter(|c| c[1].content.contains(" of ")).count();
        assert!(partials >= 3, "{partials}");
        assert!(calls.last().unwrap()[1].content.contains("## Action items"));
        assert!(calls.last().unwrap()[1].content.contains("in Italian"));
        assert!(!seen.lock().unwrap().is_empty());
    }

    #[test]
    fn partial_notes_that_are_still_too_long_are_reduced_again() {
        // every partial answer is big, so the merged notes need another level
        let m = FakeChat::new(|p| Ok(if p.contains("part ") { "x".repeat(100) } else { "FINAL".into() }));
        let cfg = SummaryConfig { max_chars_per_chunk: 700, max_levels: 4 };
        summarize(&m, &lines(80), "English", &cfg, &|_, _| {}).unwrap();
        let calls = m.calls.lock().unwrap();
        let final_len = calls.last().unwrap()[1].content.len();
        assert!(final_len < 1500, "final request must be small, was {final_len}");
    }

    #[test]
    fn level_cap_prevents_runaway_loops() {
        let m = FakeChat::new(|_| Ok("y".repeat(1000)));
        let cfg = SummaryConfig { max_chars_per_chunk: 400, max_levels: 2 };
        summarize(&m, &lines(50), "English", &cfg, &|_, _| {}).unwrap();
        assert!(m.calls.lock().unwrap().len() < 200);
    }

    #[test]
    fn empty_transcript_is_an_error_and_model_errors_propagate() {
        let m = FakeChat::new(|_| Ok("x".into()));
        assert!(summarize(&m, &[], "English", &SummaryConfig::default(), &|_, _| {}).is_err());
        assert!(summarize(&m, &["  ".into()], "English", &SummaryConfig::default(), &|_, _| {}).is_err());
        let bad = FakeChat::new(|_| anyhow::bail!("down"));
        assert!(summarize(&bad, &lines(3), "English", &SummaryConfig::default(), &|_, _| {}).is_err());
    }

    #[test]
    fn sanitize_title_strips_wrappers_and_caps_length() {
        assert_eq!(sanitize_title("  \"Budget review for Q3\"  ").as_deref(), Some("Budget review for Q3"));
        assert_eq!(sanitize_title("## Weekly sync\nmore").as_deref(), Some("Weekly sync"));
        let long = "word ".repeat(40);
        let got = sanitize_title(&long).unwrap();
        assert!(got.chars().count() <= TITLE_MAX_CHARS + 1, "{got}");
        assert!(got.ends_with('…'));
        assert!(sanitize_title(" \n ").is_none());
    }

    #[test]
    fn generate_title_uses_a_short_prompt_and_returns_clean_text() {
        let m = FakeChat::new(|_| Ok("\"Planning the product launch\"\n".into()));
        let title = generate_title(&m, &lines(5), "English").unwrap();
        assert_eq!(title, "Planning the product launch");
        let prompt = &m.calls.lock().unwrap()[0][1].content;
        assert!(prompt.contains("12 words") && prompt.contains("sentence 0"));
    }
}
