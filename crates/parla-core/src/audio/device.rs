//! Live capture through cpal.
//! * Microphone: an input device.
//! * System audio ("what the other people say"): on Windows (WASAPI) an *output* device opened
//!   as input is captured in loopback mode — no virtual cable, no per-app integration.
//!   macOS Core Audio taps are the same call on macOS 14.6+ (deferred D11).
use super::{AudioSource, MonoResampler, FRAME_SAMPLES};
use crate::types::{Frame, StreamKind};
use anyhow::{anyhow, bail, Context, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SampleFormat, SizedSample};
use serde::Serialize;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{SyncSender, TrySendError};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Short settle for every mic (AGC / first callback jitter), in 16 kHz mono samples.
pub const MIC_WARMUP_SAMPLES: u64 = 4_800; // 0.3 s
/// Extra discard when the device looks like Bluetooth SCO/HFP (~0.5–1.5 s profile switch).
pub const MIC_BT_WARMUP_SAMPLES: u64 = 20_000; // 1.25 s

fn looks_like_bluetooth(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    // Common Windows/macOS labels for BT headsets and hands-free AG profiles.
    [
        "bluetooth",
        "hands-free", // Windows BT AG profile (avoid bare "headset" — many wired mics use it)
        "handsfree",
        "airpods",
        "galaxy buds",
        "wh-",
        "wf-",
        "hfp",
        "sco",
        "le audio",
    ]
    .iter()
    .any(|k| n.contains(k))
}

fn mic_warmup_samples(device_name: &str) -> u64 {
    if looks_like_bluetooth(device_name) {
        MIC_BT_WARMUP_SAMPLES
    } else {
        MIC_WARMUP_SAMPLES
    }
}

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct DeviceInfo {
    /// Stable id stored in settings.
    pub id: String,
    pub name: String,
    pub is_default: bool,
}

fn describe<D: DeviceTrait>(d: &D, default_id: Option<&str>) -> Option<DeviceInfo> {
    let id = d.id().ok()?.to_string();
    let name = d.description().ok().map(|x| x.name().to_string()).unwrap_or_else(|| id.clone());
    Some(DeviceInfo { is_default: Some(id.as_str()) == default_id, id, name })
}

/// Microphones.
pub fn list_inputs() -> Vec<DeviceInfo> {
    let host = cpal::default_host();
    let def = host.default_input_device().and_then(|d| d.id().ok()).map(|i| i.to_string());
    host.input_devices().map(|it| it.filter_map(|d| describe(&d, def.as_deref())).collect()).unwrap_or_default()
}

/// Playback devices whose output can be captured as "system audio".
pub fn list_outputs() -> Vec<DeviceInfo> {
    let host = cpal::default_host();
    let def = host.default_output_device().and_then(|d| d.id().ok()).map(|i| i.to_string());
    host.output_devices().map(|it| it.filter_map(|d| describe(&d, def.as_deref())).collect()).unwrap_or_default()
}

pub struct DeviceSource {
    pub stream: StreamKind,
    /// Device id from `list_*`; `None` = system default.
    pub device_id: Option<String>,
    /// true = capture what is being played (loopback), false = microphone.
    pub loopback: bool,
}

impl DeviceSource {
    pub fn mic(device_id: Option<String>) -> Self {
        Self { stream: StreamKind::Mic, device_id, loopback: false }
    }
    pub fn system(device_id: Option<String>) -> Self {
        Self { stream: StreamKind::System, device_id, loopback: true }
    }

    fn open(&self) -> Result<cpal::Device> {
        let host = cpal::default_host();
        let by_id = |id: &str| -> Result<cpal::Device> {
            let parsed: cpal::DeviceId = id.parse().map_err(|_| anyhow!("unknown audio device id '{id}'"))?;
            host.device_by_id(&parsed).ok_or_else(|| anyhow!("the selected audio device is no longer available"))
        };
        match (&self.device_id, self.loopback) {
            (Some(id), _) => by_id(id),
            (None, true) => host.default_output_device().context("no playback device found to capture system audio from"),
            (None, false) => host.default_input_device().context("no microphone found"),
        }
    }
}

fn build<T>(
    device: &cpal::Device,
    cfg: cpal::StreamConfig,
    kind: StreamKind,
    tx: SyncSender<Frame>,
    failure: Arc<Mutex<Option<String>>>,
    warmup: u64,
) -> Result<cpal::Stream>
where
    T: SizedSample + Send + 'static,
    f32: FromSample<T>,
{
    let mut rs = MonoResampler::new(cfg.sample_rate, cfg.channels as usize)?;
    let mut acc: Vec<f32> = Vec::with_capacity(FRAME_SAMPLES * 2);
    let warmed = AtomicU64::new(0);
    let mut logged_ready = false;
    let stream = device.build_input_stream(
        cfg,
        move |data: &[T], _| {
            let f: Vec<f32> = data.iter().map(|s| s.to_sample::<f32>()).collect();
            let Ok(mono) = rs.process(&f) else { return };
            if warmup > 0 {
                let prev = warmed.fetch_add(mono.len() as u64, Ordering::Relaxed);
                if prev < warmup {
                    // Keep the resampler ticking so the link/AGC settle; do not feed the pipeline yet.
                    return;
                }
                if !logged_ready {
                    logged_ready = true;
                    log::info!("microphone warm-up done ({warmup} samples discarded)");
                }
            }
            acc.extend(mono);
            while acc.len() >= FRAME_SAMPLES {
                let samples: Vec<f32> = acc.drain(..FRAME_SAMPLES).collect();
                // never block the real-time audio thread: drop audio if the pipeline is behind
                match tx.try_send(Frame { stream: kind, samples }) {
                    Ok(()) | Err(TrySendError::Full(_)) | Err(TrySendError::Disconnected(_)) => {}
                }
            }
        },
        move |e| {
            // Buffer under/overruns are routine (Bluetooth headsets, busy CPUs): a few samples
            // are lost, the stream keeps running. Anything else (device removed…) is fatal.
            if e.kind() != cpal::ErrorKind::Xrun {
                *failure.lock().unwrap() = Some(e.to_string());
            }
        },
        None,
    )?;
    Ok(stream)
}

impl AudioSource for DeviceSource {
    fn run(&mut self, tx: &SyncSender<Frame>, stop: &AtomicBool) -> Result<()> {
        let device = self.open()?;
        let supported = if self.loopback { device.default_output_config() } else { device.default_input_config() }.context("reading the audio device format")?;
        let (format, cfg) = (supported.sample_format(), supported.config());
        let failure = Arc::new(Mutex::new(None));
        // Only the microphone needs a warm-up. Loopback must not drop the start of the meeting.
        let device_name = device.description().ok().map(|x| x.name().to_string()).unwrap_or_default();
        let warmup = if self.loopback { 0 } else { mic_warmup_samples(&device_name) };
        if warmup > 0 {
            log::info!(
                "opening microphone '{device_name}' with {warmup} sample warm-up (~{:.2}s{})",
                warmup as f64 / 16_000.0,
                if looks_like_bluetooth(&device_name) { ", bluetooth" } else { "" }
            );
        }
        let (tx2, f2, kind) = (tx.clone(), failure.clone(), self.stream);
        let stream = match format {
            SampleFormat::F32 => build::<f32>(&device, cfg, kind, tx2, f2, warmup),
            SampleFormat::I16 => build::<i16>(&device, cfg, kind, tx2, f2, warmup),
            SampleFormat::U16 => build::<u16>(&device, cfg, kind, tx2, f2, warmup),
            SampleFormat::I32 => build::<i32>(&device, cfg, kind, tx2, f2, warmup),
            other => bail!("unsupported audio sample format {other:?}"),
        }
        .context("starting audio capture")?;
        stream.play().context("starting audio capture")?;
        while !stop.load(Ordering::SeqCst) {
            if let Some(e) = failure.lock().unwrap().take() {
                bail!("{e}");
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bluetooth_names_get_the_long_warmup() {
        for name in [
            "Headphones (Jabra Evolve2 65 Hands-Free AG)",
            "AirPods Pro",
            "WH-1000XM5",
            "Galaxy Buds2 Pro Hands-Free AG",
            "Bluetooth Microphone",
        ] {
            assert_eq!(mic_warmup_samples(name), MIC_BT_WARMUP_SAMPLES, "{name}");
            assert!(looks_like_bluetooth(name), "{name}");
        }
    }

    #[test]
    fn wired_and_builtin_mics_get_the_short_warmup() {
        for name in ["Microphone (Realtek Audio)", "MacBook Pro Microphone", "USB Audio Device", ""] {
            assert_eq!(mic_warmup_samples(name), MIC_WARMUP_SAMPLES, "{name}");
            assert!(!looks_like_bluetooth(name), "{name}");
        }
    }
}
