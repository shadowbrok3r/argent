//! Sampled spectra and the wavelengths they are sampled at.

use std::ops::{Add, AddAssign, Div, DivAssign, Index, IndexMut, Mul, MulAssign, Neg, Sub, SubAssign};

/// Wavelengths carried per path.
pub const N: usize = 4;
/// Range of the colour matching functions, nm.
pub const LAMBDA_MIN: f32 = 360.0;
pub const LAMBDA_MAX: f32 = 830.0;

/// One value per carried wavelength.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Spec(pub [f32; N]);

impl Spec {
    pub const ZERO: Spec = Spec([0.0; N]);
    pub const ONE: Spec = Spec([1.0; N]);

    #[inline]
    pub fn splat(v: f32) -> Self {
        Spec([v; N])
    }
    #[inline]
    pub fn map(self, f: impl Fn(f32) -> f32) -> Self {
        Spec(self.0.map(f))
    }
    #[inline]
    pub fn zip(self, o: Spec, f: impl Fn(f32, f32) -> f32) -> Self {
        let mut r = [0.0; N];
        for i in 0..N {
            r[i] = f(self.0[i], o.0[i]);
        }
        Spec(r)
    }
    #[inline]
    pub fn max_value(self) -> f32 {
        self.0.iter().copied().fold(f32::MIN, f32::max)
    }
    #[inline]
    pub fn min_value(self) -> f32 {
        self.0.iter().copied().fold(f32::MAX, f32::min)
    }
    #[inline]
    pub fn average(self) -> f32 {
        self.0.iter().sum::<f32>() / N as f32
    }
    #[inline]
    pub fn is_black(self) -> bool {
        self.0.iter().all(|&v| v == 0.0)
    }
    #[inline]
    pub fn is_finite(self) -> bool {
        self.0.iter().all(|v| v.is_finite())
    }
    #[inline]
    pub fn exp(self) -> Self {
        self.map(f32::exp)
    }
    #[inline]
    pub fn sqrt(self) -> Self {
        self.map(|v| v.max(0.0).sqrt())
    }
    #[inline]
    pub fn clamp(self, lo: f32, hi: f32) -> Self {
        self.map(|v| v.clamp(lo, hi))
    }
    /// Division that yields 0 where the divisor is 0 (pbrt's SafeDiv).
    #[inline]
    pub fn safe_div(self, d: Spec) -> Self {
        self.zip(d, |a, b| if b != 0.0 { a / b } else { 0.0 })
    }
    #[inline]
    pub fn lerp(t: f32, a: Spec, b: Spec) -> Spec {
        a * (1.0 - t) + b * t
    }
}

impl Index<usize> for Spec {
    type Output = f32;
    #[inline]
    fn index(&self, i: usize) -> &f32 {
        &self.0[i]
    }
}
impl IndexMut<usize> for Spec {
    #[inline]
    fn index_mut(&mut self, i: usize) -> &mut f32 {
        &mut self.0[i]
    }
}

macro_rules! spec_binop {
    ($tr:ident, $f:ident, $atr:ident, $af:ident, $op:tt) => {
        impl $tr for Spec {
            type Output = Spec;
            #[inline]
            fn $f(self, o: Spec) -> Spec {
                let mut r = self.0;
                for i in 0..N {
                    r[i] = r[i] $op o.0[i];
                }
                Spec(r)
            }
        }
        impl $tr<f32> for Spec {
            type Output = Spec;
            #[inline]
            fn $f(self, o: f32) -> Spec {
                Spec(self.0.map(|v| v $op o))
            }
        }
        impl $atr for Spec {
            #[inline]
            fn $af(&mut self, o: Spec) {
                *self = *self $op o;
            }
        }
        impl $atr<f32> for Spec {
            #[inline]
            fn $af(&mut self, o: f32) {
                *self = *self $op o;
            }
        }
    };
}
spec_binop!(Add, add, AddAssign, add_assign, +);
spec_binop!(Sub, sub, SubAssign, sub_assign, -);
spec_binop!(Mul, mul, MulAssign, mul_assign, *);
spec_binop!(Div, div, DivAssign, div_assign, /);

impl Mul<Spec> for f32 {
    type Output = Spec;
    #[inline]
    fn mul(self, s: Spec) -> Spec {
        s * self
    }
}
impl Neg for Spec {
    type Output = Spec;
    #[inline]
    fn neg(self) -> Spec {
        self.map(|v| -v)
    }
}

/// The wavelengths a path carries and the density they were drawn with.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Wavelengths {
    pub lambda: [f32; N],
    pub pdf: [f32; N],
}

impl Wavelengths {
    /// Importance-sample the visible range (pbrt-v4's
    /// `SampleVisibleWavelengths`), stratified: the hero wavelength is drawn
    /// from `u` and the rest are `1/N` rotations of it.
    pub fn sample_visible(u: f32) -> Self {
        let mut lambda = [0.0; N];
        let mut pdf = [0.0; N];
        for i in 0..N {
            let mut up = u + i as f32 / N as f32;
            if up > 1.0 {
                up -= 1.0;
            }
            lambda[i] = sample_visible_wavelength(up);
            pdf[i] = visible_wavelength_pdf(lambda[i]);
        }
        Wavelengths { lambda, pdf }
    }

    /// Uniform sampling over `[min, max]`, stratified the same way.
    pub fn sample_uniform(u: f32, min: f32, max: f32) -> Self {
        let mut lambda = [0.0; N];
        let delta = (max - min) / N as f32;
        lambda[0] = min + u * (max - min);
        for i in 1..N {
            lambda[i] = lambda[i - 1] + delta;
            if lambda[i] > max {
                lambda[i] = min + (lambda[i] - max);
            }
        }
        Wavelengths { lambda, pdf: [1.0 / (max - min); N] }
    }

    /// Keep only the hero wavelength: what a dispersive interface must do,
    /// because the secondary wavelengths would have refracted differently.
    pub fn terminate_secondary(&mut self) {
        if self.secondary_terminated() {
            return;
        }
        for i in 1..N {
            self.pdf[i] = 0.0;
        }
        self.pdf[0] /= N as f32;
    }

    pub fn secondary_terminated(&self) -> bool {
        self.pdf[1..].iter().all(|&p| p == 0.0)
    }

    #[inline]
    pub fn pdf_spec(&self) -> Spec {
        Spec(self.pdf)
    }
}

/// pbrt-v4: wavelengths distributed close to the CIE Y response.
#[inline]
pub fn sample_visible_wavelength(u: f32) -> f32 {
    538.0 - 138.888_89 * (0.856_910_6 - 1.827_502 * u).atanh()
}

#[inline]
pub fn visible_wavelength_pdf(lambda: f32) -> f32 {
    if !(LAMBDA_MIN..=LAMBDA_MAX).contains(&lambda) {
        return 0.0;
    }
    let c = (0.0072 * (lambda - 538.0)).cosh();
    0.003_939_804_2 / (c * c)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn visible_sampling_stays_in_range_and_has_density() {
        for i in 0..1000 {
            let u = (i as f32 + 0.5) / 1000.0;
            let w = Wavelengths::sample_visible(u);
            for k in 0..N {
                assert!(w.lambda[k] >= LAMBDA_MIN && w.lambda[k] <= LAMBDA_MAX, "{:?}", w);
                assert!(w.pdf[k] > 0.0);
            }
        }
    }

    #[test]
    fn terminating_keeps_the_hero() {
        let mut w = Wavelengths::sample_visible(0.3);
        let p0 = w.pdf[0];
        w.terminate_secondary();
        assert!(w.secondary_terminated());
        assert!((w.pdf[0] - p0 / N as f32).abs() < 1e-9);
    }
}
