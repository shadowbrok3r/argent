//! Denoising. TODO(agent): Intel OIDN 2.x via runtime `dlopen` of
//! libOpenImageDenoise.so (search ARGENT_OIDN_DIR, ~/.local/opt/oidn*/lib,
//! LD_LIBRARY_PATH) using the C API (oidnNewDevice/oidnNewFilter "RT",
//! hdr=true, albedo+normal auxiliary, cleanAux), and a built-in edge-avoiding
//! à-trous wavelet fallback guided by albedo/normal.

use crate::Image;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DenoiseBackend {
    /// OIDN when it loads, else à-trous.
    Auto,
    Oidn,
    Atrous,
    None,
}

pub struct DenoiseInput<'a> {
    pub color: &'a Image,
    /// First-hit albedo in 0..1 (same size), improves edge preservation.
    pub albedo: Option<&'a Image>,
    /// First-hit shading normal in −1..1 (same size).
    pub normal: Option<&'a Image>,
}

pub fn oidn_available() -> bool {
    false
}

pub fn denoise(input: DenoiseInput, backend: DenoiseBackend) -> anyhow::Result<Image> {
    match backend {
        DenoiseBackend::Oidn => anyhow::bail!("OIDN not available"),
        _ => Ok(input.color.clone()),
    }
}
