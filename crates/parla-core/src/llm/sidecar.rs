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

/// Kills leftover runtime processes (e.g. after a crash) whose executable lives under `dir`.
/// Returns how many were killed.
pub fn kill_stale(dir: &Path) -> usize {
    let mut sys = sysinfo::System::new();
    sys.refresh_processes(sysinfo::ProcessesToUpdate::All, true);
    let mut n = 0;
    for p in sys.processes().values() {
        if p.exe().map(|e| e.starts_with(dir)).unwrap_or(false) && p.kill() {
            n += 1;
        }
    }
    n
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

    #[test]
    fn kill_stale_only_kills_processes_from_the_given_dir() {
        let t = tempfile::tempdir().unwrap();
        let sleep = t.path().join("llama-server");
        std::fs::copy("/bin/sleep", &sleep).unwrap();
        let mut mine = Command::new(&sleep).arg("60").spawn().unwrap();
        let mut other = Command::new("/bin/sleep").arg("60").spawn().unwrap();
        std::thread::sleep(Duration::from_millis(200));
        assert_eq!(kill_stale(t.path()), 1);
        assert!(mine.wait().is_ok());
        assert!(other.try_wait().unwrap().is_none(), "unrelated process must survive");
        other.kill().unwrap();
        other.wait().unwrap();
    }
}
