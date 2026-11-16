use super::{strip_think, ChatModel, ChatOptions, Message};
use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use std::time::Duration;

/// Blocking client for any OpenAI-compatible `/v1/chat/completions` endpoint.
#[derive(Clone)]
pub struct LlmClient {
    base: String,
    model: String,
    api_key: Option<String>,
    http: reqwest::blocking::Client,
}

impl LlmClient {
    /// `base` like `http://127.0.0.1:8080` (a trailing `/v1` is tolerated).
    pub fn new(base: &str, model: &str, api_key: Option<String>, timeout: Duration) -> Result<Self> {
        let base = base.trim_end_matches('/').trim_end_matches("/v1").to_string();
        let http = reqwest::blocking::Client::builder().timeout(timeout).build()?;
        Ok(Self { base, model: model.to_string(), api_key, http })
    }

    pub fn base_url(&self) -> &str {
        &self.base
    }

    /// llama-server answers 503 while the model loads; LM Studio/Ollama expose `/v1/models`.
    pub fn is_ready(&self) -> bool {
        for path in ["/health", "/v1/models"] {
            if let Ok(r) = self.http.get(format!("{}{path}", self.base)).send() {
                if r.status().is_success() {
                    return true;
                }
            }
        }
        false
    }

    fn request(&self, messages: &[Message], opts: &ChatOptions) -> Value {
        json!({
            "model": self.model,
            "messages": messages.iter().map(|m| json!({"role": m.role, "content": m.content})).collect::<Vec<_>>(),
            "temperature": opts.temperature,
            "top_p": opts.top_p,
            "top_k": opts.top_k,
            "repeat_penalty": opts.repeat_penalty,
            "max_tokens": opts.max_tokens,
            "stream": false,
            // Qwen3.x "thinking" is off: we want the answer, not the reasoning, and low latency.
            "chat_template_kwargs": { "enable_thinking": false },
        })
    }
}

impl ChatModel for LlmClient {
    fn chat(&self, messages: &[Message], opts: &ChatOptions) -> Result<String> {
        let mut req = self.http.post(format!("{}/v1/chat/completions", self.base)).json(&self.request(messages, opts));
        if let Some(k) = &self.api_key {
            req = req.bearer_auth(k);
        }
        let resp = req.send().context("could not reach the language model")?;
        let status = resp.status();
        let body = resp.text().unwrap_or_default();
        if !status.is_success() {
            bail!("language model returned {status}: {}", body.chars().take(300).collect::<String>());
        }
        let v: Value = serde_json::from_str(&body).context("language model sent invalid JSON")?;
        let text = v["choices"][0]["message"]["content"].as_str().context("language model reply had no text")?;
        Ok(strip_think(text))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::*;

    fn client(base: &str) -> LlmClient {
        LlmClient::new(base, "m", Some("secret".into()), Duration::from_secs(5)).unwrap()
    }

    #[test]
    fn sends_openai_request_and_parses_reply() {
        let srv = MockServer::start(|_| Reply::json(json!({"choices":[{"message":{"content":"<think>x</think> Ciao mondo "}}]})));
        let out = client(&format!("{}/v1/", srv.base))
            .chat(&[Message::system("s"), Message::user("hello")], &ChatOptions::translation())
            .unwrap();
        assert_eq!(out, "Ciao mondo");
        let r = &srv.requests()[0];
        assert_eq!((r.method.as_str(), r.url.as_str()), ("POST", "/v1/chat/completions"));
        let b: Value = serde_json::from_str(&r.body).unwrap();
        assert_eq!(b["messages"][1]["content"], "hello");
        assert_eq!(b["messages"][0]["role"], "system");
        assert_eq!(b["stream"], false);
        assert_eq!(b["top_k"], 20);
        assert_eq!(b["chat_template_kwargs"]["enable_thinking"], false);
    }

    #[test]
    fn http_errors_carry_the_server_message() {
        let srv = MockServer::start(|_| Reply::status(500, "model exploded"));
        let e = client(&srv.base).chat(&[Message::user("x")], &ChatOptions::summary()).unwrap_err().to_string();
        assert!(e.contains("500") && e.contains("model exploded"), "{e}");
    }

    #[test]
    fn malformed_replies_are_errors_not_panics() {
        let srv = MockServer::start(|_| Reply::ok("not json"));
        assert!(client(&srv.base).chat(&[Message::user("x")], &ChatOptions::summary()).is_err());
        let srv = MockServer::start(|_| Reply::json(json!({"choices":[]})));
        assert!(client(&srv.base).chat(&[Message::user("x")], &ChatOptions::summary()).is_err());
    }

    #[test]
    fn unreachable_server_is_a_clean_error() {
        let c = client("http://127.0.0.1:1");
        assert!(c.chat(&[Message::user("x")], &ChatOptions::summary()).is_err());
        assert!(!c.is_ready());
    }

    #[test]
    fn readiness_checks_health_then_models() {
        let srv = MockServer::start(|l| if l.url == "/health" { Reply::status(503, "loading") } else { Reply::ok("{}") });
        assert!(client(&srv.base).is_ready(), "falls back to /v1/models");
        let srv = MockServer::start(|_| Reply::status(503, ""));
        assert!(!client(&srv.base).is_ready());
    }
}
