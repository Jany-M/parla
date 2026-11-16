use super::Message;

/// Hy-MT2's documented English template (its model card: "no default system prompt").
pub fn translation_messages(target_language_name: &str, text: &str) -> Vec<Message> {
    vec![Message::user(format!(
        "Translate the following text into {target_language_name}. Note that you should only output the translated result without any additional explanation:\n\n{text}"
    ))]
}

const SYSTEM: &str = "You write accurate, concise meeting notes. Use only information present in the transcript. \
Never invent names, numbers, dates or decisions. Keep speaker names exactly as written.";

/// Final notes from a transcript (or from merged partial notes).
pub fn final_summary_messages(language_name: &str, material: &str) -> Vec<Message> {
    vec![
        Message::system(SYSTEM),
        Message::user(format!(
            "{material}\n\n---\nWrite the meeting notes in {language_name} using exactly these sections:\n\
## Summary\n3 to 6 sentences.\n## Decisions\nBullet list, or \"None\".\n## Action items\nBullet list as \"Owner: task (due date if stated)\", or \"None\"."
        )),
    ]
}

/// Notes for one slice of a long meeting; merged later.
pub fn partial_summary_messages(language_name: &str, part: usize, of: usize, slice: &str) -> Vec<Message> {
    vec![
        Message::system(SYSTEM),
        Message::user(format!(
            "This is part {part} of {of} of a meeting transcript (lines are \"Speaker: text\").\n\n{slice}\n\n---\n\
Write compact bullet-point notes in {language_name}: topics, decisions, action items with owners, and any numbers or dates mentioned."
        )),
    ]
}

/// Packs lines into chunks of at most `max_chars`, splitting only a single over-long line.
pub fn chunk_lines(lines: &[String], max_chars: usize) -> Vec<String> {
    let max = max_chars.max(200);
    let mut chunks = vec![];
    let mut cur = String::new();
    let push_piece = |piece: &str, cur: &mut String, chunks: &mut Vec<String>| {
        if !cur.is_empty() && cur.len() + piece.len() + 1 > max {
            chunks.push(std::mem::take(cur));
        }
        if !cur.is_empty() {
            cur.push('\n');
        }
        cur.push_str(piece);
    };
    for line in lines {
        if line.len() <= max {
            push_piece(line, &mut cur, &mut chunks);
        } else {
            // split on char boundaries
            let mut start = 0;
            while start < line.len() {
                let mut end = (start + max).min(line.len());
                while !line.is_char_boundary(end) {
                    end -= 1;
                }
                push_piece(&line[start..end], &mut cur, &mut chunks);
                start = end;
            }
        }
    }
    if !cur.is_empty() {
        chunks.push(cur);
    }
    chunks
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn translation_prompt_matches_the_model_card_template() {
        let m = translation_messages("Italian", "Hello world");
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].role, "user");
        assert!(m[0].content.starts_with("Translate the following text into Italian."));
        assert!(m[0].content.ends_with("\n\nHello world"));
    }

    #[test]
    fn summary_prompts_name_the_language_and_sections() {
        let m = final_summary_messages("French", "A: hi");
        assert_eq!(m[0].role, "system");
        assert!(m[1].content.contains("in French") && m[1].content.contains("## Action items") && m[1].content.contains("A: hi"));
    }

    #[test]
    fn chunking_never_exceeds_the_limit_and_keeps_every_line_in_order() {
        let lines: Vec<String> = (0..50).map(|i| format!("Speaker {}: sentence number {i} with some words", i % 3)).collect();
        let chunks = chunk_lines(&lines, 300);
        assert!(chunks.len() > 1);
        assert!(chunks.iter().all(|c| c.len() <= 300));
        assert_eq!(chunks.join("\n"), lines.join("\n"));
    }

    #[test]
    fn one_giant_line_is_split_on_char_boundaries() {
        let line = "è".repeat(1000); // 2 bytes per char
        let chunks = chunk_lines(&[line.clone()], 301);
        assert!(chunks.len() >= 6);
        assert!(chunks.iter().all(|c| c.len() <= 301));
        assert_eq!(chunks.concat().replace('\n', ""), line);
    }

    #[test]
    fn empty_input_gives_no_chunks() {
        assert!(chunk_lines(&[], 500).is_empty());
    }
}
