//! Tone mapping. TODO(agent): AgX (Troy Sobotka's, as shipped in Blender 4+,
//! with the Base/Punchy/Golden looks matching Blender's), ACES fitted
//! (Narkowicz/Hill), Khronos PBR Neutral, Reinhard; sRGB OETF; TPDF dither.

use crate::{Image, Rgb8};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgxLook {
    Base,
    Punchy,
    Golden,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ToneMap {
    AgX(AgxLook),
    AcesFitted,
    KhronosNeutral,
    Reinhard,
    /// Clip only (for linear diagnostics).
    Clamp,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Grade {
    pub exposure_ev: f32,
    pub tonemap: ToneMap,
    /// 1.0 = unchanged.
    pub saturation: f32,
}

impl Default for Grade {
    fn default() -> Self {
        Grade { exposure_ev: 0.0, tonemap: ToneMap::AgX(AgxLook::Punchy), saturation: 1.0 }
    }
}

/// Exposure → tone map → sRGB encode → 8-bit.
pub fn develop(img: &Image, grade: &Grade) -> Rgb8 {
    let mut out = Rgb8::new(img.width, img.height);
    let k = 2f32.powf(grade.exposure_ev);
    for (i, p) in img.pixels.iter().enumerate() {
        for c in 0..3 {
            let v = (p[c] * k).max(0.0);
            let v = v / (1.0 + v);
            let s = if v <= 0.003_130_8 { v * 12.92 } else { 1.055 * v.powf(1.0 / 2.4) - 0.055 };
            out.data[i * 3 + c] = (s * 255.0 + 0.5).clamp(0.0, 255.0) as u8;
        }
    }
    out
}
