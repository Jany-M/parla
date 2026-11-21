use crate::slots::SlotConfig;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub slots: SlotConfig,
    /// Language the overlay translates into.
    pub target_lang: String,
    pub mic_device: Option<String>,
    /// Output device whose audio is captured (None = system default).
    pub system_device: Option<String>,
    pub echo_guard: bool,
    pub onboarding_done: bool,
    pub overlay_font_px: u32,
    pub overlay_opacity: f32,
    /// Language of generated summaries.
    pub summary_lang: String,
    /// App chrome language (en / it / fr). Defaults from the OS when supported.
    pub ui_lang: String,
}

impl Default for Settings {
    fn default() -> Self {
        let content = crate::languages::system_language_code().to_string();
        Self {
            slots: SlotConfig::default(),
            target_lang: content.clone(),
            mic_device: None,
            system_device: None,
            echo_guard: true,
            onboarding_done: false,
            overlay_font_px: 22,
            overlay_opacity: 0.85,
            summary_lang: content,
            ui_lang: crate::languages::system_ui_language_code().to_string(),
        }
    }
}

impl Settings {
    /// Missing or corrupt file → defaults (never block startup on a bad settings file).
    pub fn load(path: &Path) -> Self {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    /// Atomic write: temp file + rename.
    pub fn save(&self, path: &Path) -> Result<()> {
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(self)?).with_context(|| format!("writing {}", tmp.display()))?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_and_defaults() {
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("s.json");
        assert_eq!(Settings::load(&p), Settings::default());
        let mut s = Settings::default();
        s.target_lang = "it".into();
        s.mic_device = Some("USB Mic".into());
        s.save(&p).unwrap();
        assert_eq!(Settings::load(&p), s);
        assert!(!p.with_extension("json.tmp").exists());
    }

    #[test]
    fn corrupt_file_falls_back_to_defaults() {
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("s.json");
        std::fs::write(&p, "{ not json").unwrap();
        assert_eq!(Settings::load(&p), Settings::default());
    }

    #[test]
    fn unknown_and_missing_fields_are_tolerated() {
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("s.json");
        std::fs::write(&p, r#"{"target_lang":"fr","future_field":1}"#).unwrap();
        let s = Settings::load(&p);
        assert_eq!(s.target_lang, "fr");
        assert!(s.echo_guard);
        assert!(["en", "it", "fr"].contains(&s.ui_lang.as_str()));
    }
}
