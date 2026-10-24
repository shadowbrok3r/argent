//! Forge reel QC. TODO(agent): ffprobe/ffmpeg-based checks, same names and
//! thresholds as Forge (see agent brief).

use std::path::Path;

#[derive(Clone, Debug)]
pub struct Check {
    pub check: String,
    pub ok: bool,
    pub detail: String,
}

#[derive(Clone, Debug)]
pub struct QcReport {
    pub passed: bool,
    pub checks: Vec<Check>,
}

pub fn forge_reel(_path: &Path) -> anyhow::Result<QcReport> {
    anyhow::bail!("qc not implemented yet")
}
