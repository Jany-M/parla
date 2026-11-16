//! Hardware detection and the model recommendation rule (one place, one table).
use serde::Serialize;

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct HardwareInfo {
    pub total_ram_mb: u64,
    pub cpu_threads: usize,
    pub has_avx2: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Recommendation {
    pub tier: &'static str,
    pub summarize_item: &'static str,
    pub translate_item: &'static str,
    /// Threads for inference engines: leave one core for the UI/capture.
    pub threads: usize,
    /// Human-readable caveat, if any.
    pub warning: Option<String>,
}

pub const MIN_SUPPORTED_RAM_MB: u64 = 7_000; // "8 GB" machines report ~7.4–7.9 GB usable

pub fn detect() -> HardwareInfo {
    let mut sys = sysinfo::System::new();
    sys.refresh_memory();
    HardwareInfo {
        total_ram_mb: sys.total_memory() / (1024 * 1024),
        cpu_threads: std::thread::available_parallelism().map(|n| n.get()).unwrap_or(2),
        has_avx2: cpu_has_avx2(),
    }
}

pub fn cpu_has_avx2() -> bool {
    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    {
        std::is_x86_feature_detected!("avx2")
    }
    #[cfg(not(any(target_arch = "x86", target_arch = "x86_64")))]
    {
        true // Apple Silicon / ARM: no AVX2 requirement
    }
}

pub fn recommend(hw: &HardwareInfo) -> Recommendation {
    let threads = hw.cpu_threads.saturating_sub(1).clamp(1, 8);
    let (tier, summarize_item) = if hw.total_ram_mb >= 14_000 {
        ("high", "sum-qwen3.5-4b-q4km")
    } else {
        ("low", "sum-qwen3.5-2b-q4km")
    };
    let warning = if !hw.has_avx2 {
        Some("This CPU lacks AVX2; the bundled AI runtime will not start.".to_string())
    } else if hw.total_ram_mb < MIN_SUPPORTED_RAM_MB {
        Some(format!("Only {} MB of RAM detected; 8 GB is the supported minimum.", hw.total_ram_mb))
    } else {
        None
    };
    Recommendation { tier, summarize_item, translate_item: "mt-hy-mt2-1.8b-q4km", threads, warning }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hw(ram: u64, threads: usize, avx2: bool) -> HardwareInfo {
        HardwareInfo { total_ram_mb: ram, cpu_threads: threads, has_avx2: avx2 }
    }

    #[test]
    fn eight_gb_gets_the_small_summary_model_and_no_warning() {
        let r = recommend(&hw(7_800, 8, true));
        assert_eq!(r.tier, "low");
        assert_eq!(r.summarize_item, "sum-qwen3.5-2b-q4km");
        assert!(r.warning.is_none());
        assert_eq!(r.threads, 7);
    }

    #[test]
    fn sixteen_gb_gets_the_4b_model() {
        assert_eq!(recommend(&hw(15_900, 16, true)).summarize_item, "sum-qwen3.5-4b-q4km");
    }

    #[test]
    fn warns_below_minimum_and_without_avx2() {
        assert!(recommend(&hw(4_000, 4, true)).warning.unwrap().contains("4000"));
        assert!(recommend(&hw(16_000, 4, false)).warning.unwrap().contains("AVX2"));
    }

    #[test]
    fn thread_count_is_clamped() {
        assert_eq!(recommend(&hw(8_000, 1, true)).threads, 1);
        assert_eq!(recommend(&hw(8_000, 64, true)).threads, 8);
    }

    #[test]
    fn detect_returns_sane_values() {
        let h = detect();
        assert!(h.total_ram_mb > 100 && h.cpu_threads >= 1);
    }
}
