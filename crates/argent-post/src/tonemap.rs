//! Tone mapping: scene-linear Rec.709 radiance → display-referred sRGB.
//!
//! [`develop`] runs exposure (×2^EV) → tone map → saturation → sRGB OETF →
//! TPDF dither → 8-bit, rayon-parallel over rows, with the transcendental
//! parts baked into per-grade lookup tables (1080×1920 develops in a few ms).
//! [`tonemap_pixel`] and [`develop_f32`] are the exact (table-free)
//! references.
//!
//! Operators:
//! - **AgX** — Troy Sobotka's AgX following Benjamin Wrensch's "Minimal AgX":
//!   inset matrix, log2 encoding over [−12.47393, +4.026069] EV around 0.18,
//!   the 6th-order sigmoid fit, an ASC CDL look, outset matrix, 2.2 display
//!   EOTF. Looks: `Base`; `Punchy` = power 1.35, sat 1.4; `Golden` = slope
//!   (1, 0.9, 0.5), power 0.8, sat 1.3. Unlike Wrensch's shader, the look's
//!   saturation uses the luma *after* slope/offset/power, as the ASC CDL
//!   spec (and OCIO's `CDLTransform`, which the AgX config uses) define it,
//!   and SOP/sat results are clamped to 0..1 like OCIO's default CDL style.
//!   Blender 4+/5 ship EaryChow's LUT-based AgX (Rec.2020 working space);
//!   this analytic AgX is close to it but not bit-identical.
//! - **ACES fitted** — Stephen Hill's RRT+ODT fit with the sRGB→AP1 and
//!   AP1→sRGB matrices. Like real ACES it maps scene 0.18 to ~0.11 display,
//!   about 0.7 EV darker than AgX Base.
//! - **Khronos PBR Neutral** — the reference GLSL (start 0.76, desat 0.15).
//! - **Reinhard** — per channel `x / (1 + x)`.
//! - **Clamp** — clip to 0..1 (linear diagnostics).
//!
//! `Grade::saturation` scales the AgX look's CDL saturation (so it acts in
//! AgX's display-encoded space like Blender's looks); for the other
//! operators it is a luma-preserving (Rec.709) saturation on the
//! display-linear result, clamped to 0..1.
//!
//! NaN inputs develop as black and ±Inf are clamped to 0 / 65504.

use crate::color::{LUMA, Lut1, srgb_encode};
use crate::{Image, Rgb8};
use rayon::prelude::*;

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

// ---------------------------------------------------------------------------
// AgX constants (Minimal AgX; GLSL column-major matrices transposed to rows).

const AGX_INSET: [[f32; 3]; 3] = [
    [0.842_479_06, 0.078_433_6, 0.079_223_745],
    [0.042_328_242, 0.878_468_6, 0.079_166_13],
    [0.042_375_655, 0.078_433_6, 0.879_143],
];
const AGX_OUTSET: [[f32; 3]; 3] = [
    [1.196_879, -0.098_020_88, -0.099_029_74],
    [-0.052_896_85, 1.151_903_1, -0.098_961_18],
    [-0.052_971_635, -0.098_043_45, 1.151_073_7],
];
const AGX_MIN_EV: f32 = -12.473_93;
const AGX_MAX_EV: f32 = 4.026_069;

struct Cdl {
    slope: [f32; 3],
    offset: [f32; 3],
    power: [f32; 3],
    sat: f32,
}

fn look_cdl(look: AgxLook) -> Cdl {
    match look {
        AgxLook::Base => Cdl { slope: [1.0; 3], offset: [0.0; 3], power: [1.0; 3], sat: 1.0 },
        AgxLook::Punchy => Cdl { slope: [1.0; 3], offset: [0.0; 3], power: [1.35; 3], sat: 1.4 },
        AgxLook::Golden => Cdl { slope: [1.0, 0.9, 0.5], offset: [0.0; 3], power: [0.8; 3], sat: 1.3 },
    }
}

#[inline]
fn mat3(m: &[[f32; 3]; 3], v: [f32; 3]) -> [f32; 3] {
    [
        m[0][0] * v[0] + m[0][1] * v[1] + m[0][2] * v[2],
        m[1][0] * v[0] + m[1][1] * v[1] + m[1][2] * v[2],
        m[2][0] * v[0] + m[2][1] * v[1] + m[2][2] * v[2],
    ]
}

#[inline]
fn luma(v: [f32; 3]) -> f32 {
    LUMA[0] * v[0] + LUMA[1] * v[1] + LUMA[2] * v[2]
}

/// Scene value after the inset → normalised log2 position 0..1.
#[inline]
fn agx_log_encode(v: f32) -> f32 {
    let ev = v.max(1e-10).log2().clamp(AGX_MIN_EV, AGX_MAX_EV);
    (ev - AGX_MIN_EV) / (AGX_MAX_EV - AGX_MIN_EV)
}

/// Wrensch's 6th-order fit of AgX's default contrast sigmoid (mean error² 3.7e-6).
#[inline]
fn agx_sigmoid(x: f32) -> f32 {
    ((((((15.5 * x - 40.14) * x + 31.96) * x - 6.868) * x + 0.4298) * x + 0.1191) * x) - 0.002_32
}

/// ASC CDL slope/offset/power on one channel, clamped like OCIO's default style.
#[inline]
fn cdl_sop(x: f32, slope: f32, offset: f32, power: f32) -> f32 {
    let v = (x * slope + offset).clamp(0.0, 1.0);
    if power == 1.0 { v } else { v.powf(power) }
}

#[inline]
fn saturate(v: [f32; 3], sat: f32) -> [f32; 3] {
    if sat == 1.0 {
        return v.map(|c| c.clamp(0.0, 1.0));
    }
    let l = luma(v);
    v.map(|c| (l + sat * (c - l)).clamp(0.0, 1.0))
}

fn agx_exact(x: [f32; 3], look: AgxLook, sat_mul: f32) -> [f32; 3] {
    let cdl = look_cdl(look);
    let v = mat3(&AGX_INSET, x);
    let mut e = [0.0; 3];
    for c in 0..3 {
        e[c] = cdl_sop(agx_sigmoid(agx_log_encode(v[c])), cdl.slope[c], cdl.offset[c], cdl.power[c]);
    }
    let e = saturate(e, cdl.sat * sat_mul);
    mat3(&AGX_OUTSET, e).map(|o| o.max(0.0).powf(2.2).min(1.0))
}

// ---------------------------------------------------------------------------
// Other operators.

const ACES_IN: [[f32; 3]; 3] = [[0.59719, 0.35458, 0.04823], [0.07600, 0.90834, 0.01566], [0.02840, 0.13383, 0.83777]];
const ACES_OUT: [[f32; 3]; 3] =
    [[1.60475, -0.53108, -0.07367], [-0.10208, 1.10813, -0.00605], [-0.00327, -0.07276, 1.07602]];

#[inline]
fn aces_fitted(x: [f32; 3]) -> [f32; 3] {
    let v = mat3(&ACES_IN, x).map(|v| {
        let a = v * (v + 0.024_578_6) - 0.000_090_537;
        let b = v * (0.983_729 * v + 0.432_951) + 0.238_081;
        a / b
    });
    mat3(&ACES_OUT, v).map(|c| c.clamp(0.0, 1.0))
}

#[inline]
fn khronos_neutral(mut c: [f32; 3]) -> [f32; 3] {
    const START: f32 = 0.8 - 0.04;
    const DESAT: f32 = 0.15;
    let x = c[0].min(c[1]).min(c[2]);
    let offset = if x < 0.08 { x - 6.25 * x * x } else { 0.04 };
    c = c.map(|v| v - offset);
    let peak = c[0].max(c[1]).max(c[2]);
    if peak < START {
        return c;
    }
    let d = 1.0 - START;
    let new_peak = 1.0 - d * d / (peak + d - START);
    c = c.map(|v| v * (new_peak / peak));
    let g = 1.0 - 1.0 / (DESAT * (peak - new_peak) + 1.0);
    c.map(|v| v + (new_peak - v) * g)
}

#[inline]
fn sanitize(v: f32) -> f32 {
    if v.is_nan() { 0.0 } else { v.clamp(0.0, 65504.0) }
}

#[inline]
fn exposure_scale(ev: f32) -> f32 {
    if ev.is_finite() { 2f32.powf(ev) } else { 1.0 }
}

/// Exact single-pixel tone map: scene-linear Rec.709 in, display-linear
/// 0..1 out (before the sRGB OETF). Applies exposure, operator and saturation.
pub fn tonemap_pixel(rgb: [f32; 3], grade: &Grade) -> [f32; 3] {
    let k = exposure_scale(grade.exposure_ev);
    let x = rgb.map(|v| sanitize(v * k));
    let sat = if grade.saturation.is_finite() { grade.saturation.max(0.0) } else { 1.0 };
    match grade.tonemap {
        ToneMap::AgX(look) => agx_exact(x, look, sat),
        ToneMap::AcesFitted => saturate(aces_fitted(x), sat),
        ToneMap::KhronosNeutral => saturate(khronos_neutral(x), sat),
        ToneMap::Reinhard => saturate(x.map(|v| v / (1.0 + v)), sat),
        ToneMap::Clamp => saturate(x, sat),
    }
}

/// Exact develop without quantisation: sRGB-encoded display values in 0..1
/// (used for 16-bit output).
pub fn develop_f32(img: &Image, grade: &Grade) -> Vec<[f32; 3]> {
    img.pixels.par_iter().map(|p| tonemap_pixel(*p, grade).map(srgb_encode)).collect()
}

// ---------------------------------------------------------------------------
// Fast path.

const LUT_SEGMENTS: usize = 4096;
/// Largest value AGX_OUTSET can produce from 0..1 inputs is 1.197.
const AGX_OUT_MAX: f32 = 1.25;

/// AgX's per-channel curve (log2 encode → sigmoid → CDL slope/offset/power)
/// tabulated against the f32 bit pattern: the exponent plus the top
/// `CURVE_MANT_BITS` mantissa bits pick a segment and the remaining bits
/// interpolate linearly, which replaces `log2` + a second lookup.
struct BitCurve {
    base: u32,
    t: Vec<f32>,
}

const CURVE_MANT_BITS: u32 = 9;
const CURVE_SHIFT: u32 = 23 - CURVE_MANT_BITS;

impl BitCurve {
    fn new(f: impl Fn(f32) -> f32) -> Self {
        let lo = 2f32.powf(AGX_MIN_EV) * 0.999;
        let hi = 2f32.powf(AGX_MAX_EV) * 1.001;
        let base = lo.to_bits() >> CURVE_SHIFT;
        let top = (hi.to_bits() >> CURVE_SHIFT) + 1;
        let t = (base..=top).map(|k| f(agx_log_encode(f32::from_bits(k << CURVE_SHIFT)))).collect();
        BitCurve { base, t }
    }

    #[inline]
    fn eval(&self, v: f32) -> f32 {
        let lo = f32::from_bits(self.base << CURVE_SHIFT);
        let hi = f32::from_bits((self.base + self.t.len() as u32 - 1) << CURVE_SHIFT);
        // max/min (not clamp) so NaN maps to the low end.
        let bits = v.max(lo).min(hi).to_bits();
        let k = ((bits >> CURVE_SHIFT) - self.base) as usize;
        let f = (bits & ((1 << CURVE_SHIFT) - 1)) as f32 * (1.0 / (1u32 << CURVE_SHIFT) as f32);
        let k = k.min(self.t.len() - 2);
        self.t[k] * (1.0 - f) + self.t[k + 1] * f
    }
}

enum Fast {
    Agx {
        /// Inset value → sigmoid → CDL slope/offset/power; one table when all
        /// channels share the CDL (Base, Punchy), else one per channel.
        curves: Vec<BitCurve>,
        pick: [usize; 3],
        sat: f32,
        /// Outset value → sRGB-encoded display value (2.2 EOTF, clip, OETF).
        encode: Lut1,
    },
    Other {
        op: ToneMap,
        sat: f32,
        encode: Lut1,
    },
}

struct Prepared {
    k: f32,
    fast: Fast,
}

impl Prepared {
    fn new(grade: &Grade) -> Self {
        let k = exposure_scale(grade.exposure_ev);
        let sat = if grade.saturation.is_finite() { grade.saturation.max(0.0) } else { 1.0 };
        let fast = match grade.tonemap {
            ToneMap::AgX(look) => {
                let cdl = look_cdl(look);
                let shared = (0..3).all(|c| {
                    (cdl.slope[c], cdl.offset[c], cdl.power[c]) == (cdl.slope[0], cdl.offset[0], cdl.power[0])
                });
                let n = if shared { 1 } else { 3 };
                let curves = (0..n)
                    .map(|c| {
                        let (s, o, p) = (cdl.slope[c], cdl.offset[c], cdl.power[c]);
                        BitCurve::new(move |t| cdl_sop(agx_sigmoid(t), s, o, p))
                    })
                    .collect();
                let pick = if shared { [0, 0, 0] } else { [0, 1, 2] };
                let encode = Lut1::new(0.0, AGX_OUT_MAX, LUT_SEGMENTS, |o| srgb_encode(o.max(0.0).powf(2.2).min(1.0)));
                Fast::Agx { curves, pick, sat: cdl.sat * sat, encode }
            }
            op => Fast::Other { op, sat, encode: Lut1::new(0.0, 1.0, LUT_SEGMENTS, srgb_encode) },
        };
        Prepared { k, fast }
    }

    /// Scene-linear pixel → sRGB-encoded display value 0..1.
    #[inline]
    fn encoded(&self, p: [f32; 3]) -> [f32; 3] {
        let x = p.map(|v| sanitize(v * self.k));
        match &self.fast {
            Fast::Agx { curves, pick, sat, encode } => {
                let v = mat3(&AGX_INSET, x);
                let e = [curves[pick[0]].eval(v[0]), curves[pick[1]].eval(v[1]), curves[pick[2]].eval(v[2])];
                let e = if *sat == 1.0 { e } else { saturate(e, *sat) };
                mat3(&AGX_OUTSET, e).map(|o| encode.eval(o))
            }
            Fast::Other { op, sat, encode } => {
                let d = match op {
                    ToneMap::AcesFitted => aces_fitted(x),
                    ToneMap::KhronosNeutral => khronos_neutral(x),
                    ToneMap::Reinhard => x.map(|v| v / (1.0 + v)),
                    _ => x,
                };
                saturate(d, *sat).map(|v| encode.eval(v))
            }
        }
    }
}

/// splitmix64 finaliser: a cheap bijective 64-bit hash.
#[inline]
fn hash64(mut x: u64) -> u64 {
    x ^= x >> 30;
    x = x.wrapping_mul(0xbf58_476d_1ce4_e5b9);
    x ^= x >> 27;
    x = x.wrapping_mul(0x94d0_49bb_1331_11eb);
    x ^ (x >> 31)
}

/// Three triangular-PDF dither values in (−1, 1) LSB for pixel `i`, each the
/// sum of two 10-bit uniforms from one hash (deterministic, so stills and
/// video frames develop identically run to run).
#[inline]
fn tpdf3(i: u64) -> [f32; 3] {
    let h = hash64(i ^ 0x9e37_79b9_7f4a_7c15);
    let u = |k: u32| ((h >> (k * 10)) & 0x3ff) as f32;
    const S: f32 = 1.0 / 1024.0;
    [(u(0) + u(1) + 1.0) * S - 1.0, (u(2) + u(3) + 1.0) * S - 1.0, (u(4) + u(5) + 1.0) * S - 1.0]
}

/// Encoded 0..1 → 8-bit with dither `n`. Exact black and white stay clean.
#[inline]
fn quantize8(v: f32, n: f32) -> u8 {
    if v <= 0.0 {
        0
    } else if v >= 1.0 {
        255
    } else {
        // Truncation equals floor here (the sum is > −1.5 and clamped), and
        // avoids a libm `floorf` call on baseline x86-64.
        ((v * 255.0 + 0.5 + n) as i32).clamp(0, 255) as u8
    }
}

/// Exposure → tone map → sRGB encode → 8-bit.
pub fn develop(img: &Image, grade: &Grade) -> Rgb8 {
    let mut out = Rgb8::new(img.width, img.height);
    let w = img.width as usize;
    if w == 0 || img.height == 0 {
        return out;
    }
    let prep = Prepared::new(grade);
    out.data.par_chunks_mut(w * 3).zip(img.pixels.par_chunks(w)).enumerate().for_each(|(y, (dst, src))| {
        let base = (y * w) as u64;
        for ((x, p), d) in src.iter().enumerate().zip(dst.chunks_exact_mut(3)) {
            let s = prep.encoded(*p);
            let n = tpdf3(base + x as u64);
            d[0] = quantize8(s[0], n[0]);
            d[1] = quantize8(s[1], n[1]);
            d[2] = quantize8(s[2], n[2]);
        }
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color::srgb_encode;

    fn g(tonemap: ToneMap) -> Grade {
        Grade { exposure_ev: 0.0, tonemap, saturation: 1.0 }
    }

    #[test]
    fn agx_black_and_mid_grey() {
        let base = g(ToneMap::AgX(AgxLook::Base));
        let black = tonemap_pixel([0.0; 3], &base);
        assert!(black.iter().all(|&c| c < 1e-4), "AgX(0) = {black:?}");
        let grey = tonemap_pixel([0.18; 3], &base);
        let enc = srgb_encode(grey[1]);
        assert!((0.45..0.55).contains(&enc), "AgX Base 0.18 → sRGB {enc}");
        assert!((grey[0] - grey[2]).abs() < 2e-3, "grey stays neutral: {grey:?}");
        // Punchy darkens the mid-tones, Golden warms them.
        let punchy = tonemap_pixel([0.18; 3], &g(ToneMap::AgX(AgxLook::Punchy)));
        assert!(punchy[1] < grey[1], "punchy {punchy:?} vs base {grey:?}");
        let golden = tonemap_pixel([0.18; 3], &g(ToneMap::AgX(AgxLook::Golden)));
        assert!(golden[0] > golden[1] && golden[1] > golden[2], "golden is warm: {golden:?}");
    }

    /// HSV hue in degrees (None for neutrals).
    fn hue(o: [f32; 3]) -> Option<f32> {
        let (mx, mn) = (o[0].max(o[1]).max(o[2]), o[0].min(o[1]).min(o[2]));
        if mx - mn < 1e-3 {
            return None;
        }
        let d = mx - mn;
        let h = if mx == o[0] {
            60.0 * ((o[1] - o[2]) / d).rem_euclid(6.0)
        } else if mx == o[1] {
            60.0 * ((o[2] - o[0]) / d + 2.0)
        } else {
            60.0 * ((o[0] - o[1]) / d + 4.0)
        };
        Some(h)
    }

    #[test]
    fn agx_rolls_off_without_hue_clipping() {
        let base = g(ToneMap::AgX(AgxLook::Base));
        let clamp = g(ToneMap::Clamp);
        let mut prev_luma = -1.0;
        let mut sats = Vec::new();
        for e in -4..=14 {
            let k = 2f32.powi(e);
            let x = [1.0 * k, 0.5 * k, 0.1 * k];
            let o = tonemap_pixel(x, &base);
            let c = tonemap_pixel(x, &clamp);
            let sat = (o[0] - o[2]) / o[0].max(1e-6);
            eprintln!("2^{e:>3}: agx {o:.4?} hue {:?} sat {sat:.3} | clamp hue {:?}", hue(o), hue(c));
            assert!(o.iter().all(|c| c.is_finite() && (0.0..=1.0).contains(c)));
            assert!(o[0] >= o[1] - 2e-3 && o[1] >= o[2] - 2e-3, "hue order kept at 2^{e}: {o:?}");
            let l = luma(o);
            assert!(l >= prev_luma - 1e-5, "monotonic at 2^{e}");
            prev_luma = l;
            if let Some(h) = hue(o) {
                // Orange may warm towards yellow as it brightens (as AgX does), but never
                // flips hue the way per-channel clipping does on the way to white.
                assert!((15.0..=62.0).contains(&h), "hue {h} at 2^{e}: {o:?}");
            }
            sats.push(sat);
        }
        // Past the shoulder, saturation falls steadily towards white...
        for w in sats[8..].windows(2) {
            assert!(w[1] <= w[0] + 1e-3, "saturation must fall monotonically: {sats:?}");
        }
        assert!(*sats.last().unwrap() < 0.1, "{sats:?}");
        // ...while plain clipping is already flat white a few stops above 1.
        let clipped = tonemap_pixel([16.0, 8.0, 1.6], &clamp);
        assert_eq!(clipped, [1.0; 3]);
        let agx = tonemap_pixel([16.0, 8.0, 1.6], &base);
        assert!(agx[0] - agx[2] > 0.15, "AgX keeps the colour of a 16x-bright orange: {agx:?}");
        let huge = tonemap_pixel([1e6, 2e5, 1e4], &base);
        assert!(huge.iter().all(|&c| c > 0.9 && c <= 1.0), "{huge:?}");
    }

    #[test]
    fn other_operators_are_sane() {
        for op in [ToneMap::AcesFitted, ToneMap::KhronosNeutral, ToneMap::Reinhard, ToneMap::Clamp] {
            let gr = g(op);
            let z = tonemap_pixel([0.0; 3], &gr);
            assert!(z.iter().all(|&c| c.abs() < 1e-3), "{op:?}(0) = {z:?}");
            let hi = tonemap_pixel([50.0, 40.0, 30.0], &gr);
            assert!(hi.iter().all(|&c| (0.0..=1.0).contains(&c)), "{op:?} {hi:?}");
            let nan = tonemap_pixel([f32::NAN, f32::INFINITY, -1.0], &gr);
            assert!(nan.iter().all(|c| c.is_finite()), "{op:?} {nan:?}");
        }
        // Hill's fit: 0.18 → ~0.106 display (darker than AgX, as real ACES).
        let aces = tonemap_pixel([0.18; 3], &g(ToneMap::AcesFitted));
        assert!((aces[1] - 0.106).abs() < 0.01, "{aces:?}");
        // Khronos Neutral is identity-ish below the compression start.
        let kn = tonemap_pixel([0.5, 0.3, 0.2], &g(ToneMap::KhronosNeutral));
        assert!((kn[0] - 0.46).abs() < 1e-3 && (kn[2] - 0.16).abs() < 1e-3, "{kn:?}");
    }

    #[test]
    fn saturation_and_exposure() {
        let gr = Grade { exposure_ev: 1.0, tonemap: ToneMap::Reinhard, saturation: 1.0 };
        assert!((tonemap_pixel([1.0; 3], &gr)[0] - 2.0 / 3.0).abs() < 1e-5);
        let grey = Grade { saturation: 0.0, ..g(ToneMap::AgX(AgxLook::Base)) };
        let o = tonemap_pixel([1.0, 0.2, 0.05], &grey);
        assert!((o[0] - o[1]).abs() < 2e-3 && (o[1] - o[2]).abs() < 2e-3, "{o:?}");
    }

    fn test_image(w: u32, h: u32) -> Image {
        let mut img = Image::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let t = x as f32 / (w - 1) as f32;
                let ev = -10.0 + 22.0 * t;
                let c = 0.18 * 2f32.powf(ev);
                let hue = y as f32 / h as f32;
                img.set(x, y, [c * (1.0 - hue), c * 0.6, c * hue]);
            }
        }
        img
    }

    #[test]
    fn fast_path_matches_exact() {
        let img = test_image(512, 64);
        for tm in [
            ToneMap::AgX(AgxLook::Base),
            ToneMap::AgX(AgxLook::Punchy),
            ToneMap::AgX(AgxLook::Golden),
            ToneMap::AcesFitted,
            ToneMap::KhronosNeutral,
            ToneMap::Reinhard,
            ToneMap::Clamp,
        ] {
            let gr = Grade { exposure_ev: 0.5, tonemap: tm, saturation: 1.2 };
            let prep = Prepared::new(&gr);
            let mut worst = 0.0f32;
            for p in &img.pixels {
                let fast = prep.encoded(*p);
                let exact = tonemap_pixel(*p, &gr).map(srgb_encode);
                for c in 0..3 {
                    worst = worst.max((fast[c] - exact[c]).abs());
                }
            }
            assert!(worst < 2e-4, "{tm:?}: fast vs exact differs by {worst}");
        }
    }

    #[test]
    fn develop_dithers_and_keeps_extremes() {
        let mut img = Image::new(256, 4);
        for x in 0..256 {
            img.set(x, 0, [0.0; 3]);
            img.set(x, 1, [1e9; 3]);
            img.set(x, 2, [0.18; 3]);
        }
        let out = develop(&img, &g(ToneMap::Clamp));
        assert!(out.data[..256 * 3].iter().all(|&v| v == 0), "black stays 0");
        assert!(out.data[256 * 3..512 * 3].iter().all(|&v| v == 255), "white stays 255");
        // 0.18 linear = 117.65 sRGB codes → dithered mix of neighbours, mean preserved.
        let row = &out.data[512 * 3..768 * 3];
        let mean = row.iter().map(|&v| v as f32).sum::<f32>() / row.len() as f32;
        let want = srgb_encode(0.18) * 255.0;
        assert!((mean - want).abs() < 0.15, "dithered mean {mean} vs {want}");
        assert!(row.iter().any(|&v| v != row[0]), "dither present");
    }

    /// Per-operator single-thread CPU cost (`cargo test -- --ignored develop_cost`).
    #[test]
    #[ignore]
    fn develop_cost_per_operator() {
        let img = test_image(1080, 1920);
        let pool = rayon::ThreadPoolBuilder::new().num_threads(1).build().unwrap();
        let cpu_ns = || {
            std::fs::read_to_string("/proc/thread-self/schedstat")
                .ok()
                .and_then(|s| s.split_whitespace().next().and_then(|v| v.parse::<f64>().ok()))
                .unwrap_or(0.0)
        };
        for tm in [ToneMap::Clamp, ToneMap::Reinhard, ToneMap::AcesFitted, ToneMap::AgX(AgxLook::Base), ToneMap::AgX(AgxLook::Golden)] {
            let gr = Grade { exposure_ev: 0.0, tonemap: tm, saturation: 1.0 };
            let ns = pool.install(|| {
                let _ = develop(&img, &gr);
                (0..7)
                    .map(|_| {
                        let c0 = cpu_ns();
                        std::hint::black_box(develop(&img, &gr));
                        (cpu_ns() - c0) / img.pixels.len() as f64
                    })
                    .fold(f64::INFINITY, f64::min)
            });
            eprintln!("{tm:?}: {ns:.1} ns/px (best of 7)");
        }
    }

    #[test]
    fn develop_speed_1080x1920() {
        let img = test_image(1080, 1920);
        let gr = Grade::default();
        let _ = develop(&img, &gr); // warm the pool
        let t = std::time::Instant::now();
        let n = 5;
        for _ in 0..n {
            std::hint::black_box(develop(&img, &gr));
        }
        let ms = t.elapsed().as_secs_f64() * 1e3 / n as f64;
        // Single-thread CPU time per pixel (immune to other load on the box).
        let cpu_ns = || {
            std::fs::read_to_string("/proc/thread-self/schedstat")
                .ok()
                .and_then(|s| s.split_whitespace().next().and_then(|v| v.parse::<f64>().ok()))
        };
        let pool = rayon::ThreadPoolBuilder::new().num_threads(1).build().unwrap();
        let ns_px = pool.install(|| {
            let _ = develop(&img, &gr);
            let (t0, c0) = (std::time::Instant::now(), cpu_ns());
            std::hint::black_box(develop(&img, &gr));
            let wall = t0.elapsed().as_secs_f64() * 1e9;
            c0.zip(cpu_ns()).map(|(a, b)| b - a).unwrap_or(wall) / img.pixels.len() as f64
        });
        eprintln!(
            "develop 1080x1920 AgX Punchy: {ms:.2} ms wall on {} threads; {ns_px:.1} ns/px single-thread CPU",
            rayon::current_num_threads()
        );
        assert!(ms < 250.0, "develop too slow: {ms} ms");
    }
}
