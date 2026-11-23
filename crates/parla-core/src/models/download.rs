use super::manifest::{FileSpec, Item};
use anyhow::{bail, Context, Result};
use sha2::{Digest, Sha256};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

/// Shared cancel flag.
#[derive(Clone, Default)]
pub struct Cancel(pub Arc<AtomicBool>);
impl Cancel {
    pub fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

const OK_MARKER: &str = ".installed";

pub fn installed_dir(models_dir: &Path, id: &str) -> PathBuf {
    models_dir.join(id)
}

pub fn is_installed(models_dir: &Path, id: &str) -> bool {
    installed_dir(models_dir, id).join(OK_MARKER).exists()
}

pub fn uninstall_item(models_dir: &Path, id: &str) -> Result<()> {
    crate::paths::remove_dir_if_exists(&installed_dir(models_dir, id))
}

/// Drops installed language-model runtimes that do not match this Mac's CPU (e.g. leftover Intel
/// build after an Apple Silicon reinstall, or a Rosetta-era download). Returns how many were removed.
pub fn prune_incompatible_runtimes(models_dir: &Path, manifest: &super::Manifest, platform: &str, arch: &str) -> usize {
    let mut n = 0;
    for item in manifest.items.iter().filter(|i| i.slot == super::Slot::Runtime && i.matches_platform(platform)) {
        if item.matches_arch(arch) || !is_installed(models_dir, &item.id) {
            continue;
        }
        log::info!("removing incompatible runtime {} (this machine needs {arch})", item.id);
        if uninstall_item(models_dir, &item.id).is_ok() {
            n += 1;
        }
    }
    n
}

/// Detects a broken llama.cpp macOS extract: versioned `.dylib` symlinks flattened to empty files.
/// Older Parla builds wrote those stubs; dyld then aborts with "Library not loaded".
pub fn ensure_runtime_dylibs(llama_server: &Path) -> Result<()> {
    let Some(dir) = llama_server.parent() else {
        return Ok(());
    };
    let stub = dir.join("libllama-common.1.dylib");
    if !stub.exists() {
        return Ok(());
    }
    #[cfg(unix)]
    {
        let meta = fs::symlink_metadata(&stub).with_context(|| format!("reading {}", stub.display()))?;
        if meta.file_type().is_symlink() {
            return Ok(());
        }
        if meta.len() == 0 {
            bail!(
                "The language-model runtime is incomplete (broken library links). \
                 In Settings → Models, remove the language-model runtime and download it again."
            );
        }
    }
    Ok(())
}

fn sha256_file(p: &Path) -> Result<String> {
    let mut f = File::open(p)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(hex::encode(h.finalize()))
}

fn verify(p: &Path, spec: &FileSpec) -> Result<()> {
    if let Some(size) = spec.size {
        let got = fs::metadata(p)?.len();
        if got != size {
            bail!("{} has the wrong size ({got} bytes, expected {size})", spec.name);
        }
    }
    if let Some(want) = &spec.sha256 {
        let got = sha256_file(p)?;
        if !got.eq_ignore_ascii_case(want) {
            bail!("{} failed its integrity check (sha256 mismatch)", spec.name);
        }
    }
    Ok(())
}

/// Downloads one file to `dest` with resume (`dest.part`), progress and cancellation.
/// `progress(bytes_done_for_this_file)`.
fn fetch(http: &reqwest::blocking::Client, spec: &FileSpec, dest: &Path, cancel: &Cancel, progress: &mut dyn FnMut(u64)) -> Result<()> {
    if dest.exists() && verify(dest, spec).is_ok() {
        progress(fs::metadata(dest)?.len());
        return Ok(());
    }
    let part = dest.with_extension(format!("{}.part", dest.extension().and_then(|e| e.to_str()).unwrap_or("")));
    let mut have = fs::metadata(&part).map(|m| m.len()).unwrap_or(0);
    for _ in 0..2 {
        let mut req = http.get(&spec.url);
        if have > 0 {
            req = req.header("Range", format!("bytes={have}-"));
        }
        let mut resp = req.send().with_context(|| format!("downloading {}", spec.name))?;
        let status = resp.status();
        if status.as_u16() == 416 {
            // we already have everything (or the part is garbage): verify, else start over
            if verify_part(&part, spec).is_ok() {
                break;
            }
            let _ = fs::remove_file(&part);
            have = 0;
            continue;
        }
        if !status.is_success() {
            bail!("download of {} failed: HTTP {status}", spec.name);
        }
        let resumed = status.as_u16() == 206;
        if !resumed {
            have = 0; // server ignored Range → restart from zero
        }
        let mut out = if resumed { OpenOptions::new().append(true).open(&part)? } else { File::create(&part)? };
        let mut done = have;
        let mut buf = vec![0u8; 256 * 1024];
        loop {
            if cancel.is_cancelled() {
                bail!("cancelled");
            }
            let n = resp.read(&mut buf).with_context(|| format!("connection lost while downloading {}", spec.name))?;
            if n == 0 {
                break;
            }
            out.write_all(&buf[..n])?;
            done += n as u64;
            progress(done);
        }
        out.flush()?;
        drop(out);
        break;
    }
    if let Err(e) = verify_part(&part, spec) {
        let _ = fs::remove_file(&part); // corrupt: never resume from it
        return Err(e);
    }
    let _ = fs::remove_file(dest);
    fs::rename(&part, dest).with_context(|| format!("saving {}", spec.name))?;
    Ok(())
}

fn verify_part(part: &Path, spec: &FileSpec) -> Result<()> {
    anyhow::ensure!(part.exists(), "nothing downloaded for {}", spec.name);
    verify(part, spec)
}

/// Unpacks a zip into `into`, rejecting any entry that would escape it (zip-slip).
fn extract_zip(zip_path: &Path, into: &Path) -> Result<()> {
    let mut z = zip::ZipArchive::new(File::open(zip_path)?).context("not a valid zip archive")?;
    for i in 0..z.len() {
        let mut f = z.by_index(i)?;
        let rel = f.enclosed_name().with_context(|| format!("unsafe path in archive: {}", f.name()))?;
        let out = into.join(rel);
        if f.is_dir() {
            fs::create_dir_all(&out)?;
        } else {
            if let Some(p) = out.parent() {
                fs::create_dir_all(p)?;
            }
            std::io::copy(&mut f, &mut File::create(&out)?)?;
        }
    }
    Ok(())
}

fn safe_archive_rel(name: &str) -> Result<PathBuf> {
    let p = PathBuf::from(name);
    anyhow::ensure!(!p.is_absolute() && p.components().all(|c| matches!(c, std::path::Component::Normal(_))), "unsafe path in archive: {name}");
    Ok(p)
}

fn safe_symlink_target(target: &str) -> Result<&Path> {
    let p = Path::new(target);
    // Relative only, no `..` - versioned dylib links in llama.cpp releases are same-dir names.
    anyhow::ensure!(
        !p.is_absolute() && p.components().all(|c| matches!(c, std::path::Component::Normal(_))),
        "unsafe symlink target in archive: {target}"
    );
    Ok(p)
}

/// Unpacks a `.tar.gz` into `into`, rejecting path escape (tar-slip).
/// Symlinks are preserved on Unix (llama.cpp macOS runtimes ship versioned `.dylib` links).
fn extract_tar_gz(archive: &Path, into: &Path) -> Result<()> {
    let file = File::open(archive).with_context(|| format!("opening {}", archive.display()))?;
    let dec = flate2::read::GzDecoder::new(file);
    let mut ar = tar::Archive::new(dec);
    for entry in ar.entries().context("not a valid tar.gz archive")? {
        let mut entry = entry?;
        let name = entry.path().context("bad tar entry path")?.into_owned();
        let name = name.to_string_lossy();
        let rel = safe_archive_rel(&name)?;
        let out = into.join(&rel);
        let kind = entry.header().entry_type();
        if kind.is_dir() {
            fs::create_dir_all(&out)?;
            continue;
        }
        if let Some(p) = out.parent() {
            fs::create_dir_all(p)?;
        }
        if kind.is_symlink() {
            let target = entry
                .link_name()?
                .context("symlink without target")?
                .into_owned();
            let target = target.to_string_lossy();
            let target = safe_symlink_target(&target)?;
            let _ = fs::remove_file(&out);
            #[cfg(unix)]
            {
                std::os::unix::fs::symlink(target, &out)
                    .with_context(|| format!("creating symlink {}", out.display()))?;
            }
            #[cfg(not(unix))]
            {
                // macOS runtimes are never installed on Windows; materialize when the target exists.
                let src = out.parent().unwrap_or(into).join(target);
                if src.is_file() {
                    fs::copy(&src, &out)?;
                }
            }
            continue;
        }
        let mut dest = File::create(&out)?;
        std::io::copy(&mut entry, &mut dest)?;
        #[cfg(unix)]
        if let Ok(mode) = entry.header().mode() {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(&out, fs::Permissions::from_mode(mode));
        }
    }
    Ok(())
}

#[cfg(unix)]
fn ensure_llama_executable(dir: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    for walk in walkdir_files(dir) {
        let p = walk?;
        if p.file_name().and_then(|n| n.to_str()) == Some("llama-server") {
            let mut perms = fs::metadata(&p)?.permissions();
            perms.set_mode(perms.mode() | 0o755);
            fs::set_permissions(&p, perms)?;
        }
    }
    #[cfg(target_os = "macos")]
    {
        // Downloaded unsigned binaries can carry Gatekeeper quarantine and abort on spawn.
        let _ = std::process::Command::new("xattr")
            .args(["-dr", "com.apple.quarantine"])
            .arg(dir)
            .status();
    }
    Ok(())
}

#[cfg(unix)]
fn walkdir_files(dir: &Path) -> Vec<Result<PathBuf>> {
    fn rec(dir: &Path, out: &mut Vec<Result<PathBuf>>) {
        let entries = match fs::read_dir(dir) {
            Ok(e) => e,
            Err(e) => {
                out.push(Err(e.into()));
                return;
            }
        };
        for e in entries {
            match e {
                Ok(e) => {
                    let p = e.path();
                    if p.is_dir() {
                        rec(&p, out);
                    } else {
                        out.push(Ok(p));
                    }
                }
                Err(err) => out.push(Err(err.into())),
            }
        }
    }
    let mut out = Vec::new();
    rec(dir, &mut out);
    out
}

fn extract_archive(path: &Path, into: &Path) -> Result<()> {
    let name = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
    if name.ends_with(".tar.gz") || name.ends_with(".tgz") {
        extract_tar_gz(path, into)?;
    } else {
        extract_zip(path, into)?;
    }
    #[cfg(unix)]
    ensure_llama_executable(into)?;
    Ok(())
}

/// Installs every file of `item` into `models_dir/<id>/`. Resumable; idempotent when already
/// installed. `progress(done_bytes, total_bytes)` where total is 0 if sizes are unknown.
pub fn install_item(models_dir: &Path, item: &Item, cancel: &Cancel, progress: &mut dyn FnMut(u64, u64)) -> Result<PathBuf> {
    let dir = installed_dir(models_dir, &item.id);
    if is_installed(models_dir, &item.id) {
        return Ok(dir);
    }
    fs::create_dir_all(&dir)?;
    let http = reqwest::blocking::Client::builder().connect_timeout(Duration::from_secs(20)).timeout(Duration::from_secs(60 * 60 * 6)).build()?;
    let total = item.total_bytes();
    let mut base = 0u64;
    for spec in &item.files {
        let dest = dir.join(&spec.name);
        fetch(&http, spec, &dest, cancel, &mut |d| progress(base + d, total))?;
        base += fs::metadata(&dest)?.len();
        if spec.extract {
            extract_archive(&dest, &dir)?;
            fs::remove_file(&dest)?;
        }
    }
    fs::write(dir.join(OK_MARKER), b"ok")?;
    Ok(dir)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::manifest::Slot;
    use crate::testutil::*;

    fn sha(data: &[u8]) -> String {
        hex::encode(Sha256::digest(data))
    }

    fn item(id: &str, files: Vec<FileSpec>) -> Item {
        Item { id: id.into(), slot: Slot::Stt, label: id.into(), platform: "any".into(), arch: None, license: String::new(), description: String::new(), files }
    }

    fn spec(base: &str, name: &str, data: &[u8]) -> FileSpec {
        FileSpec { name: name.into(), url: format!("{base}/{name}"), sha256: Some(sha(data)), size: Some(data.len() as u64), extract: false }
    }

    fn data(n: usize) -> Vec<u8> {
        (0..n).map(|i| (i * 31 % 251) as u8).collect()
    }

    fn serve(files: Vec<(&'static str, Vec<u8>)>) -> MockServer {
        MockServer::start(move |l| match files.iter().find(|(n, _)| l.url == format!("/{n}")) {
            Some((_, d)) => range_reply(d, l.range.as_deref()),
            None => Reply::status(404, ""),
        })
    }

    #[test]
    fn downloads_verifies_marks_installed_and_is_idempotent() {
        let d = data(700_000);
        let srv = serve(vec![("model.bin", d.clone())]);
        let t = tempfile::tempdir().unwrap();
        let it = item("stt-a", vec![spec(&srv.base, "model.bin", &d)]);
        let mut last = (0, 0);
        let dir = install_item(t.path(), &it, &Cancel::default(), &mut |a, b| last = (a, b)).unwrap();
        assert_eq!(fs::read(dir.join("model.bin")).unwrap(), d);
        assert_eq!(last, (700_000, 700_000));
        assert!(is_installed(t.path(), "stt-a"));
        let n = srv.requests().len();
        install_item(t.path(), &it, &Cancel::default(), &mut |_, _| {}).unwrap();
        assert_eq!(srv.requests().len(), n, "second install must not hit the network");
    }

    #[test]
    fn resumes_a_partial_download_with_a_range_request() {
        let d = data(500_000);
        let srv = serve(vec![("m.bin", d.clone())]);
        let t = tempfile::tempdir().unwrap();
        let dir = t.path().join("stt-a");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("m.bin.part"), &d[..200_000]).unwrap();
        install_item(t.path(), &item("stt-a", vec![spec(&srv.base, "m.bin", &d)]), &Cancel::default(), &mut |_, _| {}).unwrap();
        assert_eq!(fs::read(dir.join("m.bin")).unwrap(), d);
        assert_eq!(srv.requests()[0].range.as_deref(), Some("bytes=200000-"));
    }

    #[test]
    fn server_that_ignores_range_restarts_cleanly() {
        let d = data(100_000);
        let d2 = d.clone();
        let srv = MockServer::start(move |_| Reply::ok(d2.clone())); // always 200, full body
        let t = tempfile::tempdir().unwrap();
        let dir = t.path().join("a");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("m.bin.part"), &d[..30_000]).unwrap();
        install_item(t.path(), &item("a", vec![spec(&srv.base, "m.bin", &d)]), &Cancel::default(), &mut |_, _| {}).unwrap();
        assert_eq!(fs::read(dir.join("m.bin")).unwrap(), d, "no duplicated prefix");
    }

    #[test]
    fn complete_part_file_answered_with_416_is_accepted() {
        let d = data(50_000);
        let srv = serve(vec![("m.bin", d.clone())]);
        let t = tempfile::tempdir().unwrap();
        let dir = t.path().join("a");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("m.bin.part"), &d).unwrap();
        install_item(t.path(), &item("a", vec![spec(&srv.base, "m.bin", &d)]), &Cancel::default(), &mut |_, _| {}).unwrap();
        assert_eq!(fs::read(dir.join("m.bin")).unwrap(), d);
    }

    #[test]
    fn hash_mismatch_fails_deletes_the_bad_file_and_is_not_marked_installed() {
        let d = data(10_000);
        let srv = serve(vec![("m.bin", d.clone())]);
        let t = tempfile::tempdir().unwrap();
        let mut s = spec(&srv.base, "m.bin", &d);
        s.sha256 = Some(sha(b"something else"));
        let e = install_item(t.path(), &item("a", vec![s]), &Cancel::default(), &mut |_, _| {}).unwrap_err().to_string();
        assert!(e.contains("integrity"), "{e}");
        assert!(!is_installed(t.path(), "a"));
        assert!(!t.path().join("a/m.bin").exists() && !t.path().join("a/m.bin.part").exists());
    }

    #[test]
    fn wrong_size_and_http_errors_are_reported() {
        let d = data(1_000);
        let srv = serve(vec![("m.bin", d.clone())]);
        let t = tempfile::tempdir().unwrap();
        let mut s = spec(&srv.base, "m.bin", &d);
        s.size = Some(999);
        s.sha256 = None;
        assert!(install_item(t.path(), &item("a", vec![s]), &Cancel::default(), &mut |_, _| {}).unwrap_err().to_string().contains("wrong size"));
        let missing = spec(&srv.base, "nope.bin", &d);
        assert!(install_item(t.path(), &item("b", vec![missing]), &Cancel::default(), &mut |_, _| {}).unwrap_err().to_string().contains("404"));
    }

    #[test]
    fn cancel_stops_the_download_and_keeps_the_partial_for_resume() {
        let d = data(3_000_000);
        let srv = serve(vec![("m.bin", d.clone())]);
        let t = tempfile::tempdir().unwrap();
        let cancel = Cancel::default();
        let c2 = cancel.clone();
        let it = item("a", vec![spec(&srv.base, "m.bin", &d)]);
        let e = install_item(t.path(), &it, &cancel, &mut |done, _| {
            if done > 300_000 {
                c2.cancel();
            }
        })
        .unwrap_err()
        .to_string();
        assert_eq!(e, "cancelled");
        assert!(t.path().join("a/m.bin.part").exists());
        assert!(!is_installed(t.path(), "a"));
        // a fresh attempt finishes the job
        install_item(t.path(), &it, &Cancel::default(), &mut |_, _| {}).unwrap();
        assert_eq!(fs::read(t.path().join("a/m.bin")).unwrap(), d);
    }

    fn make_zip(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut buf = std::io::Cursor::new(Vec::new());
        {
            let mut w = zip::ZipWriter::new(&mut buf);
            for (name, body) in entries {
                w.start_file(*name, zip::write::SimpleFileOptions::default()).unwrap();
                w.write_all(body).unwrap();
            }
            w.finish().unwrap();
        }
        buf.into_inner()
    }

    #[test]
    fn zip_archives_are_extracted_and_the_archive_removed() {
        let z = make_zip(&[("bin/llama-server.exe", b"EXE"), ("readme.txt", b"hi")]);
        let srv = serve(vec![("rt.zip", z.clone())]);
        let t = tempfile::tempdir().unwrap();
        let mut s = spec(&srv.base, "rt.zip", &z);
        s.extract = true;
        let dir = install_item(t.path(), &item("rt", vec![s]), &Cancel::default(), &mut |_, _| {}).unwrap();
        assert_eq!(fs::read(dir.join("bin/llama-server.exe")).unwrap(), b"EXE");
        assert!(!dir.join("rt.zip").exists());
    }

    fn make_tar_gz(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut raw = Vec::new();
        {
            let mut builder = tar::Builder::new(&mut raw);
            for (name, body) in entries {
                let mut header = tar::Header::new_gnu();
                header.set_size(body.len() as u64);
                header.set_mode(0o644);
                header.set_cksum();
                builder.append_data(&mut header, *name, *body).unwrap();
            }
            builder.finish().unwrap();
        }
        let mut out = Vec::new();
        let mut enc = flate2::write::GzEncoder::new(&mut out, flate2::Compression::fast());
        enc.write_all(&raw).unwrap();
        enc.finish().unwrap();
        out
    }

    #[test]
    fn tar_gz_archives_are_extracted_and_the_archive_removed() {
        let z = make_tar_gz(&[("llama-b/llama-server", b"BIN"), ("llama-b/readme.txt", b"hi")]);
        let srv = serve(vec![("rt.tar.gz", z.clone())]);
        let t = tempfile::tempdir().unwrap();
        let mut s = spec(&srv.base, "rt.tar.gz", &z);
        s.extract = true;
        let dir = install_item(t.path(), &item("rt-mac", vec![s]), &Cancel::default(), &mut |_, _| {}).unwrap();
        assert_eq!(fs::read(dir.join("llama-b/llama-server")).unwrap(), b"BIN");
        assert!(!dir.join("rt.tar.gz").exists());
    }

    fn make_tar_gz_with_symlink(file_name: &str, body: &[u8], link_name: &str, link_target: &str) -> Vec<u8> {
        let mut raw = Vec::new();
        {
            let mut builder = tar::Builder::new(&mut raw);
            let mut header = tar::Header::new_gnu();
            header.set_size(body.len() as u64);
            header.set_mode(0o755);
            header.set_cksum();
            builder.append_data(&mut header, file_name, body).unwrap();
            let mut link = tar::Header::new_gnu();
            link.set_entry_type(tar::EntryType::Symlink);
            link.set_size(0);
            link.set_mode(0o777);
            link.set_path(link_name).unwrap();
            link.set_link_name(link_target).unwrap();
            link.set_cksum();
            builder.append(&link, std::io::empty()).unwrap();
            builder.finish().unwrap();
        }
        let mut out = Vec::new();
        let mut enc = flate2::write::GzEncoder::new(&mut out, flate2::Compression::fast());
        enc.write_all(&raw).unwrap();
        enc.finish().unwrap();
        out
    }

    #[test]
    fn tar_gz_preserves_versioned_dylib_symlinks() {
        // llama.cpp macOS builds ship libfoo.1.dylib -> libfoo.1.0.0.dylib; flattening those to
        // empty files makes dyld abort with "Library not loaded".
        let z = make_tar_gz_with_symlink(
            "llama-b/libllama-common.1.0.0.dylib",
            b"REALDYLIB",
            "llama-b/libllama-common.1.dylib",
            "libllama-common.1.0.0.dylib",
        );
        let srv = serve(vec![("rt.tar.gz", z.clone())]);
        let t = tempfile::tempdir().unwrap();
        let mut s = spec(&srv.base, "rt.tar.gz", &z);
        s.extract = true;
        let dir = install_item(t.path(), &item("rt-mac", vec![s]), &Cancel::default(), &mut |_, _| {}).unwrap();
        let link = dir.join("llama-b/libllama-common.1.dylib");
        let real = dir.join("llama-b/libllama-common.1.0.0.dylib");
        assert_eq!(fs::read(&real).unwrap(), b"REALDYLIB");
        #[cfg(unix)]
        {
            let meta = fs::symlink_metadata(&link).unwrap();
            assert!(meta.file_type().is_symlink(), "must stay a symlink, not an empty file");
            assert_eq!(fs::read_link(&link).unwrap(), PathBuf::from("libllama-common.1.0.0.dylib"));
            assert_eq!(fs::read(&link).unwrap(), b"REALDYLIB");
        }
        #[cfg(not(unix))]
        {
            assert_eq!(fs::read(&link).unwrap(), b"REALDYLIB", "Windows materializes the symlink target");
        }
    }

    #[test]
    fn archive_relative_paths_reject_escape() {
        assert!(safe_archive_rel("../evil.txt").is_err());
        assert!(safe_archive_rel("/abs/evil.txt").is_err());
        assert!(safe_archive_rel("llama-b/llama-server").is_ok());
    }

    #[test]
    fn ensure_runtime_dylibs_is_a_noop_for_windows_style_layouts() {
        // Windows ships real .dlls next to llama-server.exe, not versioned .dylib symlinks.
        let t = tempfile::tempdir().unwrap();
        let exe = t.path().join("llama-server.exe");
        fs::write(&exe, b"EXE").unwrap();
        fs::write(t.path().join("llama.dll"), b"DLL").unwrap();
        ensure_runtime_dylibs(&exe).unwrap();
    }

    #[test]
    fn prune_incompatible_runtimes_keeps_only_this_machines_arch() {
        use super::super::manifest::{Item, Manifest, Slot};
        let m = Manifest {
            version: 1,
            items: vec![
                Item {
                    id: "rt-mac-arm".into(),
                    slot: Slot::Runtime,
                    label: "arm".into(),
                    description: String::new(),
                    license: "MIT".into(),
                    platform: "macos".into(),
                    arch: Some("arm64".into()),
                    files: vec![],
                },
                Item {
                    id: "rt-mac-x64".into(),
                    slot: Slot::Runtime,
                    label: "x64".into(),
                    description: String::new(),
                    license: "MIT".into(),
                    platform: "macos".into(),
                    arch: Some("x64".into()),
                    files: vec![],
                },
            ],
        };
        let t = tempfile::tempdir().unwrap();
        fs::create_dir_all(t.path().join("rt-mac-arm")).unwrap();
        fs::write(t.path().join("rt-mac-arm/.installed"), b"ok").unwrap();
        fs::create_dir_all(t.path().join("rt-mac-x64")).unwrap();
        fs::write(t.path().join("rt-mac-x64/.installed"), b"ok").unwrap();
        assert_eq!(prune_incompatible_runtimes(t.path(), &m, "macos", "arm64"), 1);
        assert!(is_installed(t.path(), "rt-mac-arm"));
        assert!(!is_installed(t.path(), "rt-mac-x64"));
    }

    #[test]
    fn zip_slip_entries_are_rejected_and_nothing_escapes() {
        let z = make_zip(&[("../evil.txt", b"pwned")]);
        let srv = serve(vec![("rt.zip", z.clone())]);
        let t = tempfile::tempdir().unwrap();
        let mut s = spec(&srv.base, "rt.zip", &z);
        s.extract = true;
        assert!(install_item(t.path(), &item("rt", vec![s]), &Cancel::default(), &mut |_, _| {}).is_err());
        assert!(!t.path().join("evil.txt").exists());
        assert!(!is_installed(t.path(), "rt"));
    }

    #[test]
    fn multi_file_progress_is_cumulative_and_uninstall_removes_everything() {
        let (a, b) = (data(100_000), data(50_000));
        let srv = serve(vec![("a.bin", a.clone()), ("b.bin", b.clone())]);
        let t = tempfile::tempdir().unwrap();
        let it = item("m", vec![spec(&srv.base, "a.bin", &a), spec(&srv.base, "b.bin", &b)]);
        let mut max = 0;
        install_item(t.path(), &it, &Cancel::default(), &mut |d, tot| {
            assert_eq!(tot, 150_000);
            assert!(d >= max, "monotonic");
            max = d;
        })
        .unwrap();
        assert_eq!(max, 150_000);
        uninstall_item(t.path(), "m").unwrap();
        assert!(!t.path().join("m").exists() && !is_installed(t.path(), "m"));
        uninstall_item(t.path(), "m").unwrap(); // idempotent
    }
}
