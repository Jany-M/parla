//! One OpenAI-compatible chat interface serves both translation and summarisation.
//! The bundled llama.cpp server is just the default endpoint; LM Studio/Ollama are the same code.
pub mod client;
pub mod prompts;
pub mod sidecar;

pub use client::LlmClient;
pub use sidecar::{kill_stale, LlamaConfig, LlamaServer};

use anyhow::Result;

#[derive(Clone, Debug, PartialEq)]
pub struct Message {
    pub role: &'static str,
    pub content: String,
}

impl Message {
    pub fn system(c: impl Into<String>) -> Self {
        Self { role: "system", content: c.into() }
    }
    pub fn user(c: impl Into<String>) -> Self {
        Self { role: "user", content: c.into() }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ChatOptions {
    pub temperature: f32,
    pub top_p: f32,
    pub top_k: i32,
    pub repeat_penalty: f32,
    pub max_tokens: u32,
}

impl ChatOptions {
    /// Sampling recommended on the Hy-MT2 model card for the 1.8B/7B models.
    pub fn translation() -> Self {
        Self { temperature: 0.7, top_p: 0.6, top_k: 20, repeat_penalty: 1.05, max_tokens: 512 }
    }
    pub fn summary() -> Self {
        Self { temperature: 0.3, top_p: 0.9, top_k: 40, repeat_penalty: 1.05, max_tokens: 1500 }
    }
    /// One short meeting title after a recording stops.
    pub fn title() -> Self {
        Self { temperature: 0.2, top_p: 0.9, top_k: 40, repeat_penalty: 1.05, max_tokens: 48 }
    }
}

pub trait ChatModel: Send + Sync {
    fn chat(&self, messages: &[Message], opts: &ChatOptions) -> Result<String>;
}

/// Removes `<think>…</think>` reasoning blocks some models emit (also an unterminated one).
pub fn strip_think(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(i) = rest.find("<think>") {
        out.push_str(&rest[..i]);
        match rest[i..].find("</think>") {
            Some(j) => rest = &rest[i + j + "</think>".len()..],
            None => {
                rest = "";
                break;
            }
        }
    }
    out.push_str(rest);
    out.trim().to_string()
}

#[cfg(test)]
pub(crate) mod fakes {
    use super::*;
    use std::sync::Mutex;

    /// Records every prompt; answers with `f(prompt)`.
    pub struct FakeChat<F: Fn(&str) -> Result<String> + Send + Sync> {
        pub f: F,
        pub calls: Mutex<Vec<Vec<Message>>>,
    }
    impl<F: Fn(&str) -> Result<String> + Send + Sync> FakeChat<F> {
        pub fn new(f: F) -> Self {
            Self { f, calls: Mutex::new(vec![]) }
        }
    }
    impl<F: Fn(&str) -> Result<String> + Send + Sync> ChatModel for FakeChat<F> {
        fn chat(&self, m: &[Message], _: &ChatOptions) -> Result<String> {
            self.calls.lock().unwrap().push(m.to_vec());
            (self.f)(&m.last().unwrap().content)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn think_blocks_are_removed() {
        assert_eq!(strip_think("<think>hmm</think>\nCiao"), "Ciao");
        assert_eq!(strip_think("a<think>x</think>b<think>y</think>c"), "abc");
        assert_eq!(strip_think("Ciao <think>never closed"), "Ciao");
        assert_eq!(strip_think("  plain  "), "plain");
    }
}
