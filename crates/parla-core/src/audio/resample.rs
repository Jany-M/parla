use crate::types::SAMPLE_RATE;
use anyhow::{Context, Result};
use rubato::{FftFixedIn, Resampler};

const CHUNK: usize = 1024;

/// Interleaved multi-channel f32 at any rate → mono f32 at 16 kHz, for arbitrary chunk sizes.
pub struct MonoResampler {
    channels: usize,
    rs: Option<FftFixedIn<f32>>,
    pending: Vec<f32>,
}

impl MonoResampler {
    pub fn new(in_rate: u32, channels: usize) -> Result<Self> {
        anyhow::ensure!(channels >= 1 && in_rate > 0, "invalid audio format");
        let rs = if in_rate == SAMPLE_RATE {
            None
        } else {
            Some(FftFixedIn::<f32>::new(in_rate as usize, SAMPLE_RATE as usize, CHUNK, 2, 1).context("audio resampler")?)
        };
        Ok(Self { channels, rs, pending: Vec::new() })
    }

    fn downmix(&self, interleaved: &[f32]) -> Vec<f32> {
        if self.channels == 1 {
            return interleaved.to_vec();
        }
        interleaved.chunks_exact(self.channels).map(|f| f.iter().sum::<f32>() / self.channels as f32).collect()
    }

    pub fn process(&mut self, interleaved: &[f32]) -> Result<Vec<f32>> {
        let mono = self.downmix(interleaved);
        let Some(rs) = self.rs.as_mut() else { return Ok(mono) };
        self.pending.extend_from_slice(&mono);
        let mut out = Vec::new();
        loop {
            let need = rs.input_frames_next();
            if self.pending.len() < need {
                break;
            }
            let chunk: Vec<f32> = self.pending.drain(..need).collect();
            out.extend(rs.process(&[chunk], None)?.remove(0));
        }
        Ok(out)
    }

    /// Flushes the tail at end of stream.
    pub fn finish(&mut self) -> Result<Vec<f32>> {
        let Some(rs) = self.rs.as_mut() else { return Ok(Vec::new()) };
        if self.pending.is_empty() {
            return Ok(Vec::new());
        }
        let tail = std::mem::take(&mut self.pending);
        Ok(rs.process_partial(Some(&[tail]), None)?.remove(0))
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub fn sine(rate: u32, secs: f32, freq: f32, amp: f32) -> Vec<f32> {
        (0..(rate as f32 * secs) as usize).map(|i| amp * (2.0 * std::f32::consts::PI * freq * i as f32 / rate as f32).sin()).collect()
    }

    fn rms(x: &[f32]) -> f32 {
        (x.iter().map(|v| v * v).sum::<f32>() / x.len() as f32).sqrt()
    }

    #[test]
    fn sixteen_k_mono_passes_through_untouched() {
        let mut r = MonoResampler::new(16_000, 1).unwrap();
        let x = sine(16_000, 0.5, 300.0, 0.4);
        assert_eq!(r.process(&x).unwrap(), x);
        assert!(r.finish().unwrap().is_empty());
    }

    #[test]
    fn stereo_is_averaged_to_mono() {
        let mut r = MonoResampler::new(16_000, 2).unwrap();
        assert_eq!(r.process(&[1.0, 0.0, 0.5, 0.5, -1.0, 1.0]).unwrap(), [0.5, 0.5, 0.0]);
    }

    #[test]
    fn rates_44k1_and_48k_give_the_right_length_and_keep_the_level() {
        for rate in [44_100u32, 48_000, 22_050, 8_000] {
            let x = sine(rate, 3.0, 440.0, 0.5);
            let mut r = MonoResampler::new(rate, 1).unwrap();
            let mut out = r.process(&x).unwrap();
            out.extend(r.finish().unwrap());
            let want = 3.0 * 16_000.0;
            assert!((out.len() as f32 - want).abs() < want * 0.03, "{rate}: {} samples", out.len());
            let mid = &out[8_000..out.len() - 8_000];
            assert!((rms(mid) - 0.5 / 2f32.sqrt()).abs() < 0.02, "{rate}: rms {}", rms(mid));
        }
    }

    #[test]
    fn chunk_size_of_the_input_does_not_change_the_output() {
        let x = sine(48_000, 2.0, 500.0, 0.5);
        let run = |size: usize| {
            let mut r = MonoResampler::new(48_000, 1).unwrap();
            let mut o = Vec::new();
            for c in x.chunks(size) {
                o.extend(r.process(c).unwrap());
            }
            o.extend(r.finish().unwrap());
            o
        };
        let a = run(x.len());
        let b = run(997);
        assert_eq!(a.len(), b.len());
        assert!(a.iter().zip(&b).all(|(p, q)| (p - q).abs() < 1e-4));
    }

    #[test]
    fn invalid_format_is_rejected() {
        assert!(MonoResampler::new(0, 1).is_err());
        assert!(MonoResampler::new(16_000, 0).is_err());
    }
}
