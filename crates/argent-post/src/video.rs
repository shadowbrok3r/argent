//! ffmpeg video writer. TODO(agent).

use crate::Rgb8;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub enum Audio {
    /// No audio stream.
    None,
    /// Silent AAC 48 kHz stereo track (Forge's QC requires an audio stream).
    Silent,
    /// Mux this file, trimmed to the video.
    File(PathBuf),
}

#[derive(Clone, Debug)]
pub struct VideoSpec {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    /// x264 CRF (lower = better), default 18.
    pub crf: u8,
    /// Cap in kbit/s (Forge requires < 25 Mbps), default 16000.
    pub max_kbps: u32,
    pub audio: Audio,
}

impl Default for VideoSpec {
    fn default() -> Self {
        VideoSpec { width: 1080, height: 1920, fps: 30, crf: 18, max_kbps: 16_000, audio: Audio::Silent }
    }
}

pub struct VideoWriter {
    _private: (),
}

impl VideoWriter {
    pub fn create(_path: &Path, _spec: &VideoSpec) -> anyhow::Result<Self> {
        anyhow::bail!("video writer not implemented yet")
    }
    pub fn push(&mut self, _frame: &Rgb8) -> anyhow::Result<()> {
        anyhow::bail!("video writer not implemented yet")
    }
    pub fn finish(self) -> anyhow::Result<()> {
        Ok(())
    }
}
