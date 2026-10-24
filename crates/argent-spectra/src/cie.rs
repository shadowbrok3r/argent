//! CIE 1931 2° colour matching and the XYZ → linear sRGB transform.
//!
//! PLACEHOLDER: `xyz_bar` is the Wyman, Sloan & Shirley (2013) multi-lobe
//! Gaussian fit (≈1% error). To be replaced by the tabulated CIE 1931 2°
//! functions at 1 nm (360..=830) with linear interpolation; keep the
//! signatures.

use crate::spec::{Spec, Wavelengths, N};

/// ∫ ȳ(λ) dλ over 360..830 nm for the tabulated CIE 1931 2° observer.
pub const CIE_Y_INTEGRAL: f32 = 106.856_895;

#[inline]
fn g(x: f32, mu: f32, s1: f32, s2: f32) -> f32 {
    let t = (x - mu) / if x < mu { s1 } else { s2 };
    (-0.5 * t * t).exp()
}

/// x̄, ȳ, z̄ at `lambda` nm.
#[inline]
pub fn xyz_bar(lambda: f32) -> [f32; 3] {
    let x = 1.056 * g(lambda, 599.8, 37.9, 31.0) + 0.362 * g(lambda, 442.0, 16.0, 26.7)
        - 0.065 * g(lambda, 501.1, 20.4, 26.2);
    let y = 0.821 * g(lambda, 568.8, 46.9, 40.5) + 0.286 * g(lambda, 530.9, 16.3, 31.1);
    let z = 1.217 * g(lambda, 437.0, 11.8, 36.0) + 0.681 * g(lambda, 459.0, 26.0, 13.8);
    [x, y, z]
}

/// Monte Carlo estimate of the XYZ tristimulus of a sampled spectrum
/// (pbrt-v4 `SampledSpectrum::ToXYZ`): average of `L(λ)·cmf(λ)/pdf(λ)`,
/// divided by [`CIE_Y_INTEGRAL`] so that a constant spectrum of 1 gives Y≈1.
#[inline]
pub fn to_xyz(l: Spec, wl: &Wavelengths) -> [f32; 3] {
    let mut xyz = [0.0f32; 3];
    for i in 0..N {
        let p = wl.pdf[i];
        if p == 0.0 || l.0[i] == 0.0 {
            continue;
        }
        let c = xyz_bar(wl.lambda[i]);
        let w = l.0[i] / p;
        xyz[0] += c[0] * w;
        xyz[1] += c[1] * w;
        xyz[2] += c[2] * w;
    }
    let s = 1.0 / (N as f32 * CIE_Y_INTEGRAL);
    [xyz[0] * s, xyz[1] * s, xyz[2] * s]
}

/// XYZ (D65 white) to linear sRGB / Rec.709 primaries.
#[inline]
pub fn xyz_to_linear_srgb(xyz: [f32; 3]) -> [f32; 3] {
    let [x, y, z] = xyz;
    [
        3.240_454_2 * x - 1.537_138_5 * y - 0.498_531_4 * z,
        -0.969_266 * x + 1.876_010_8 * y + 0.041_556 * z,
        0.055_643_4 * x - 0.204_025_9 * y + 1.057_225_2 * z,
    ]
}

/// Linear sRGB to XYZ (inverse of [`xyz_to_linear_srgb`]).
#[inline]
pub fn linear_srgb_to_xyz(rgb: [f32; 3]) -> [f32; 3] {
    let [r, g, b] = rgb;
    [
        0.412_456_4 * r + 0.357_576_1 * g + 0.180_437_5 * b,
        0.212_672_9 * r + 0.715_152_2 * g + 0.072_175 * b,
        0.019_333_9 * r + 0.119_192 * g + 0.950_304_1 * b,
    ]
}
