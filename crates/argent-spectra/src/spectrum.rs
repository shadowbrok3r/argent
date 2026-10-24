//! Continuous spectra: the [`Spectrum`] trait and its basic implementations.

use crate::spec::{Spec, Wavelengths, N};

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

/// Planck emitter, normalized so its peak is 1.
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

/// A reflectance in `[0, 1]` given as linear sRGB.
///
/// PLACEHOLDER: a smooth partition of unity (white → constant 1, bounded),
/// not an exact round trip. To be replaced by a Jakob & Hanika (2019)
/// sigmoid-polynomial fit solved per colour (Gauss-Newton, cached), keeping
/// `new` and `eval`.
#[derive(Clone, Copy, Debug)]
pub struct RgbAlbedo {
    rgb: [f32; 3],
}

impl RgbAlbedo {
    pub fn new(rgb: [f32; 3]) -> Self {
        RgbAlbedo { rgb: rgb.map(|v| v.clamp(0.0, 1.0)) }
    }
}

#[inline]
fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

impl Spectrum for RgbAlbedo {
    fn eval(&self, l: f32) -> f32 {
        let wb = 1.0 - smoothstep(470.0, 515.0, l);
        let wr = smoothstep(565.0, 610.0, l);
        let wg = (1.0 - wb - wr).max(0.0);
        self.rgb[2] * wb + self.rgb[1] * wg + self.rgb[0] * wr
    }
}

/// An emission colour given as linear sRGB, relative to a D65 white:
/// `RgbIlluminant::new([1,1,1])` is D65-like white.
///
/// PLACEHOLDER: albedo shape times a 6504 K blackbody. To be replaced by the
/// sigmoid fit times the tabulated CIE D65 SPD.
#[derive(Clone, Copy, Debug)]
pub struct RgbIlluminant {
    albedo: RgbAlbedo,
    scale: f32,
    illum: Blackbody,
}

impl RgbIlluminant {
    pub fn new(rgb: [f32; 3]) -> Self {
        let m = rgb.iter().copied().fold(0.0f32, f32::max);
        let scale = if m > 0.0 { m } else { 0.0 };
        let norm = if m > 0.0 { rgb.map(|v| v / m) } else { [0.0; 3] };
        RgbIlluminant { albedo: RgbAlbedo::new(norm), scale, illum: Blackbody::new(6504.0) }
    }
}

impl Spectrum for RgbIlluminant {
    #[inline]
    fn eval(&self, l: f32) -> f32 {
        self.scale * self.albedo.eval(l) * self.illum.eval(l)
    }
}

/// Luminance Y of a spectrum (CIE 1931), normalized by the Y integral.
pub fn luminance_y(s: &dyn Spectrum) -> f32 {
    let mut y = 0.0f32;
    let mut l = 360.0f32;
    while l <= 830.0 {
        y += s.eval(l) * crate::cie::xyz_bar(l)[1];
        l += 1.0;
    }
    y / crate::cie::CIE_Y_INTEGRAL
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
