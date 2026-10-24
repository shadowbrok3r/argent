//! sRGB transfer functions and small lookup tables shared by tone mapping,
//! text compositing and resampling.

use std::sync::OnceLock;

/// Rec.709 / sRGB luminance weights (linear light).
pub(crate) const LUMA: [f32; 3] = [0.2126, 0.7152, 0.0722];

/// sRGB OETF (IEC 61966-2-1): display-linear 0..1 → encoded 0..1.
#[inline]
pub fn srgb_encode(x: f32) -> f32 {
    if x <= 0.003_130_8 {
        12.92 * x.max(0.0)
    } else if x >= 1.0 {
        1.0
    } else {
        1.055 * x.powf(1.0 / 2.4) - 0.055
    }
}

/// Inverse of [`srgb_encode`]: encoded 0..1 → display-linear 0..1.
#[inline]
pub fn srgb_decode(v: f32) -> f32 {
    if v <= 0.040_45 {
        v.max(0.0) / 12.92
    } else if v >= 1.0 {
        1.0
    } else {
        ((v + 0.055) / 1.055).powf(2.4)
    }
}

/// Piecewise-linear table of `f` over `[lo, hi]`; inputs outside are clamped.
pub(crate) struct Lut1 {
    lo: f32,
    scale: f32,
    last: usize,
    t: Vec<f32>,
}

impl Lut1 {
    pub(crate) fn new(lo: f32, hi: f32, segments: usize, f: impl Fn(f32) -> f32) -> Self {
        let n = segments.max(2);
        let t = (0..=n).map(|i| f(lo + (hi - lo) * i as f32 / n as f32)).collect();
        Lut1 { lo, scale: n as f32 / (hi - lo), last: n, t }
    }

    #[inline]
    pub(crate) fn eval(&self, x: f32) -> f32 {
        // Written so NaN (which fails `>=`) maps to f(lo).
        let u = (x - self.lo) * self.scale;
        let u = if u >= 0.0 { u.min(self.last as f32) } else { 0.0 };
        let i = (u as usize).min(self.last - 1);
        let f = u - i as f32;
        // Lerp form exact at both ends (table end points are hit exactly).
        self.t[i] * (1.0 - f) + self.t[i + 1] * f
    }
}

/// sRGB 8-bit code value → linear light.
pub(crate) fn decode_lut() -> &'static [f32; 256] {
    static LUT: OnceLock<[f32; 256]> = OnceLock::new();
    LUT.get_or_init(|| std::array::from_fn(|i| srgb_decode(i as f32 / 255.0)))
}

/// Linear light 0..1 → sRGB encoded 0..1 (4096 segments, error < 2e-5).
pub(crate) fn encode_lut() -> &'static Lut1 {
    static LUT: OnceLock<Lut1> = OnceLock::new();
    LUT.get_or_init(|| Lut1::new(0.0, 1.0, 4096, srgb_encode))
}

/// Linear light → nearest sRGB 8-bit code value.
#[inline]
pub(crate) fn encode_u8(l: f32) -> u8 {
    (encode_lut().eval(l) * 255.0 + 0.5) as u8
}
