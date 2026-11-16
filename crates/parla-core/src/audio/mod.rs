//! The single capture seam: every source (microphone, system loopback, imported file) produces
//! 16 kHz mono `Frame`s. Nothing downstream knows or cares where audio came from.
pub mod file;
pub mod resample;

#[cfg(feature = "device-capture")]
pub mod device;

use crate::types::Frame;
use anyhow::Result;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::SyncSender;

pub use file::{decode_file, FileSource};
pub use resample::MonoResampler;

/// ~100 ms of audio at 16 kHz.
pub const FRAME_SAMPLES: usize = 1600;

pub trait AudioSource: Send {
    /// Blocks, sending frames until the source ends or `stop` is set. Call from a worker thread.
    fn run(&mut self, tx: &SyncSender<Frame>, stop: &AtomicBool) -> Result<()>;
}
