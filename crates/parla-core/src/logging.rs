use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;

const MAX_BYTES: u64 = 2 * 1024 * 1024;

/// Tiny file logger: one file, rotated to `.old` past 2 MB. Lives under the app root so
/// uninstalling removes it.
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
        let file = std::fs::OpenOptions::new().create(true).append(true).open(&path).ok();
        log::set_boxed_logger(Box::new(FileLogger { path, file: Mutex::new(file) }))?;
        log::set_max_level(log::LevelFilter::Info);
        Ok(())
    }
    pub fn path(&self) -> &PathBuf {
        &self.path
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
    use super::stamp;

    #[test]
    fn stamps_are_readable_utc() {
        assert_eq!(stamp(0), "1970-01-01 00:00:00");
        assert_eq!(stamp(951_782_400 + 3_661), "2000-02-29 01:01:01");
        assert_eq!(stamp(1_794_842_823), "2026-11-16 15:27:03");
    }
}
