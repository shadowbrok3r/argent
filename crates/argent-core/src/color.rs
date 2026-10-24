//! Camera white balance: Bradford chromatic adaptation from the studio's
//! key-light white to D65, folded into the XYZ → linear sRGB transform.

use argent_spectra::{cie, Blackbody, Spectrum};

pub type M3 = [[f32; 3]; 3];

const BRADFORD: M3 = [[0.8951, 0.2664, -0.1614], [-0.7502, 1.7135, 0.0367], [0.0389, -0.0685, 1.0296]];
const BRADFORD_INV: M3 = [[0.986_993, -0.147_054, 0.159_963], [0.432_305, 0.518_36, 0.049_291], [-0.008_529, 0.040_043, 0.968_487]];
const XYZ_TO_SRGB: M3 = [[3.240_454_2, -1.537_138_5, -0.498_531_4], [-0.969_266, 1.876_010_8, 0.041_556], [0.055_643_4, -0.204_025_9, 1.057_225_2]];
const D65: [f32; 3] = [0.950_47, 1.0, 1.088_83];

fn mul(a: &M3, b: &M3) -> M3 {
    let mut r = [[0.0; 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            r[i][j] = (0..3).map(|k| a[i][k] * b[k][j]).sum();
        }
    }
    r
}

fn apply(m: &M3, v: [f32; 3]) -> [f32; 3] {
    [
        m[0][0] * v[0] + m[0][1] * v[1] + m[0][2] * v[2],
        m[1][0] * v[0] + m[1][1] * v[1] + m[1][2] * v[2],
        m[2][0] * v[0] + m[2][1] * v[1] + m[2][2] * v[2],
    ]
}

/// XYZ of a blackbody at `kelvin`, normalized to Y = 1.
pub fn blackbody_white(kelvin: f32) -> [f32; 3] {
    let bb = Blackbody::new(kelvin);
    let mut xyz = [0.0f32; 3];
    let mut l = 360.0f32;
    while l <= 830.0 {
        let c = cie::xyz_bar(l);
        let e = bb.eval(l);
        for i in 0..3 {
            xyz[i] += c[i] * e;
        }
        l += 1.0;
    }
    [xyz[0] / xyz[1], 1.0, xyz[2] / xyz[1]]
}

/// XYZ → linear sRGB, white-balanced so `white` (XYZ, Y=1) maps to neutral.
pub fn camera_matrix(white: Option<[f32; 3]>) -> M3 {
    let Some(w) = white else { return XYZ_TO_SRGB };
    let src = apply(&BRADFORD, w);
    let dst = apply(&BRADFORD, D65);
    let d: M3 = [[dst[0] / src[0], 0.0, 0.0], [0.0, dst[1] / src[1], 0.0], [0.0, 0.0, dst[2] / src[2]]];
    let cat = mul(&BRADFORD_INV, &mul(&d, &BRADFORD));
    mul(&XYZ_TO_SRGB, &cat)
}

#[inline]
pub fn to_rgb(m: &M3, xyz: [f32; 3]) -> [f32; 3] {
    apply(m, xyz)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn balanced_white_is_neutral() {
        let w = blackbody_white(5600.0);
        let m = camera_matrix(Some(w));
        let rgb = to_rgb(&m, w);
        assert!((rgb[0] - rgb[1]).abs() < 0.01 && (rgb[2] - rgb[1]).abs() < 0.01, "{rgb:?}");
        // D65 itself passes through unchanged.
        let m65 = camera_matrix(Some(D65));
        let a = to_rgb(&m65, [0.3, 0.4, 0.5]);
        let b = to_rgb(&XYZ_TO_SRGB, [0.3, 0.4, 0.5]);
        for i in 0..3 {
            assert!((a[i] - b[i]).abs() < 1e-3);
        }
    }
}
