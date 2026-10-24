//! Continuous spectra: the [`Spectrum`] trait and its basic implementations.

use crate::cie;
use crate::spec::{Spec, Wavelengths, N};
use std::sync::OnceLock;

/// A function of wavelength (nm).
pub trait Spectrum: Send + Sync {
    fn eval(&self, lambda: f32) -> f32;

    #[inline]
    fn sample(&self, wl: &Wavelengths) -> Spec {
        let mut r = [0.0; N];
        for i in 0..N {
            r[i] = self.eval(wl.lambda[i]);
        }
        Spec(r)
    }
}

impl<T: Spectrum + ?Sized> Spectrum for Box<T> {
    #[inline]
    fn eval(&self, lambda: f32) -> f32 {
        (**self).eval(lambda)
    }
}
impl<T: Spectrum + ?Sized> Spectrum for std::sync::Arc<T> {
    #[inline]
    fn eval(&self, lambda: f32) -> f32 {
        (**self).eval(lambda)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Constant(pub f32);
impl Spectrum for Constant {
    #[inline]
    fn eval(&self, _: f32) -> f32 {
        self.0
    }
}

/// Linear interpolation between `(nm, value)` samples, clamped at the ends.
#[derive(Clone, Debug)]
pub struct Piecewise {
    lambda: Vec<f32>,
    value: Vec<f32>,
}

impl Piecewise {
    /// `pairs` must be sorted by wavelength.
    pub fn new(pairs: &[(f32, f32)]) -> Self {
        assert!(!pairs.is_empty());
        debug_assert!(pairs.windows(2).all(|w| w[0].0 < w[1].0), "wavelengths must ascend");
        Piecewise {
            lambda: pairs.iter().map(|p| p.0).collect(),
            value: pairs.iter().map(|p| p.1).collect(),
        }
    }
    pub fn from_fn(lo: f32, hi: f32, step: f32, f: impl Fn(f32) -> f32) -> Self {
        let mut pairs = Vec::new();
        let mut l = lo;
        while l <= hi + 1e-3 {
            pairs.push((l, f(l)));
            l += step;
        }
        Piecewise::new(&pairs)
    }
    pub fn scaled(mut self, s: f32) -> Self {
        for v in &mut self.value {
            *v *= s;
        }
        self
    }
    pub fn samples(&self) -> impl Iterator<Item = (f32, f32)> + '_ {
        self.lambda.iter().copied().zip(self.value.iter().copied())
    }
}

impl Spectrum for Piecewise {
    fn eval(&self, l: f32) -> f32 {
        let n = self.lambda.len();
        if l <= self.lambda[0] {
            return self.value[0];
        }
        if l >= self.lambda[n - 1] {
            return self.value[n - 1];
        }
        let i = self.lambda.partition_point(|&x| x <= l) - 1;
        let t = (l - self.lambda[i]) / (self.lambda[i + 1] - self.lambda[i]);
        self.value[i] * (1.0 - t) + self.value[i + 1] * t
    }
}

/// Planck emitter. [`Blackbody::new`] normalizes the peak to 1;
/// [`Blackbody::with_luminance`] and [`blackbody_normalized`] normalize the
/// CIE luminance instead, which is what light intensities should mean.
#[derive(Clone, Copy, Debug)]
pub struct Blackbody {
    pub kelvin: f32,
    norm: f32,
}

impl Blackbody {
    pub fn new(kelvin: f32) -> Self {
        let peak = 2.897_772e-3 / kelvin * 1e9; // Wien, nm
        Blackbody { kelvin, norm: 1.0 / planck(peak, kelvin) }
    }

    /// Planck emitter scaled so that `luminance_y` of it equals `y`.
    pub fn with_luminance(kelvin: f32, y: f32) -> Self {
        let raw = Blackbody { kelvin, norm: 1.0 };
        let l = luminance_y(&raw);
        Blackbody { kelvin, norm: if l > 0.0 && l.is_finite() { y / l } else { 0.0 } }
    }
}

/// Planck emitter at `kelvin` with CIE luminance Y = 1, so a light of
/// intensity `i` built from it has luminance `i` whatever its colour
/// temperature (the same convention as [`RgbIlluminant`]).
pub fn blackbody_normalized(kelvin: f32) -> Blackbody {
    Blackbody::with_luminance(kelvin, 1.0)
}

/// Planck's law, wavelength in nm, arbitrary scale.
pub fn planck(lambda_nm: f32, t: f32) -> f32 {
    if t <= 0.0 {
        return 0.0;
    }
    let c = 299_792_458.0f64;
    let h = 6.626_069_57e-34f64;
    let kb = 1.380_648_8e-23f64;
    let l = lambda_nm as f64 * 1e-9;
    let le = (2.0 * h * c * c) / (l.powi(5) * (((h * c) / (l * kb * t as f64)).exp() - 1.0));
    le as f32
}

impl Spectrum for Blackbody {
    #[inline]
    fn eval(&self, l: f32) -> f32 {
        planck(l, self.kelvin) * self.norm
    }
}

/// Wavelength normalization of the sigmoid polynomial: `t = (λ − 360) / 470`.
const SIG_L0: f32 = 360.0;
const SIG_DL: f32 = 470.0;

/// Jakob & Hanika's sigmoid, `S(x) = ½ + x / (2√(1+x²))`: smooth, monotone,
/// bounded in (0, 1), and cheap (no `exp`).
#[inline]
fn sigmoid(x: f32) -> f32 {
    if x.abs() > 1e8 {
        // x/√(1+x²) is ±1 to within 1e-16 here; avoid x² overflowing.
        return if x > 0.0 { 1.0 } else { 0.0 };
    }
    0.5 + 0.5 * x / (1.0 + x * x).sqrt()
}

/// A reflectance in `[0, 1]` given as linear sRGB, upsampled to a smooth
/// spectrum with Jakob & Hanika's (2019) sigmoid-polynomial model:
///
/// `s(λ) = S(c0 λ² + c1 λ + c2)`,  `S(x) = ½ + x / (2√(1+x²))`.
///
/// The three coefficients are solved per colour in [`RgbAlbedo::new`] so that
/// the spectrum, lit by D65 and integrated against the CIE 1931 observer at
/// 1 nm, maps back to the input linear sRGB: Newton (Gauss–Newton on the
/// square 3×3 system, analytic Jacobian, backtracking line search) in the
/// normalized wavelength `t = (λ − 360)/470`, started from the flat grey of
/// the colour's mean and walked to the target in eight continuation steps on
/// 5 nm-binned weights, then polished on the exact 1 nm weights (Jakob &
/// Hanika propagate neighbouring solutions through a table for the same
/// reason: Newton needs a nearby start for saturated colours). Round-trip
/// error is ~1e-6 over the whole sRGB cube, primaries included (worst
/// 1.1e-6 on a 21³ grid). A solve costs 40–70 µs, so build albedos once per
/// material (argent-core bakes them to tables anyway), not per sample.
///
/// Greys, including 0 and 1, are stored as exact constants: the sigmoid only
/// approaches 0 and 1 asymptotically, and a flat grey must stay flat.
/// Inputs are clamped to `[0, 1]`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RgbAlbedo {
    shape: Shape,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Shape {
    Constant(f32),
    /// x(t) = (c0 t + c1) t + c2 in the normalized wavelength t.
    Sigmoid([f32; 3]),
}

impl RgbAlbedo {
    pub fn new(rgb: [f32; 3]) -> Self {
        let rgb = rgb.map(|v| if v.is_nan() { 0.0 } else { v.clamp(0.0, 1.0) });
        if rgb[0] == rgb[1] && rgb[1] == rgb[2] {
            return RgbAlbedo { shape: Shape::Constant(rgb[0]) };
        }
        match solve_sigmoid(rgb) {
            Some(c) => RgbAlbedo { shape: Shape::Sigmoid(c) },
            None => RgbAlbedo { shape: Shape::Constant((rgb[0] + rgb[1] + rgb[2]) / 3.0) },
        }
    }

    /// `[c0, c1, c2]` of `s(λ) = S(c0 λ² + c1 λ + c2)` with λ in nm, or `None`
    /// for an exact constant (grey).
    pub fn coefficients(&self) -> Option<[f32; 3]> {
        match self.shape {
            Shape::Constant(_) => None,
            Shape::Sigmoid([c0, c1, c2]) => {
                let (c0, c1, c2) = (c0 as f64, c1 as f64, c2 as f64);
                let (l0, dl) = (SIG_L0 as f64, SIG_DL as f64);
                let a = c0 / (dl * dl);
                let b = c1 / dl - 2.0 * a * l0;
                let c = a * l0 * l0 - c1 * l0 / dl + c2;
                Some([a as f32, b as f32, c as f32])
            }
        }
    }
}

impl Spectrum for RgbAlbedo {
    #[inline]
    fn eval(&self, l: f32) -> f32 {
        match self.shape {
            Shape::Constant(v) => v,
            Shape::Sigmoid([c0, c1, c2]) => {
                let t = (l - SIG_L0) * (1.0 / SIG_DL);
                sigmoid((c0 * t + c1) * t + c2)
            }
        }
    }
}

/// Quadrature for the sigmoid fit: normalized wavelengths `t` and the
/// linear-sRGB weights of a reflectance lit by D65 (rgb = Σ r(t)·w).
struct Quadrature {
    t: Vec<f64>,
    w: Vec<[f64; 3]>,
}

/// The exact quadrature: every nanometre 360..=830, the same sum as
/// [`cie::reflectance_srgb`] (r ≡ 1 gives (1, 1, 1) to 1e-4).
fn fine_quadrature() -> &'static Quadrature {
    static Q: OnceLock<Quadrature> = OnceLock::new();
    Q.get_or_init(|| {
        const M: [[f64; 3]; 3] = [
            [3.240_454_2, -1.537_138_5, -0.498_531_4],
            [-0.969_266, 1.876_010_8, 0.041_556],
            [0.055_643_4, -0.204_025_9, 1.057_225_2],
        ];
        let xyz = cie::d65_cmf_weights();
        let w = xyz.iter().map(|v| [0, 1, 2].map(|k| M[k][0] * v[0] + M[k][1] * v[1] + M[k][2] * v[2])).collect();
        let t = (0..xyz.len()).map(|j| j as f64 / SIG_DL as f64).collect();
        Quadrature { t, w }
    })
}

/// The same weights summed into 5 nm bins (evaluated at the bin centres): a
/// fifth of the work, accurate to ~1e-3, used to walk the continuation before
/// the final polish on [`fine_quadrature`].
fn coarse_quadrature() -> &'static Quadrature {
    static Q: OnceLock<Quadrature> = OnceLock::new();
    Q.get_or_init(|| {
        let f = fine_quadrature();
        let bins = (f.t.len() - 1) / 5 + 1;
        let mut w = vec![[0.0f64; 3]; bins];
        for (i, wi) in f.w.iter().enumerate() {
            let b = ((i + 2) / 5).min(bins - 1);
            for k in 0..3 {
                w[b][k] += wi[k];
            }
        }
        let t = (0..bins).map(|b| (5 * b) as f64 / SIG_DL as f64).collect();
        Quadrature { t, w }
    })
}

/// rgb of the sigmoid spectrum with normalized coefficients `c`.
fn sigmoid_rgb(c: &[f64; 3], q: &Quadrature) -> [f64; 3] {
    let mut acc = [0.0f64; 3];
    for (&t, wj) in q.t.iter().zip(q.w.iter()) {
        let x = (c[0] * t + c[1]) * t + c[2];
        let s = 0.5 + 0.5 * x / (1.0 + x * x).sqrt();
        for k in 0..3 {
            acc[k] += s * wj[k];
        }
    }
    acc
}

/// Solve the 3×3 system `a x = b` by Gaussian elimination with partial pivoting.
fn solve3(mut a: [[f64; 3]; 3], mut b: [f64; 3]) -> Option<[f64; 3]> {
    for col in 0..3 {
        let piv = (col..3).max_by(|&i, &j| a[i][col].abs().total_cmp(&a[j][col].abs()))?;
        if a[piv][col].abs() < 1e-300 {
            return None;
        }
        a.swap(col, piv);
        b.swap(col, piv);
        for r in col + 1..3 {
            let f = a[r][col] / a[col][col];
            let pivot_row = a[col];
            for (ark, pk) in a[r][col..].iter_mut().zip(&pivot_row[col..]) {
                *ark -= f * pk;
            }
            b[r] -= f * b[col];
        }
    }
    let mut x = [0.0f64; 3];
    for r in (0..3).rev() {
        let s: f64 = (r + 1..3).map(|k| a[r][k] * x[k]).sum();
        x[r] = (b[r] - s) / a[r][r];
    }
    x.iter().all(|v| v.is_finite()).then_some(x)
}

/// Newton on `rgb(c) = goal` with a backtracking line search, until
/// `|rgb − goal|² < tol` or `max_iter` steps.
fn newton(c: &mut [f64; 3], goal: [f64; 3], q: &Quadrature, max_iter: usize, tol: f64) {
    for _ in 0..max_iter {
        let mut f = [-goal[0], -goal[1], -goal[2]];
        let mut jac = [[0.0f64; 3]; 3];
        for (&t, wj) in q.t.iter().zip(q.w.iter()) {
            let x = (c[0] * t + c[1]) * t + c[2];
            let r = 1.0 / (1.0 + x * x).sqrt();
            let s = 0.5 + 0.5 * x * r;
            let d = 0.5 * r * r * r; // S'(x)
            let dx = [d * t * t, d * t, d];
            for k in 0..3 {
                f[k] += s * wj[k];
                for m in 0..3 {
                    jac[k][m] += dx[m] * wj[k];
                }
            }
        }
        let f2 = f[0] * f[0] + f[1] * f[1] + f[2] * f[2];
        if f2 < tol {
            return;
        }
        let Some(step) = solve3(jac, [-f[0], -f[1], -f[2]]) else { return };
        let mut alpha = 1.0;
        loop {
            let cn = [c[0] + alpha * step[0], c[1] + alpha * step[1], c[2] + alpha * step[2]];
            let r = sigmoid_rgb(&cn, q);
            let e = [r[0] - goal[0], r[1] - goal[1], r[2] - goal[2]];
            if e[0] * e[0] + e[1] * e[1] + e[2] * e[2] < f2 {
                *c = cn;
                break;
            }
            alpha *= 0.5;
            if alpha < 1e-6 {
                return;
            }
        }
    }
}

/// Normalized sigmoid coefficients whose D65 reflectance is `target` (linear sRGB).
fn solve_sigmoid(target: [f32; 3]) -> Option<[f32; 3]> {
    let (coarse, fine) = (coarse_quadrature(), fine_quadrature());
    let target = target.map(|v| v as f64);
    let g0 = ((target[0] + target[1] + target[2]) / 3.0).clamp(0.02, 0.98);
    let u = 2.0 * g0 - 1.0;
    let mut c = [0.0, 0.0, u / (1.0 - u * u).sqrt()];
    let start = sigmoid_rgb(&c, coarse);
    const STEPS: usize = 8;
    for k in 1..=STEPS {
        let a = k as f64 / STEPS as f64;
        let goal = [0, 1, 2].map(|i| start[i] + (target[i] - start[i]) * a);
        newton(&mut c, goal, coarse, 30, 1e-10);
    }
    newton(&mut c, target, fine, 30, 1e-16);
    c.iter().all(|v| v.is_finite()).then(|| c.map(|v| v as f32))
}

/// An emission colour given as linear sRGB, relative to a D65 white:
/// `RgbIlluminant::new([1,1,1])` is CIE D65 scaled to luminance Y = 1, and
/// maps back to linear sRGB (1, 1, 1) (to 1e-3).
///
/// pbrt-v4's construction: `m = max(rgb)`, the chromaticity `rgb / 2m` (max
/// component ½, which keeps the sigmoid fit smooth and well inside (0, 1)) is
/// upsampled with [`RgbAlbedo`], and the result is `2m · s(λ) · D65₁(λ)` with
/// D65₁ = [`cie::d65_normalized`]. Because the albedo fit is exact under D65,
/// the emitted XYZ is `2m × XYZ(s under D65)`, i.e. exactly the input colour:
/// intensities in the renderer are luminances. Negative inputs clamp to 0.
#[derive(Clone, Copy, Debug)]
pub struct RgbIlluminant {
    albedo: RgbAlbedo,
    scale: f32,
    illum: &'static Piecewise,
}

impl RgbIlluminant {
    pub fn new(rgb: [f32; 3]) -> Self {
        let rgb = rgb.map(|v| if v.is_finite() { v.max(0.0) } else { 0.0 });
        let m = rgb.iter().copied().fold(0.0f32, f32::max);
        let scale = 2.0 * m;
        let albedo = if m > 0.0 { RgbAlbedo::new(rgb.map(|v| v / scale)) } else { RgbAlbedo::new([0.0; 3]) };
        RgbIlluminant { albedo, scale, illum: cie::d65_normalized() }
    }
}

impl Spectrum for RgbIlluminant {
    #[inline]
    fn eval(&self, l: f32) -> f32 {
        self.scale * self.albedo.eval(l) * self.illum.eval(l)
    }
}

/// Luminance Y of a spectrum (CIE 1931), normalized by the Y integral: the
/// 1 nm sum over 360–830 nm, so a constant 1 has Y = 1.
pub fn luminance_y(s: &dyn Spectrum) -> f32 {
    let mut y = 0.0f64;
    for i in 0..=470u32 {
        let l = 360.0 + i as f32;
        y += s.eval(l) as f64 * cie::xyz_bar(l)[1] as f64;
    }
    (y / cie::CIE_Y_INTEGRAL as f64) as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip(rgb: [f32; 3]) -> [f32; 3] {
        let a = RgbAlbedo::new(rgb);
        cie::reflectance_srgb(|l| a.eval(l))
    }

    #[test]
    fn piecewise_interpolates_and_clamps() {
        let p = Piecewise::new(&[(400.0, 0.0), (500.0, 1.0)]);
        assert_eq!(p.eval(300.0), 0.0);
        assert!((p.eval(450.0) - 0.5).abs() < 1e-6);
        assert_eq!(p.eval(900.0), 1.0);
    }

    #[test]
    fn white_albedo_is_flat() {
        let w = RgbAlbedo::new([1.0, 1.0, 1.0]);
        for l in (360..830).step_by(10) {
            assert!((w.eval(l as f32) - 1.0).abs() < 1e-5);
        }
    }

    #[test]
    fn greys_are_exact_constants() {
        for g in [0.0f32, 0.04, 0.18, 0.5, 0.9, 1.0] {
            let a = RgbAlbedo::new([g, g, g]);
            assert!(a.coefficients().is_none());
            for l in [300.0f32, 360.0, 555.0, 830.0, 900.0] {
                assert_eq!(a.eval(l), g);
            }
        }
        // out-of-range input clamps
        assert_eq!(RgbAlbedo::new([2.0, 2.0, 2.0]).eval(500.0), 1.0);
        assert_eq!(RgbAlbedo::new([-1.0, -1.0, -1.0]).eval(500.0), 0.0);
    }

    #[test]
    fn albedo_round_trips_the_srgb_cube() {
        let vals = [0.0f32, 0.02, 0.1, 0.3, 0.6, 0.9, 0.98, 1.0];
        let mut worst = (0.0f32, [0.0f32; 3]);
        for &r in &vals {
            for &g in &vals {
                for &b in &vals {
                    let rgb = [r, g, b];
                    let back = round_trip(rgb);
                    for k in 0..3 {
                        let e = (back[k] - rgb[k]).abs();
                        if e > worst.0 {
                            worst = (e, rgb);
                        }
                    }
                }
            }
        }
        // spec: < 0.5% for in-gamut colours; the solver reaches ~1e-6
        assert!(worst.0 < 5e-3, "worst round trip error {} at {:?}", worst.0, worst.1);
        assert!(worst.0 < 1e-4, "worst round trip error {} at {:?}", worst.0, worst.1);
    }

    #[test]
    fn albedo_is_bounded_and_smooth() {
        for rgb in [[1.0f32, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0], [0.9, 0.6, 0.1], [0.05, 0.4, 0.7]] {
            let a = RgbAlbedo::new(rgb);
            let c = a.coefficients().expect("coloured albedo is a sigmoid");
            assert!(c.iter().all(|v| v.is_finite()));
            for l in 300..900 {
                let v = a.eval(l as f32);
                assert!((0.0..=1.0).contains(&v), "{rgb:?} {l} {v}");
            }
            // the raw-nm coefficients describe the same function
            let l = 517.0f32;
            let x = (c[0] as f64 * l as f64 + c[1] as f64) * l as f64 + c[2] as f64;
            let s = 0.5 + 0.5 * x / (1.0 + x * x).sqrt();
            assert!((s as f32 - a.eval(l)).abs() < 1e-4, "{rgb:?}");
        }
    }

    #[test]
    fn white_illuminant_is_unit_luminance_d65() {
        let w = RgbIlluminant::new([1.0, 1.0, 1.0]);
        assert!((luminance_y(&w) - 1.0).abs() < 1e-5, "{}", luminance_y(&w));
        let rgb = cie::xyz_to_linear_srgb(cie::emission_xyz(&w));
        for c in rgb {
            assert!((c - 1.0).abs() < 1e-3, "{rgb:?}");
        }
        let d = cie::d65_normalized();
        for l in [400.0f32, 555.0, 700.0] {
            assert!((w.eval(l) - d.eval(l)).abs() < 1e-6);
        }
        assert_eq!(RgbIlluminant::new([0.0; 3]).eval(555.0), 0.0);
    }

    #[test]
    fn coloured_illuminants_round_trip() {
        for rgb in [[2.0f32, 1.0, 0.5], [0.1, 0.4, 3.0], [5.0, 5.0, 5.0], [1.0, 0.0, 0.0], [0.02, 0.03, 0.01]] {
            let s = RgbIlluminant::new(rgb);
            let back = cie::xyz_to_linear_srgb(cie::emission_xyz(&s));
            let m = rgb.iter().copied().fold(0.0f32, f32::max);
            for k in 0..3 {
                assert!((back[k] - rgb[k]).abs() < 5e-3 * m, "{rgb:?} -> {back:?}");
            }
            assert!(luminance_y(&s) > 0.0);
        }
    }

    #[test]
    fn normalized_blackbody_has_unit_luminance() {
        for k in [1900.0f32, 2700.0, 4000.0, 5000.0, 6500.0, 10000.0] {
            let b = blackbody_normalized(k);
            assert!((luminance_y(&b) - 1.0).abs() < 1e-4, "{k}: {}", luminance_y(&b));
            let c = Blackbody::with_luminance(k, 3.0);
            assert!((luminance_y(&c) - 3.0).abs() < 1e-3);
        }
        // the peak-normalized form is unchanged
        let b = Blackbody::new(5000.0);
        let peak = 2.897_772e-3 / 5000.0 * 1e9;
        assert!((b.eval(peak) - 1.0).abs() < 1e-4);
        // warm vs cool: a 2700 K lamp is redder than 6500 K
        let warm = cie::xyz_to_linear_srgb(cie::emission_xyz(&blackbody_normalized(2700.0)));
        let cool = cie::xyz_to_linear_srgb(cie::emission_xyz(&blackbody_normalized(6500.0)));
        assert!(warm[0] / warm[2] > cool[0] / cool[2]);
    }
}
