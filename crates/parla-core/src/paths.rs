use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

/// Everything the app writes lives under ONE root so uninstalling is "delete this folder".
/// (The webview profile is also redirected into `webview/` by the shell.)
#[derive(Clone, Debug)]
pub struct AppPaths {
    pub root: PathBuf,
}

pub const APP_DIR_NAME: &str = "Parla";

impl AppPaths {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// `%LOCALAPPDATA%\Parla` on Windows, `~/Library/Application Support/Parla` on macOS.
    pub fn default_root() -> PathBuf {
        dirs::data_local_dir()
            .unwrap_or_else(std::env::temp_dir)
            .join(APP_DIR_NAME)
    }

    pub fn models_dir(&self) -> PathBuf { self.root.join("models") }
    pub fn db_path(&self) -> PathBuf { self.root.join("parla.db") }
    pub fn settings_path(&self) -> PathBuf { self.root.join("settings.json") }
    pub fn logs_dir(&self) -> PathBuf { self.root.join("logs") }
    pub fn audio_dir(&self) -> PathBuf { self.root.join("audio") }
    pub fn webview_dir(&self) -> PathBuf { self.root.join("webview") }

    pub fn ensure_dirs(&self) -> Result<()> {
        for d in [self.models_dir(), self.logs_dir(), self.audio_dir(), self.webview_dir()] {
            std::fs::create_dir_all(&d).with_context(|| format!("creating {}", d.display()))?;
        }
        Ok(())
    }

    /// Removes every downloaded model and runtime (keeps meetings and settings).
    pub fn wipe_models(&self) -> Result<()> {
        remove_dir_if_exists(&self.models_dir())?;
        std::fs::create_dir_all(self.models_dir())?;
        Ok(())
    }

    /// Total bytes used under the root (for the settings page).
    pub fn disk_usage(&self) -> u64 {
        dir_size(&self.root)
    }
}

pub fn remove_dir_if_exists(p: &Path) -> Result<()> {
    match std::fs::remove_dir_all(p) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e).with_context(|| format!("removing {}", p.display())),
    }
}

pub fn dir_size(p: &Path) -> u64 {
    let Ok(rd) = std::fs::read_dir(p) else { return 0 };
    rd.filter_map(|e| e.ok())
        .map(|e| match e.metadata() {
            Ok(m) if m.is_dir() => dir_size(&e.path()),
            Ok(m) => m.len(),
            Err(_) => 0,
        })
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn everything_lives_under_root() {
        let t = tempfile::tempdir().unwrap();
        let p = AppPaths::new(t.path().join("Parla"));
        p.ensure_dirs().unwrap();
        for d in [p.models_dir(), p.logs_dir(), p.audio_dir(), p.webview_dir(), p.db_path(), p.settings_path()] {
            assert!(d.starts_with(&p.root), "{} escapes root", d.display());
        }
    }

    #[test]
    fn wipe_models_keeps_other_data() {
        let t = tempfile::tempdir().unwrap();
        let p = AppPaths::new(t.path().join("Parla"));
        p.ensure_dirs().unwrap();
        std::fs::write(p.models_dir().join("big.bin"), vec![0u8; 1000]).unwrap();
        std::fs::write(p.db_path(), b"db").unwrap();
        assert!(p.disk_usage() >= 1002);
        p.wipe_models().unwrap();
        assert!(p.models_dir().exists());
        assert_eq!(std::fs::read_dir(p.models_dir()).unwrap().count(), 0);
        assert!(p.db_path().exists());
    }

    #[test]
    fn remove_missing_dir_is_ok() {
        let t = tempfile::tempdir().unwrap();
        remove_dir_if_exists(&t.path().join("nope")).unwrap();
    }
}
