//! CIE 1931 2° colour matching, the CIE D65 illuminant, and the XYZ ↔ linear
//! sRGB transforms.
//!
//! Data (embedded in `data::cie`, checked against the CIE's own files):
//!
//! * colour matching functions: the tabulated CIE 1931 2° standard observer
//!   at 1 nm, 360–830 nm (CIE 015:2018; dataset `CIE_xyz_1931_2deg.csv`,
//!   DOI 10.25039/CIE.DS.xvudnb9b, identical to CVRL's `ciexyz31_1.csv`).
//!   [`xyz_bar`] interpolates it linearly, so it is exact at whole
//!   nanometres;
//! * illuminant: CIE standard illuminant D65 relative SPD, 300–830 nm
//!   (dataset `CIE_std_illum_D65.csv`, DOI 10.25039/CIE.DS.hjfjmt59), stored
//!   at 5 nm because the official 1 nm table is the linear interpolation of
//!   the 5 nm values.
//!
//! Integrals are 1 nm sums over 360–830 nm, the same quadrature as
//! [`crate::spectrum::luminance_y`] and pbrt-v4, so a spectrum's luminance,
//! its XYZ and the normalizations below all agree exactly. With these tables
//! D65 lands at xy = (0.31273, 0.32902), i.e. XYZ (0.950 47, 1, 1.088 83):
//! the white point the sRGB matrices below assume, to 1e-5.

use crate::data::cie::{CMF_1931_2DEG, D65_300_830_5NM};
use crate::spec::{Spec, Wavelengths, N};
use crate::spectrum::{Piecewise, Spectrum};
use std::sync::OnceLock;

/// ∫ ȳ(λ) dλ over 360..830 nm for the tabulated CIE 1931 2° observer: the
/// 1 nm sum of the table (106.856 917; pbrt-v4 uses 106.856 895).
pub const CIE_Y_INTEGRAL: f32 = 106.856_917;

/// First and last tabulated wavelength of the colour matching functions, nm.
pub const CMF_MIN: f32 = 360.0;
pub const CMF_MAX: f32 = 830.0;

/// x̄, ȳ, z̄ at `lambda` nm: the CIE table, linearly interpolated; zero
/// outside 360–830 nm.
#[inline]
pub fn xyz_bar(lambda: f32) -> [f32; 3] {
    let x = lambda - CMF_MIN;
    // `contains` is false for NaN as well.
    if !(0.0..=(CMF_MAX - CMF_MIN)).contains(&x) {
        return [0.0; 3];
    }
    let i = (x as usize).min(CMF_1931_2DEG.len() - 2);
    let t = x - i as f32;
    let a = CMF_1931_2DEG[i];
    let b = CMF_1931_2DEG[i + 1];
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t]
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

/// CIE standard illuminant D65, relative SPD (100 at 560 nm), 300–830 nm,
/// linearly interpolated and held constant outside the table.
pub fn d65() -> &'static Piecewise {
    static D65: OnceLock<Piecewise> = OnceLock::new();
    D65.get_or_init(|| {
        let pairs: Vec<(f32, f32)> =
            D65_300_830_5NM.iter().enumerate().map(|(i, &v)| (300.0 + 5.0 * i as f32, v)).collect();
        Piecewise::new(&pairs)
    })
}

/// D65 scaled to unit luminance: `luminance_y(d65_normalized()) == 1`.
/// The emitter behind [`crate::RgbIlluminant`].
pub fn d65_normalized() -> &'static Piecewise {
    static D65N: OnceLock<Piecewise> = OnceLock::new();
    D65N.get_or_init(|| {
        let y = crate::spectrum::luminance_y(d65());
        d65().clone().scaled(1.0 / y)
    })
}

/// XYZ of an emission spectrum: the 1 nm sum of `s·cmf` over 360–830 nm
/// divided by [`CIE_Y_INTEGRAL`] (a constant 1 has Y = 1; Y equals
/// [`crate::spectrum::luminance_y`]).
pub fn emission_xyz(s: &dyn Spectrum) -> [f32; 3] {
    let mut acc = [0.0f64; 3];
    for (i, c) in CMF_1931_2DEG.iter().enumerate() {
        let v = s.eval(CMF_MIN + i as f32) as f64;
        for k in 0..3 {
            acc[k] += v * c[k] as f64;
        }
    }
    let n = CIE_Y_INTEGRAL as f64;
    [(acc[0] / n) as f32, (acc[1] / n) as f32, (acc[2] / n) as f32]
}

/// Per-nanometre weights D65(λ)·cmf(λ) / Σ D65·ȳ for 360..=830 nm: a
/// reflectance `r` lit by D65 has XYZ = Σ r(λ)·w(λ), and r ≡ 1 gives Y = 1.
pub(crate) fn d65_cmf_weights() -> &'static [[f64; 3]; 471] {
    static W: OnceLock<[[f64; 3]; 471]> = OnceLock::new();
    W.get_or_init(|| {
        let d = d65();
        let mut w = [[0.0f64; 3]; 471];
        let mut ny = 0.0f64;
        for (i, c) in CMF_1931_2DEG.iter().enumerate() {
            let e = d.eval(CMF_MIN + i as f32) as f64;
            for k in 0..3 {
                w[i][k] = e * c[k] as f64;
            }
            ny += w[i][1];
        }
        for row in w.iter_mut() {
            for v in row.iter_mut() {
                *v /= ny;
            }
        }
        w
    })
}

/// XYZ of a reflectance (or transmittance) `r(λ)` under D65, normalized so a
/// perfect white (r ≡ 1) has Y = 1. 1 nm sum over 360–830 nm.
pub fn reflectance_xyz(r: impl Fn(f32) -> f32) -> [f32; 3] {
    let w = d65_cmf_weights();
    let mut acc = [0.0f64; 3];
    for (i, wi) in w.iter().enumerate() {
        let v = r(CMF_MIN + i as f32) as f64;
        for k in 0..3 {
            acc[k] += v * wi[k];
        }
    }
    [acc[0] as f32, acc[1] as f32, acc[2] as f32]
}

/// Linear sRGB of a reflectance (or transmittance) under D65: white is
/// (1, 1, 1) to within 1e-4.
pub fn reflectance_srgb(r: impl Fn(f32) -> f32) -> [f32; 3] {
    xyz_to_linear_srgb(reflectance_xyz(r))
}

/// CIE 1976 L*a*b* of `xyz` relative to the D65 white of these tables
/// (`reflectance_xyz(|_| 1.0)`), for colour checks in tests and tools.
pub fn xyz_to_lab(xyz: [f32; 3]) -> [f32; 3] {
    static WHITE: OnceLock<[f32; 3]> = OnceLock::new();
    let w = WHITE.get_or_init(|| reflectance_xyz(|_| 1.0));
    let f = |t: f32| {
        const D: f32 = 6.0 / 29.0;
        if t > D * D * D { t.cbrt() } else { t / (3.0 * D * D) + 4.0 / 29.0 }
    };
    let (fx, fy, fz) = (f(xyz[0] / w[0]), f(xyz[1] / w[1]), f(xyz[2] / w[2]));
    [116.0 * fy - 16.0, 500.0 * (fx - fy), 200.0 * (fy - fz)]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn y_integral_matches_the_table() {
        let s: f64 = CMF_1931_2DEG.iter().map(|c| c[1] as f64).sum();
        assert!((s - 106.857).abs() < 1e-3, "{s}");
        assert!((s as f32 - CIE_Y_INTEGRAL).abs() < 1e-4, "{s}");
    }

    #[test]
    fn table_lookups_are_exact_at_whole_nanometres_and_zero_outside() {
        assert_eq!(xyz_bar(555.0)[1], 1.0);
        assert_eq!(xyz_bar(360.0), [0.0001299, 3.917e-6, 0.0006061]);
        assert_eq!(xyz_bar(830.0), [1.251141e-6, 4.5181e-7, 0.0]);
        assert_eq!(xyz_bar(359.9), [0.0; 3]);
        assert_eq!(xyz_bar(830.1), [0.0; 3]);
        assert_eq!(xyz_bar(f32::NAN), [0.0; 3]);
        // halfway between two rows
        let (a, b, m) = (xyz_bar(600.0), xyz_bar(601.0), xyz_bar(600.5));
        for k in 0..3 {
            assert!((m[k] - 0.5 * (a[k] + b[k])).abs() < 1e-6);
        }
    }

    #[test]
    fn d65_is_the_cie_table() {
        let d = d65();
        assert_eq!(d.eval(560.0), 100.0);
        assert_eq!(d.eval(300.0), 0.0341);
        assert_eq!(d.eval(830.0), 60.3125);
        // 1 nm values of the official table are the lerp of the 5 nm ones
        assert!((d.eval(301.0) - 0.360_14).abs() < 1e-4);
        assert!((d.eval(587.0) - 90.8163).abs() < 1e-3, "{}", d.eval(587.0));
    }

    #[test]
    fn d65_white_point() {
        let xyz = reflectance_xyz(|_| 1.0);
        let s = xyz[0] + xyz[1] + xyz[2];
        let (x, y) = (xyz[0] / s, xyz[1] / s);
        assert!((x - 0.3127).abs() < 2e-4 && (y - 0.3290).abs() < 2e-4, "{x} {y}");
        assert!((xyz[1] - 1.0).abs() < 1e-6);
        let rgb = xyz_to_linear_srgb(xyz);
        for c in rgb {
            assert!((c - 1.0).abs() < 1e-3, "{rgb:?}");
        }
        let lab = xyz_to_lab(xyz);
        assert!((lab[0] - 100.0).abs() < 1e-3 && lab[1].abs() < 1e-3 && lab[2].abs() < 1e-3, "{lab:?}");
    }

    #[test]
    fn normalized_d65_has_unit_luminance() {
        let y = crate::spectrum::luminance_y(d65_normalized());
        assert!((y - 1.0).abs() < 1e-5, "{y}");
        let xyz = emission_xyz(d65_normalized());
        assert!((xyz[1] - 1.0).abs() < 1e-5);
        let rgb = xyz_to_linear_srgb(xyz);
        for c in rgb {
            assert!((c - 1.0).abs() < 1e-3, "{rgb:?}");
        }
    }

    #[test]
    fn srgb_matrices_invert() {
        for v in [[1.0f32, 0.0, 0.0], [0.2, 0.5, 0.9], [0.3, 0.3, 0.3]] {
            let back = xyz_to_linear_srgb(linear_srgb_to_xyz(v));
            for k in 0..3 {
                assert!((back[k] - v[k]).abs() < 1e-5, "{v:?} {back:?}");
            }
        }
    }

    #[test]
    fn monte_carlo_estimate_of_a_constant_is_unit_luminance() {
        let n = 4096;
        let mut y = 0.0f64;
        for i in 0..n {
            let wl = Wavelengths::sample_visible((i as f32 + 0.5) / n as f32);
            y += to_xyz(Spec::ONE, &wl)[1] as f64;
        }
        let y = y / n as f64;
        assert!((y - 1.0).abs() < 2e-3, "{y}");
    }
}
