use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

const MAX_BYTES: u64 = 2 * 1024 * 1024;
const RETAIN_DAYS: u64 = 30;
const STAMP_LEN: usize = 19; // "YYYY-MM-DD HH:MM:SS"

/// Tiny file logger: one file, rotated to `.old` past 2 MB. Lives under the app root so
/// uninstalling removes it. On open, lines older than 30 days are dropped.
pub struct FileLogger {
    path: PathBuf,
    file: Mutex<Option<std::fs::File>>,
}

impl FileLogger {
    pub fn init(dir: &std::path::Path) -> Result<(), log::SetLoggerError> {
        let _ = std::fs::create_dir_all(dir);
        let path = dir.join("parla.log");
        if std::fs::metadata(&path).map(|m| m.len() > MAX_BYTES).unwrap_or(false) {
            let _ = std::fs::rename(&path, dir.join("parla.log.old"));
        }
        prune_log_file(&path, RETAIN_DAYS);
        prune_log_file(&dir.join("parla.log.old"), RETAIN_DAYS);
        let file = std::fs::OpenOptions::new().create(true).append(true).open(&path).ok();
        log::set_boxed_logger(Box::new(FileLogger { path, file: Mutex::new(file) }))?;
        log::set_max_level(log::LevelFilter::Info);
        Ok(())
    }
    pub fn path(&self) -> &PathBuf {
        &self.path
    }
}

/// Drop lines whose leading stamp is older than `retain_days`. No-op if the file is missing.
/// Assumes chronological append order (find first kept line, rewrite from there).
pub fn prune_log_file(path: &Path, retain_days: u64) {
    let Ok(meta) = std::fs::metadata(path) else {
        return;
    };
    if !meta.is_file() || meta.len() == 0 {
        return;
    }
    let Ok(content) = std::fs::read_to_string(path) else {
        return;
    };
    if content.is_empty() {
        return;
    }

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let cutoff = stamp(now.saturating_sub(retain_days.saturating_mul(86_400)));

    let mut keep_from: Option<usize> = None;
    for (idx, line) in content.lines().enumerate() {
        if let Some(ts) = line_stamp(line) {
            if ts >= cutoff.as_str() {
                keep_from = Some(idx);
                break;
            }
        }
    }

    match keep_from {
        Some(0) => {} // nothing older than the cutoff
        Some(idx) => {
            let mut kept = content.lines().skip(idx).collect::<Vec<_>>().join("\n");
            if content.ends_with('\n') {
                kept.push('\n');
            }
            let _ = std::fs::write(path, kept);
        }
        None => {
            // Every stamped line is too old (or file had no stamps) — clear it.
            let _ = std::fs::write(path, b"");
        }
    }
}

fn line_stamp(line: &str) -> Option<&str> {
    let ts = line.get(..STAMP_LEN)?;
    let b = ts.as_bytes();
    if b.len() == STAMP_LEN
        && b[4] == b'-'
        && b[7] == b'-'
        && b[10] == b' '
        && b[13] == b':'
        && b[16] == b':'
    {
        Some(ts)
    } else {
        None
    }
}

/// Unix seconds → "2026-11-16 15:27:03" (UTC), without a date library.
pub fn stamp(secs: u64) -> String {
    let (days, rem) = ((secs / 86_400) as i64, secs % 86_400);
    // civil-from-days (Howard Hinnant)
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02} {:02}:{:02}:{:02}", rem / 3600, rem % 3600 / 60, rem % 60)
}

impl log::Log for FileLogger {
    fn enabled(&self, m: &log::Metadata) -> bool {
        m.level() <= log::Level::Info
    }
    fn log(&self, r: &log::Record) {
        if !self.enabled(r.metadata()) {
            return;
        }
        let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        if let Ok(mut g) = self.file.lock() {
            if let Some(f) = g.as_mut() {
                let _ = writeln!(f, "{} {:5} {} - {}", stamp(secs), r.level(), r.target(), r.args());
                if r.level() <= log::Level::Warn {
                    let _ = f.flush();
                }
            }
        }
    }
    fn flush(&self) {
        if let Ok(mut g) = self.file.lock() {
            if let Some(f) = g.as_mut() {
                let _ = f.flush();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{line_stamp, prune_log_file, stamp};
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn stamps_are_readable_utc() {
        assert_eq!(stamp(0), "1970-01-01 00:00:00");
        assert_eq!(stamp(951_782_400 + 3_661), "2000-02-29 01:01:01");
        assert_eq!(stamp(1_794_842_823), "2026-11-16 15:27:03");
    }

    #[test]
    fn line_stamp_reads_leading_utc() {
        assert_eq!(
            line_stamp("2026-11-16 15:27:03 INFO  parla - hi"),
            Some("2026-11-16 15:27:03")
        );
        assert!(line_stamp("not a log line").is_none());
        assert!(line_stamp("short").is_none());
    }

    #[test]
    fn prune_drops_lines_older_than_retain_window() {
        let dir = std::env::temp_dir().join(format!("parla-log-prune-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("parla.log");

        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
        let old = stamp(now.saturating_sub(40 * 86_400));
        let mid = stamp(now.saturating_sub(10 * 86_400));
        let recent = stamp(now.saturating_sub(86_400));
        let body = format!(
            "{old} INFO  t - ancient\n{mid} INFO  t - keep-me\n{recent} WARN  t - also-keep\n"
        );
        std::fs::write(&path, &body).unwrap();

        prune_log_file(&path, 30);

        let kept = std::fs::read_to_string(&path).unwrap();
        assert!(!kept.contains("ancient"));
        assert!(kept.contains("keep-me"));
        assert!(kept.contains("also-keep"));
        assert!(kept.starts_with(&mid));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn prune_clears_when_everything_is_old() {
        let dir = std::env::temp_dir().join(format!("parla-log-prune-all-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("parla.log");

        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
        let old = stamp(now.saturating_sub(60 * 86_400));
        std::fs::write(&path, format!("{old} INFO  t - gone\n")).unwrap();

        prune_log_file(&path, 30);

        assert_eq!(std::fs::read_to_string(&path).unwrap(), "");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn prune_is_noop_when_file_missing() {
        let path = std::env::temp_dir().join(format!("parla-log-missing-{}.log", std::process::id()));
        let _ = std::fs::remove_file(&path);
        prune_log_file(&path, 30); // must not panic
    }
}
