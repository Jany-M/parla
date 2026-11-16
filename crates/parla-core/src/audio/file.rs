use super::{AudioSource, MonoResampler, FRAME_SAMPLES};
use crate::types::{Frame, StreamKind};
use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::SyncSender;
use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::{DecoderOptions, CODEC_TYPE_NULL};
use symphonia::core::errors::Error as SymError;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

/// Decodes any audio or video file (the audio track) into 16 kHz mono and hands it to `on_samples`
/// in arbitrary sizes. Returns the decoded duration in seconds.
/// `progress` receives 0..1 when the container reports its length.
/// Return `false` from `on_samples` to stop early.
pub fn decode_file(path: &Path, on_samples: &mut dyn FnMut(&[f32]) -> bool, progress: &mut dyn FnMut(f32)) -> Result<f64> {
    let file = std::fs::File::open(path).with_context(|| format!("opening {}", path.display()))?;
    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }
    let mss = MediaSourceStream::new(Box::new(file), Default::default());
    let probed = symphonia::default::get_probe()
        .format(&hint, mss, &FormatOptions::default(), &MetadataOptions::default())
        .context("this file format is not supported (or the file is damaged)")?;
    let mut format = probed.format;
    let track = format.tracks().iter().find(|t| t.codec_params.codec != CODEC_TYPE_NULL).context("no audio track found in this file")?;
    let (track_id, params) = (track.id, track.codec_params.clone());
    let total_frames = params.n_frames;
    let mut decoder = symphonia::default::get_codecs().make(&params, &DecoderOptions::default()).context("unsupported audio codec")?;

    let mut resampler: Option<MonoResampler> = None;
    let mut out_samples = 0u64;
    let mut in_frames = 0u64;
    let mut sbuf: Option<SampleBuffer<f32>> = None;
    let mut keep_going = true;

    while keep_going {
        let packet = match format.next_packet() {
            Ok(p) => p,
            Err(SymError::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(SymError::ResetRequired) => break,
            Err(e) => return Err(e).context("reading the file failed"),
        };
        if packet.track_id() != track_id {
            continue;
        }
        let decoded = match decoder.decode(&packet) {
            Ok(d) => d,
            Err(SymError::DecodeError(e)) => {
                log::warn!("skipping undecodable packet: {e}");
                continue;
            }
            Err(e) => return Err(e).context("decoding failed"),
        };
        let spec = *decoded.spec();
        if resampler.is_none() {
            resampler = Some(MonoResampler::new(spec.rate, spec.channels.count())?);
        }
        if sbuf.as_ref().map(|b| b.capacity() < decoded.capacity() * spec.channels.count()).unwrap_or(true) {
            sbuf = Some(SampleBuffer::<f32>::new(decoded.capacity() as u64, spec));
        }
        let sb = sbuf.as_mut().unwrap();
        sb.copy_interleaved_ref(decoded);
        in_frames += (sb.samples().len() / spec.channels.count()) as u64;
        let mono = resampler.as_mut().unwrap().process(sb.samples())?;
        out_samples += mono.len() as u64;
        if !mono.is_empty() {
            keep_going = on_samples(&mono);
        }
        if let Some(t) = total_frames.filter(|t| *t > 0) {
            progress((in_frames as f32 / t as f32).min(1.0));
        }
    }
    if let (true, Some(r)) = (keep_going, resampler.as_mut()) {
        let tail = r.finish()?;
        out_samples += tail.len() as u64;
        if !tail.is_empty() {
            on_samples(&tail);
        }
    }
    if out_samples == 0 {
        bail!("no audio could be read from this file");
    }
    progress(1.0);
    Ok(out_samples as f64 / crate::types::SAMPLE_RATE as f64)
}

/// A file as an `AudioSource`: frames are produced faster than real time (bounded by the channel).
pub struct FileSource {
    pub path: PathBuf,
    pub progress: Box<dyn FnMut(f32) + Send>,
}

impl AudioSource for FileSource {
    fn run(&mut self, tx: &SyncSender<Frame>, stop: &AtomicBool) -> Result<()> {
        let mut buf: Vec<f32> = Vec::new();
        let mut alive = true;
        let send = |buf: &mut Vec<f32>, all: bool| -> bool {
            while buf.len() >= FRAME_SAMPLES || (all && !buf.is_empty()) {
                let n = buf.len().min(FRAME_SAMPLES);
                let frame = Frame { stream: StreamKind::File, samples: buf.drain(..n).collect() };
                if tx.send(frame).is_err() {
                    return false;
                }
            }
            true
        };
        let progress = &mut self.progress;
        decode_file(
            &self.path,
            &mut |s| {
                buf.extend_from_slice(s);
                alive = send(&mut buf, false) && !stop.load(Ordering::SeqCst);
                alive
            },
            &mut |p| progress(p),
        )?;
        if alive {
            send(&mut buf, true);
        }
        Ok(())
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::audio::resample::tests::sine;

    /// Minimal PCM16 WAV writer for fixtures.
    pub fn wav_bytes(rate: u32, channels: u16, samples: &[f32]) -> Vec<u8> {
        let data: Vec<u8> = samples.iter().flat_map(|s| ((s.clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes()).collect();
        let mut v = Vec::new();
        v.extend(b"RIFF");
        v.extend((36 + data.len() as u32).to_le_bytes());
        v.extend(b"WAVEfmt ");
        v.extend(16u32.to_le_bytes());
        v.extend(1u16.to_le_bytes());
        v.extend(channels.to_le_bytes());
        v.extend(rate.to_le_bytes());
        v.extend((rate * channels as u32 * 2).to_le_bytes());
        v.extend((channels * 2).to_le_bytes());
        v.extend(16u16.to_le_bytes());
        v.extend(b"data");
        v.extend((data.len() as u32).to_le_bytes());
        v.extend(data);
        v
    }

    fn write(dir: &Path, name: &str, bytes: &[u8]) -> PathBuf {
        let p = dir.join(name);
        std::fs::write(&p, bytes).unwrap();
        p
    }

    fn collect(p: &Path) -> (Vec<f32>, f64, Vec<f32>) {
        let (mut all, mut prog) = (vec![], vec![]);
        let d = decode_file(p, &mut |s| { all.extend_from_slice(s); true }, &mut |x| prog.push(x)).unwrap();
        (all, d, prog)
    }

    #[test]
    fn decodes_a_16k_mono_wav_exactly() {
        let t = tempfile::tempdir().unwrap();
        let x = sine(16_000, 1.0, 300.0, 0.5);
        let (out, dur, prog) = collect(&write(t.path(), "a.wav", &wav_bytes(16_000, 1, &x)));
        assert_eq!(out.len(), 16_000);
        assert!((dur - 1.0).abs() < 1e-6);
        assert!(out.iter().zip(&x).all(|(a, b)| (a - b).abs() < 1e-3));
        assert_eq!(*prog.last().unwrap(), 1.0);
    }

    #[test]
    fn stereo_44k1_is_converted_to_16k_mono() {
        let t = tempfile::tempdir().unwrap();
        let mono = sine(44_100, 2.0, 440.0, 0.5);
        let stereo: Vec<f32> = mono.iter().flat_map(|s| [*s, *s]).collect();
        let (out, dur, _) = collect(&write(t.path(), "b.wav", &wav_bytes(44_100, 2, &stereo)));
        assert!((dur - 2.0).abs() < 0.05, "{dur}");
        assert!((out.len() as f64 - 32_000.0).abs() < 800.0);
        let rms = (out[4000..out.len() - 4000].iter().map(|v| v * v).sum::<f32>() / (out.len() - 8000) as f32).sqrt();
        assert!((rms - 0.354).abs() < 0.02, "{rms}");
    }

    #[test]
    fn progress_is_monotonic() {
        let t = tempfile::tempdir().unwrap();
        let (_, _, prog) = collect(&write(t.path(), "c.wav", &wav_bytes(16_000, 1, &sine(16_000, 5.0, 200.0, 0.3))));
        assert!(prog.windows(2).all(|w| w[0] <= w[1]));
    }

    #[test]
    fn garbage_empty_and_missing_files_are_clean_errors() {
        let t = tempfile::tempdir().unwrap();
        let none = |_: &[f32]| true;
        let mut n = none;
        assert!(decode_file(&write(t.path(), "g.mp3", b"this is not audio at all, definitely not"), &mut n, &mut |_| {}).is_err());
        assert!(decode_file(&write(t.path(), "e.wav", b""), &mut n, &mut |_| {}).is_err());
        assert!(decode_file(&t.path().join("missing.wav"), &mut n, &mut |_| {}).is_err());
        assert!(decode_file(&write(t.path(), "h.wav", &wav_bytes(16_000, 1, &[])), &mut n, &mut |_| {}).is_err(), "header only → no audio");
    }

    #[test]
    fn truncated_wav_returns_what_was_readable() {
        let t = tempfile::tempdir().unwrap();
        let mut b = wav_bytes(16_000, 1, &sine(16_000, 2.0, 200.0, 0.3));
        b.truncate(b.len() / 2);
        let (out, _, _) = collect(&write(t.path(), "t.wav", &b));
        assert!(out.len() > 10_000 && out.len() < 20_000, "{}", out.len());
    }

    #[test]
    fn file_source_emits_100ms_frames_tagged_as_file_and_honours_stop() {
        let t = tempfile::tempdir().unwrap();
        let p = write(t.path(), "d.wav", &wav_bytes(16_000, 1, &sine(16_000, 2.0, 200.0, 0.3)));
        let (tx, rx) = std::sync::mpsc::sync_channel(1000);
        let mut src = FileSource { path: p.clone(), progress: Box::new(|_| {}) };
        src.run(&tx, &AtomicBool::new(false)).unwrap();
        drop(tx);
        let frames: Vec<Frame> = rx.iter().collect();
        assert_eq!(frames.iter().map(|f| f.samples.len()).sum::<usize>(), 32_000);
        assert!(frames.iter().all(|f| f.stream == StreamKind::File && f.samples.len() <= FRAME_SAMPLES));

        let (tx, rx) = std::sync::mpsc::sync_channel(1000);
        let mut src = FileSource { path: p, progress: Box::new(|_| {}) };
        src.run(&tx, &AtomicBool::new(true)).unwrap();
        drop(tx);
        assert!(rx.iter().map(|f| f.samples.len()).sum::<usize>() < 32_000, "stop flag ends early");
    }

    #[test]
    fn dropped_receiver_stops_the_source_without_error() {
        let t = tempfile::tempdir().unwrap();
        let p = write(t.path(), "e.wav", &wav_bytes(16_000, 1, &sine(16_000, 3.0, 200.0, 0.3)));
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        drop(rx);
        let mut src = FileSource { path: p, progress: Box::new(|_| {}) };
        assert!(src.run(&tx, &AtomicBool::new(false)).is_ok());
    }
}
