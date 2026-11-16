//! Each model "slot" is Managed (downloaded by the app), a user-supplied local file, or — for
//! the two language-model slots — an OpenAI-compatible endpoint (LM Studio, Ollama, llama.cpp).
//! Everything user-supplied is validated before it is accepted.
use crate::llm::{ChatModel, ChatOptions, Message};
use anyhow::{bail, ensure, Result};
use serde::{Deserialize, Serialize};
use std::io::Read;
use std::path::Path;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SlotChoice {
    /// Use the app's own download. `item_id: None` = the one recommended for this machine.
    Managed { #[serde(default)] item_id: Option<String> },
    /// STT: a folder with encoder/decoder/joiner/tokens; speaker: an .onnx; LLM slots: a .gguf.
    LocalFile { path: String },
    Endpoint { url: String, model: String, #[serde(default)] api_key: Option<String> },
}

impl Default for SlotChoice {
    fn default() -> Self {
        SlotChoice::Managed { item_id: None }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SlotConfig {
    #[serde(default)]
    pub stt: SlotChoice,
    #[serde(default)]
    pub speaker: SlotChoice,
    #[serde(default)]
    pub translate: SlotChoice,
    #[serde(default)]
    pub summarize: SlotChoice,
}

/// True for loopback hosts: the only endpoints that keep audio/text on this machine.
pub fn is_local_url(url: &str) -> bool {
    let rest = url.split("://").nth(1).unwrap_or(url);
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    let authority = authority.rsplit('@').next().unwrap_or(authority);
    let host = if let Some(h) = authority.strip_prefix('[') {
        h.split(']').next().unwrap_or("")
    } else {
        authority.split(':').next().unwrap_or("")
    };
    host.eq_ignore_ascii_case("localhost") || host == "::1" || host == "127.0.0.1" || host.starts_with("127.")
}

pub fn validate_endpoint_url(url: &str) -> Result<()> {
    ensure!(url.starts_with("http://") || url.starts_with("https://"), "the address must start with http:// or https://");
    ensure!(url.split("://").nth(1).map(|r| !r.is_empty()).unwrap_or(false), "the address has no host");
    Ok(())
}

/// A `.gguf` model starts with the magic bytes "GGUF".
pub fn validate_gguf(path: &Path) -> Result<()> {
    ensure!(path.is_file(), "{} is not a file", path.display());
    let mut magic = [0u8; 4];
    std::fs::File::open(path)?.read_exact(&mut magic).ok();
    ensure!(&magic == b"GGUF", "{} is not a GGUF model file", path.display());
    Ok(())
}

pub fn validate_onnx(path: &Path) -> Result<()> {
    ensure!(path.is_file(), "{} is not a file", path.display());
    ensure!(path.extension().and_then(|e| e.to_str()).map(|e| e.eq_ignore_ascii_case("onnx")).unwrap_or(false), "expected an .onnx file");
    Ok(())
}

/// Files of an offline transducer recogniser (e.g. Parakeet TDT). Returns (encoder, decoder, joiner, tokens).
pub fn find_transducer_files(dir: &Path) -> Result<(String, String, String, String)> {
    ensure!(dir.is_dir(), "{} is not a folder", dir.display());
    let mut names: Vec<String> = std::fs::read_dir(dir)?.filter_map(|e| e.ok()).map(|e| e.file_name().to_string_lossy().into_owned()).collect();
    names.sort();
    // prefer int8 variants (smaller, what the app downloads)
    let pick = |prefix: &str| -> Result<String> {
        let c: Vec<&String> = names.iter().filter(|n| n.starts_with(prefix) && n.ends_with(".onnx")).collect();
        let best = c.iter().find(|n| n.contains("int8")).or_else(|| c.first());
        match best {
            Some(n) => Ok(dir.join(n).to_string_lossy().into_owned()),
            None => bail!("the folder has no {prefix}*.onnx file. Pick a Parakeet/transducer model folder"),
        }
    };
    let tokens = dir.join("tokens.txt");
    ensure!(tokens.is_file(), "the folder has no tokens.txt");
    Ok((pick("encoder")?, pick("decoder")?, pick("joiner")?, tokens.to_string_lossy().into_owned()))
}

/// One tiny request proving the endpoint really answers like a chat model.
pub fn smoke_test(model: &dyn ChatModel) -> Result<()> {
    let opts = ChatOptions { max_tokens: 16, ..ChatOptions::translation() };
    let out = model.chat(&[Message::user("Reply with the single word: OK")], &opts)?;
    ensure!(!out.trim().is_empty(), "the model answered with an empty reply");
    Ok(())
}

/// Validates what the user typed for a slot without starting anything heavy.
/// `llm_slot`: translate/summarize accept endpoints and .gguf; stt/speaker accept folders/.onnx.
pub fn validate_choice(choice: &SlotChoice, slot: crate::models::Slot) -> Result<()> {
    use crate::models::Slot::*;
    match (choice, slot) {
        (SlotChoice::Managed { .. }, _) => Ok(()),
        (SlotChoice::LocalFile { path }, Stt) => find_transducer_files(Path::new(path)).map(|_| ()),
        (SlotChoice::LocalFile { path }, Speaker) => validate_onnx(Path::new(path)),
        (SlotChoice::LocalFile { path }, Translate | Summarize) => validate_gguf(Path::new(path)),
        (SlotChoice::Endpoint { url, model, .. }, Translate | Summarize) => {
            validate_endpoint_url(url)?;
            ensure!(!model.trim().is_empty(), "enter the model name");
            Ok(())
        }
        (SlotChoice::Endpoint { .. }, _) => bail!("only the translation and summary models can use an endpoint"),
        (SlotChoice::LocalFile { .. }, _) => bail!("this slot cannot use a local file"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::fakes::FakeChat;
    use crate::models::Slot;

    #[test]
    fn loopback_detection_handles_ports_paths_credentials_and_ipv6() {
        for u in ["http://localhost:1234/v1", "http://127.0.0.1:11434", "https://LOCALHOST", "http://[::1]:8080/x", "http://127.0.0.5/"] {
            assert!(is_local_url(u), "{u}");
        }
        for u in ["http://192.168.1.5:1234", "https://api.example.com/v1", "http://localhost.evil.com", "http://127.0.0.1@evil.com/", "http://10.0.0.1"] {
            assert!(!is_local_url(u), "{u}");
        }
    }

    #[test]
    fn endpoint_urls_need_a_scheme_and_host() {
        assert!(validate_endpoint_url("http://localhost:1234").is_ok());
        assert!(validate_endpoint_url("localhost:1234").is_err());
        assert!(validate_endpoint_url("http://").is_err());
    }

    #[test]
    fn gguf_and_onnx_validation() {
        let t = tempfile::tempdir().unwrap();
        let good = t.path().join("m.gguf");
        std::fs::write(&good, b"GGUF\x03\x00").unwrap();
        let bad = t.path().join("x.gguf");
        std::fs::write(&bad, b"nope").unwrap();
        assert!(validate_gguf(&good).is_ok());
        assert!(validate_gguf(&bad).is_err());
        assert!(validate_gguf(&t.path().join("missing")).is_err());
        let onnx = t.path().join("s.onnx");
        std::fs::write(&onnx, b"x").unwrap();
        assert!(validate_onnx(&onnx).is_ok());
        assert!(validate_onnx(&good).is_err());
    }

    #[test]
    fn transducer_folder_prefers_int8_and_requires_every_part() {
        let t = tempfile::tempdir().unwrap();
        for f in ["encoder.int8.onnx", "encoder.onnx", "decoder.int8.onnx", "joiner.int8.onnx", "tokens.txt"] {
            std::fs::write(t.path().join(f), b"x").unwrap();
        }
        let (e, d, j, tok) = find_transducer_files(t.path()).unwrap();
        assert!(e.ends_with("encoder.int8.onnx") && d.ends_with("decoder.int8.onnx") && j.ends_with("joiner.int8.onnx") && tok.ends_with("tokens.txt"));
        std::fs::remove_file(t.path().join("joiner.int8.onnx")).unwrap();
        assert!(find_transducer_files(t.path()).unwrap_err().to_string().contains("joiner"));
        std::fs::remove_file(t.path().join("tokens.txt")).unwrap();
        assert!(find_transducer_files(t.path()).is_err());
    }

    #[test]
    fn choices_are_checked_against_the_slot_they_are_for() {
        let ep = SlotChoice::Endpoint { url: "http://localhost:1234".into(), model: "qwen".into(), api_key: None };
        assert!(validate_choice(&ep, Slot::Summarize).is_ok());
        assert!(validate_choice(&ep, Slot::Stt).is_err());
        let no_model = SlotChoice::Endpoint { url: "http://localhost:1234".into(), model: " ".into(), api_key: None };
        assert!(validate_choice(&no_model, Slot::Translate).is_err());
        assert!(validate_choice(&SlotChoice::default(), Slot::Stt).is_ok());
        assert!(validate_choice(&SlotChoice::LocalFile { path: "/nope".into() }, Slot::Speaker).is_err());
    }

    #[test]
    fn smoke_test_accepts_a_reply_and_rejects_empty_or_errors() {
        assert!(smoke_test(&FakeChat::new(|_| Ok("OK".into()))).is_ok());
        assert!(smoke_test(&FakeChat::new(|_| Ok("  ".into()))).is_err());
        assert!(smoke_test(&FakeChat::new(|_| anyhow::bail!("refused"))).is_err());
    }

    #[test]
    fn config_roundtrips_and_old_settings_files_load() {
        let c = SlotConfig { translate: SlotChoice::Endpoint { url: "http://x".into(), model: "m".into(), api_key: Some("k".into()) }, ..Default::default() };
        let back: SlotConfig = serde_json::from_str(&serde_json::to_string(&c).unwrap()).unwrap();
        assert_eq!(back, c);
        let empty: SlotConfig = serde_json::from_str("{}").unwrap();
        assert_eq!(empty, SlotConfig::default());
    }
}
