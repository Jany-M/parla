use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Slot {
    Stt,
    Vad,
    Speaker,
    Translate,
    Summarize,
    /// llama.cpp server binaries.
    Runtime,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FileSpec {
    pub name: String,
    pub url: String,
    /// Lower-case hex. Items without a pinned hash are still size-checked when `size` is set.
    #[serde(default)]
    pub sha256: Option<String>,
    #[serde(default)]
    pub size: Option<u64>,
    /// `.zip` files: unpack into the item folder after verification, then delete the archive.
    #[serde(default)]
    pub extract: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Item {
    pub id: String,
    pub slot: Slot,
    pub label: String,
    /// "windows", "macos" or "any".
    #[serde(default = "any")]
    pub platform: String,
    #[serde(default)]
    pub license: String,
    #[serde(default)]
    pub description: String,
    pub files: Vec<FileSpec>,
}

fn any() -> String {
    "any".into()
}

impl Item {
    pub fn total_bytes(&self) -> u64 {
        self.files.iter().filter_map(|f| f.size).sum()
    }
    pub fn matches_platform(&self, platform: &str) -> bool {
        self.platform == "any" || self.platform == platform
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Manifest {
    pub version: u32,
    pub items: Vec<Item>,
}

/// "windows" / "macos" / "linux" — the keys used in `Item::platform`.
pub fn current_platform() -> &'static str {
    if cfg!(target_os = "windows") {
        "windows"
    } else if cfg!(target_os = "macos") {
        "macos"
    } else {
        "linux"
    }
}

impl Manifest {
    pub fn parse(json: &str) -> Result<Self> {
        let m: Manifest = serde_json::from_str(json).context("invalid model manifest")?;
        let mut seen = std::collections::HashSet::new();
        for i in &m.items {
            anyhow::ensure!(seen.insert(&i.id), "duplicate manifest id '{}'", i.id);
            anyhow::ensure!(!i.files.is_empty(), "manifest item '{}' has no files", i.id);
            anyhow::ensure!(
                !i.id.is_empty() && i.id.chars().all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c)),
                "bad manifest id '{}'",
                i.id
            );
            for f in &i.files {
                anyhow::ensure!(
                    !f.name.is_empty() && !f.name.contains(['/', '\\']) && f.name != "..",
                    "bad file name '{}' in '{}'",
                    f.name,
                    i.id
                );
                if let Some(h) = &f.sha256 {
                    anyhow::ensure!(h.len() == 64 && h.chars().all(|c| c.is_ascii_hexdigit()), "bad sha256 for {}", f.name);
                }
            }
        }
        Ok(m)
    }

    /// The catalogue shipped inside the app (`resources/manifest.json`, built by `scripts/gen_manifest.py`).
    pub fn bundled() -> Self {
        Self::parse(include_str!("../../../../resources/manifest.json")).expect("bundled manifest.json is valid")
    }

    pub fn get(&self, id: &str) -> Option<&Item> {
        self.items.iter().find(|i| i.id == id)
    }

    pub fn for_slot(&self, slot: Slot, platform: &str) -> Vec<&Item> {
        self.items.iter().filter(|i| i.slot == slot && i.matches_platform(platform)).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOOD: &str = r#"{"version":1,"items":[
      {"id":"stt-a","slot":"stt","label":"A","files":[{"name":"m.onnx","url":"http://x/m","sha256":"0000000000000000000000000000000000000000000000000000000000000000","size":10}]},
      {"id":"rt-win","slot":"runtime","label":"R","platform":"windows","files":[{"name":"r.zip","url":"http://x/r","extract":true}]},
      {"id":"rt-mac","slot":"runtime","label":"R","platform":"macos","files":[{"name":"r.zip","url":"http://x/r2","extract":true}]}]}"#;

    #[test]
    fn parses_and_filters_by_platform() {
        let m = Manifest::parse(GOOD).unwrap();
        assert_eq!(m.items.len(), 3);
        assert_eq!(m.get("stt-a").unwrap().platform, "any");
        assert_eq!(m.get("stt-a").unwrap().total_bytes(), 10);
        let rt: Vec<_> = m.for_slot(Slot::Runtime, "windows").iter().map(|i| i.id.as_str()).collect();
        assert_eq!(rt, ["rt-win"]);
        assert_eq!(m.for_slot(Slot::Stt, "macos").len(), 1, "'any' matches every platform");
    }

    #[test]
    fn bundled_manifest_is_complete_pinned_and_matches_the_ids_the_app_uses() {
        let m = Manifest::bundled();
        for id in [crate::service::ids::STT, crate::service::ids::VAD, crate::service::ids::SPEAKER, "rt-llama-cpp-windows"] {
            assert!(m.get(id).is_some(), "missing {id}");
        }
        // every hardware tier must point at an item that exists
        for ram in [8_000, 16_000, 32_000] {
            let r = crate::hardware::recommend(&crate::hardware::HardwareInfo { total_ram_mb: ram, cpu_threads: 8, has_avx2: true });
            assert!(m.get(r.summarize_item).is_some() && m.get(r.translate_item).is_some(), "tier for {ram} MB");
        }
        for i in &m.items {
            for f in &i.files {
                assert!(f.url.starts_with("https://"), "{}: plain http", f.name);
                assert!(f.sha256.is_some() && f.size.is_some(), "{} must be pinned (sha256 + size)", f.name);
            }
        }
        assert!(m.for_slot(Slot::Runtime, "windows")[0].files[0].extract, "runtime zip must be unpacked");
        assert!(m.items.iter().map(|i| i.total_bytes()).sum::<u64>() < 8_000_000_000);
    }

    #[test]
    fn rejects_bad_manifests() {
        for bad in [
            r#"{"version":1,"items":[{"id":"a","slot":"stt","label":"","files":[]}]}"#,
            r#"{"version":1,"items":[{"id":"../a","slot":"stt","label":"","files":[{"name":"x","url":"u"}]}]}"#,
            r#"{"version":1,"items":[{"id":"a","slot":"stt","label":"","files":[{"name":"../x","url":"u"}]}]}"#,
            r#"{"version":1,"items":[{"id":"a","slot":"stt","label":"","files":[{"name":"x","url":"u","sha256":"zz"}]}]}"#,
            r#"{"version":1,"items":[{"id":"a","slot":"stt","label":"","files":[{"name":"x","url":"u"}]},{"id":"a","slot":"stt","label":"","files":[{"name":"x","url":"u"}]}]}"#,
            "not json",
        ] {
            assert!(Manifest::parse(bad).is_err(), "{bad}");
        }
    }
}
