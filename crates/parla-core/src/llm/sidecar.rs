use super::client::LlmClient;
use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

#[derive(Clone, Debug)]
pub struct LlamaConfig {
    pub exe: PathBuf,
    pub model: PathBuf,
    pub ctx: u32,
    pub threads: usize,
    pub log_file: Option<PathBuf>,
    pub ready_timeout: Duration,
}

/// A llama.cpp `llama-server` child process bound to 127.0.0.1 on a free port.
/// Killed on drop, so closing the app never leaves 2 GB of model memory behind.
pub struct LlamaServer {
    child: Child,
    port: u16,
}

fn free_port() -> Result<u16> {
    let l = std::net::TcpListener::bind("127.0.0.1:0")?;
    Ok(l.local_addr()?.port())
}

impl LlamaServer {
    pub fn start(cfg: &LlamaConfig) -> Result<Self> {
        if !cfg.exe.exists() {
            bail!("language-model runtime not found at {}", cfg.exe.display());
        }
        if !cfg.model.exists() {
            bail!("model file not found at {}", cfg.model.display());
        }
        let port = free_port()?;
        let mut cmd = Command::new(&cfg.exe);
        cmd.arg("-m").arg(&cfg.model)
            .args(["--host", "127.0.0.1", "--port", &port.to_string()])
            .args(["-c", &cfg.ctx.to_string(), "-t", &cfg.threads.to_string(), "-np", "1"])
            .stdin(Stdio::null())
            .stdout(Stdio::null());
        match cfg.log_file.as_ref().and_then(|p| std::fs::OpenOptions::new().create(true).append(true).open(p).ok()) {
            Some(f) => cmd.stderr(Stdio::from(f)),
            None => cmd.stderr(Stdio::null()),
        };
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        }
        let child = cmd.spawn().with_context(|| format!("starting {}", cfg.exe.display()))?;
        let mut me = Self { child, port };
        let probe = LlmClient::new(&me.base_url(), "local", None, Duration::from_secs(2))?;
        let start = Instant::now();
        loop {
            if let Some(status) = me.child.try_wait()? {
                bail!("language-model runtime exited early ({status}); see the log in the app folder");
            }
            if probe.is_ready() {
                return Ok(me);
            }
            if start.elapsed() > cfg.ready_timeout {
                bail!("language model did not become ready within {} s", cfg.ready_timeout.as_secs());
            }
            std::thread::sleep(Duration::from_millis(200));
        }
    }

    pub fn base_url(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    pub fn stop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for LlamaServer {
    fn drop(&mut self) {
        self.stop();
    }
}

fn path_under_dir(path: &Path, dir: &Path) -> bool {
    let path = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    path.starts_with(dir)
}

/// Strip the ` (deleted)` suffix Linux adds to `/proc/pid/exe` when the file was replaced.
#[cfg(target_os = "linux")]
fn strip_deleted_suffix(path: PathBuf) -> PathBuf {
    const DELETED: &[u8] = b" (deleted)";
    let Some(name) = path.file_name() else { return path };
    let bytes = name.as_encoded_bytes();
    if !bytes.ends_with(DELETED) {
        return path;
    }
    let trimmed = bytes[..bytes.len() - DELETED.len()].to_vec();
    let mut out = path;
    // SAFETY: we only removed an ASCII suffix from bytes that were already a valid OsStr.
    unsafe {
        out.set_file_name(std::ffi::OsString::from_encoded_bytes_unchecked(trimmed));
    }
    out
}

/// Linux: read `/proc/*/exe` directly. More reliable than sysinfo here (open-file limits / refresh
/// kind can leave `Process::exe` empty on CI runners, so stale llama-server processes survive).
#[cfg(target_os = "linux")]
fn kill_stale_impl(dir: &Path) -> usize {
    let dir = std::fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
    let Ok(entries) = std::fs::read_dir("/proc") else { return 0 };
    let mut n = 0;
    for entry in entries.flatten() {
        let pid = match entry.file_name().to_str().and_then(|s| s.parse::<i32>().ok()) {
            Some(p) => p,
            None => continue,
        };
        let Ok(raw) = std::fs::read_link(format!("/proc/{pid}/exe")) else { continue };
        let exe = strip_deleted_suffix(raw);
        if !path_under_dir(&exe, &dir) {
            continue;
        }
        let ok = Command::new("kill").args(["-KILL", &pid.to_string()]).status().map(|s| s.success()).unwrap_or(false);
        if ok {
            n += 1;
        }
    }
    n
}

#[cfg(not(target_os = "linux"))]
fn kill_stale_impl(dir: &Path) -> usize {
    let dir = std::fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
    let mut sys = sysinfo::System::new();
    sys.refresh_processes_specifics(
        sysinfo::ProcessesToUpdate::All,
        true,
        sysinfo::ProcessRefreshKind::nothing().with_exe(sysinfo::UpdateKind::Always).without_tasks(),
    );
    let mut n = 0;
    for p in sys.processes().values() {
        let Some(exe) = p.exe() else { continue };
        if path_under_dir(exe, &dir) && p.kill() {
            n += 1;
        }
    }
    n
}

/// Kills leftover runtime processes (e.g. after a crash) whose executable lives under `dir`.
/// Returns how many were killed.
pub fn kill_stale(dir: &Path) -> usize {
    kill_stale_impl(dir)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    const STUB_PY: &str = r#"
import sys, http.server, json
a = sys.argv
port = int(a[a.index('--port') + 1])
class H(http.server.BaseHTTPRequestHandler):
    def log_message(self, *x): pass
    def do_GET(self):
        self.send_response(200); self.end_headers(); self.wfile.write(b'{"status":"ok"}')
http.server.HTTPServer(('127.0.0.1', port), H).serve_forever()
"#;

    fn have_python() -> bool {
        Command::new("python3").arg("--version").output().map(|o| o.status.success()).unwrap_or(false)
    }

    fn stub(dir: &Path, body: &str) -> PathBuf {
        let exe = dir.join("llama-server");
        std::fs::write(&exe, body).unwrap();
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
        exe
    }

    fn cfg(dir: &Path, exe: PathBuf) -> LlamaConfig {
        let model = dir.join("m.gguf");
        std::fs::write(&model, b"GGUF").unwrap();
        LlamaConfig { exe, model, ctx: 2048, threads: 2, log_file: Some(dir.join("log.txt")), ready_timeout: Duration::from_secs(10) }
    }

    #[test]
    fn starts_waits_for_health_and_stops_on_drop() {
        if !have_python() {
            return;
        }
        let t = tempfile::tempdir().unwrap();
        std::fs::write(t.path().join("stub.py"), STUB_PY).unwrap();
        let exe = stub(t.path(), &format!("#!/bin/sh\nexec python3 {} \"$@\"\n", t.path().join("stub.py").display()));
        let s = LlamaServer::start(&cfg(t.path(), exe)).unwrap();
        let url = s.base_url();
        assert!(LlmClient::new(&url, "m", None, Duration::from_secs(2)).unwrap().is_ready());
        drop(s);
        std::thread::sleep(Duration::from_millis(300));
        assert!(!LlmClient::new(&url, "m", None, Duration::from_secs(1)).unwrap().is_ready(), "process must die with the handle");
    }

    #[test]
    fn early_exit_is_reported_immediately_not_after_the_timeout() {
        let t = tempfile::tempdir().unwrap();
        let exe = stub(t.path(), "#!/bin/sh\nexit 3\n");
        let started = Instant::now();
        let e = LlamaServer::start(&cfg(t.path(), exe)).err().expect("must fail").to_string();
        assert!(e.contains("exited early"), "{e}");
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn missing_files_are_clear_errors() {
        let t = tempfile::tempdir().unwrap();
        let mut c = cfg(t.path(), t.path().join("nope"));
        assert!(LlamaServer::start(&c).err().unwrap().to_string().contains("runtime not found"));
        c.exe = stub(t.path(), "#!/bin/sh\n");
        c.model = t.path().join("missing.gguf");
        assert!(LlamaServer::start(&c).err().unwrap().to_string().contains("model file not found"));
    }

    /// Real ELF under `dir` (copying `/bin/sleep` exits immediately on some runners when argv0 changes).
    fn stale_stub(dir: &Path) -> PathBuf {
        let src = dir.join("stale_stub.rs");
        let exe = dir.join("llama-server");
        std::fs::write(&src, "fn main() { loop { std::thread::park(); } }\n").unwrap();
        let st = Command::new("rustc").args(["-C", "opt-level=0", "-o"]).arg(&exe).arg(&src).status().expect("run rustc");
        assert!(st.success(), "rustc failed to build kill_stale stub: {st}");
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
        exe
    }

    #[test]
    fn kill_stale_only_kills_processes_from_the_given_dir() {
        let t = tempfile::tempdir().unwrap();
        let other_dir = tempfile::tempdir().unwrap();
        let mine_exe = stale_stub(t.path());
        let other_exe = stale_stub(other_dir.path());
        let mut mine = Command::new(&mine_exe).spawn().unwrap();
        let mut other = Command::new(&other_exe).spawn().unwrap();
        std::thread::sleep(Duration::from_millis(100));
        if let Some(st) = mine.try_wait().unwrap() {
            other.kill().ok();
            panic!("stub exited immediately ({st}); cannot test kill_stale");
        }
        #[cfg(target_os = "linux")]
        {
            let exe = strip_deleted_suffix(std::fs::read_link(format!("/proc/{}/exe", mine.id())).expect("read /proc/pid/exe"));
            assert!(
                path_under_dir(&exe, &std::fs::canonicalize(t.path()).unwrap()),
                "child exe {exe:?} should live under {:?}",
                t.path()
            );
        }
        let killed = kill_stale(t.path());
        assert_eq!(killed, 1, "kill_stale should kill only the runtime under the temp dir");
        assert!(mine.wait().is_ok());
        assert!(other.try_wait().unwrap().is_none(), "unrelated process must survive");
        other.kill().unwrap();
        other.wait().unwrap();
    }
}
