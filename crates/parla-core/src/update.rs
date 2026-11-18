//! Check GitHub Releases for a newer Windows installer and download it.
use anyhow::{bail, Context, Result};
use serde::Deserialize;
use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

pub const GITHUB_REPO: &str = "Jany-M/parla";
const LATEST_API: &str = "https://api.github.com/repos/Jany-M/parla/releases/latest";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateInfo {
    /// Semver without a leading `v` (e.g. `0.1.2`).
    pub version: String,
    pub tag: String,
    pub asset_name: String,
    pub download_url: String,
    pub size: u64,
}

#[derive(Debug, Deserialize)]
struct GhRelease {
    tag_name: String,
    assets: Vec<GhAsset>,
}

#[derive(Debug, Deserialize)]
struct GhAsset {
    name: String,
    browser_download_url: String,
    size: u64,
}

/// Parse `1.2.3`, `v1.2.3`, or `1.2.3-foo` → `(major, minor, patch)`.
pub fn parse_version(s: &str) -> Option<(u64, u64, u64)> {
    let s = s.trim().trim_start_matches('v');
    let core = s.split(['-', '+']).next().unwrap_or(s);
    let mut parts = core.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next().unwrap_or("0").parse().ok()?;
    let patch = parts.next().unwrap_or("0").parse().ok()?;
    Some((major, minor, patch))
}

pub fn is_newer(remote: &str, current: &str) -> bool {
    match (parse_version(remote), parse_version(current)) {
        (Some(r), Some(c)) => r > c,
        _ => false,
    }
}

fn pick_windows_installer(assets: &[GhAsset]) -> Option<&GhAsset> {
    assets.iter().find(|a| {
        let n = a.name.to_ascii_lowercase();
        n.ends_with("-setup.exe") || (n.ends_with(".exe") && n.contains("setup"))
    })
}

fn http_client(user_agent: &str) -> Result<reqwest::blocking::Client> {
    reqwest::blocking::Client::builder()
        .user_agent(user_agent)
        // Installers are ~10 MB; allow slow links without aborting mid-download.
        .timeout(std::time::Duration::from_secs(60 * 10))
        .connect_timeout(std::time::Duration::from_secs(30))
        .build()
        .context("building HTTP client")
}

/// Returns `Ok(Some(..))` when GitHub's latest release is newer than `current_version`.
pub fn check_latest(current_version: &str) -> Result<Option<UpdateInfo>> {
    let ua = format!("Parla/{current_version} (+https://github.com/{GITHUB_REPO})");
    let client = http_client(&ua)?;
    let release: GhRelease = client
        .get(LATEST_API)
        .header("Accept", "application/vnd.github+json")
        .send()
        .context("contacting GitHub Releases")?
        .error_for_status()
        .context("GitHub Releases returned an error")?
        .json()
        .context("reading GitHub Releases JSON")?;

    if !is_newer(&release.tag_name, current_version) {
        return Ok(None);
    }
    let Some(asset) = pick_windows_installer(&release.assets) else {
        bail!("latest release {} has no Windows installer asset", release.tag_name);
    };
    let version = release.tag_name.trim().trim_start_matches('v').to_string();
    Ok(Some(UpdateInfo {
        version,
        tag: release.tag_name,
        asset_name: asset.name.clone(),
        download_url: asset.browser_download_url.clone(),
        size: asset.size,
    }))
}

/// Download `info` to `dest_dir`/`info.asset_name`. `progress(done, total)`.
pub fn download_installer(info: &UpdateInfo, dest_dir: &Path, progress: &mut dyn FnMut(u64, u64)) -> Result<PathBuf> {
    std::fs::create_dir_all(dest_dir).context("creating update download folder")?;
    let dest = dest_dir.join(&info.asset_name);
    let part = dest.with_extension("exe.part");

    let ua = format!("Parla/{} (+https://github.com/{GITHUB_REPO})", info.version);
    let client = http_client(&ua)?;
    let mut resp = client
        .get(&info.download_url)
        .header("Accept", "application/octet-stream")
        .send()
        .context("downloading installer")?
        .error_for_status()
        .context("installer download failed")?;

    let total = resp.content_length().unwrap_or(info.size).max(1);
    let mut file = File::create(&part).with_context(|| format!("creating {}", part.display()))?;
    let mut done = 0u64;
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = resp.read(&mut buf).context("reading installer bytes")?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n])?;
        done = done.saturating_add(n as u64);
        progress(done, total);
    }
    file.flush()?;
    drop(file);
    std::fs::rename(&part, &dest).with_context(|| format!("finalizing {}", dest.display()))?;
    progress(total, total);
    Ok(dest)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_compare_orders_semver_tuples() {
        assert!(is_newer("0.1.1", "0.1.0"));
        assert!(is_newer("v0.2.0", "0.1.9"));
        assert!(!is_newer("0.1.0", "0.1.0"));
        assert!(!is_newer("0.1.0", "0.1.1"));
        assert!(!is_newer("bad", "0.1.0"));
    }

    #[test]
    fn parse_strips_v_and_pre_release() {
        assert_eq!(parse_version("v1.2.3-beta"), Some((1, 2, 3)));
        assert_eq!(parse_version("0.1"), Some((0, 1, 0)));
    }

    #[test]
    fn picks_nsis_setup_asset() {
        let assets = vec![
            GhAsset {
                name: "checksums.txt".into(),
                browser_download_url: "https://example/checksums.txt".into(),
                size: 10,
            },
            GhAsset {
                name: "Parla_0.1.3_universal.dmg".into(),
                browser_download_url: "https://example/parla.dmg".into(),
                size: 8_000_000,
            },
            GhAsset {
                name: "Parla_0.1.2_x64-setup.exe".into(),
                browser_download_url: "https://example/setup.exe".into(),
                size: 9_000_000,
            },
        ];
        let a = pick_windows_installer(&assets).unwrap();
        assert_eq!(a.name, "Parla_0.1.2_x64-setup.exe");
    }
}
